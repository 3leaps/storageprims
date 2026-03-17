//! AWS S3 provider implementation for storageprims.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::Duration;

use aws_config::{BehaviorVersion, Region};
use aws_credential_types::Credentials;
use aws_sdk_s3::error::{DisplayErrorContext, ProvideErrorMetadata};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use bytes::Bytes;
use http_body::{Frame, SizeHint};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use storageprims_core::{
    BoxFuture, BoxedByteStream, Capability, CopyRequest, CopyResult, CopyStrategy,
    CredentialSource, GetRangeRequest, ListOptions, ListResult, ObjectMetadata, ObjectSummary,
    ProviderConfig, ProviderKind, PutOptions, PutResult, Result, StorageError, StorageOperation,
    StorageProvider, StorageUri,
};
use tokio::io::{AsyncRead, ReadBuf};

#[derive(Clone, Debug)]
pub struct S3Provider {
    client: Client,
    bucket: String,
    root_prefix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct S3Location {
    bucket: String,
    key: String,
}

struct AsyncReadBody {
    reader: Mutex<BoxedByteStream>,
    finished: AtomicBool,
    remaining: Mutex<u64>,
}

impl S3Provider {
    pub async fn from_uri(uri: &StorageUri, config: ProviderConfig) -> Result<Self> {
        if uri.provider != ProviderKind::S3 {
            return Err(StorageError::InvalidArgument {
                operation: Some(StorageOperation::ConfigureProvider),
                argument: uri.raw.clone(),
                reason: "expected an s3:// URI".to_string(),
            });
        }

        let bucket = uri
            .container
            .clone()
            .or_else(|| config.target.container.clone())
            .ok_or_else(|| StorageError::InvalidArgument {
                operation: Some(StorageOperation::ConfigureProvider),
                argument: uri.raw.clone(),
                reason: "S3 provider requires a bucket/container".to_string(),
            })?;

        let root_prefix = normalize_prefix(
            match (config.target.root_prefix.clone(), uri.path.as_str()) {
                (Some(prefix), _) if !prefix.is_empty() => Some(prefix),
                (None, path) if !path.is_empty() => Some(path.to_string()),
                _ => None,
            },
        );

        Self::from_parts(bucket, root_prefix, config).await
    }

    pub async fn from_config(config: ProviderConfig) -> Result<Self> {
        if config.provider != ProviderKind::S3 {
            return Err(StorageError::InvalidArgument {
                operation: Some(StorageOperation::ConfigureProvider),
                argument: format!("{:?}", config.provider),
                reason: "provider config is not for S3".to_string(),
            });
        }

        let bucket =
            config
                .target
                .container
                .clone()
                .ok_or_else(|| StorageError::InvalidArgument {
                    operation: Some(StorageOperation::ConfigureProvider),
                    argument: "target.container".to_string(),
                    reason: "S3 provider requires a bucket/container".to_string(),
                })?;
        let root_prefix = normalize_prefix(config.target.root_prefix.clone());

        Self::from_parts(bucket, root_prefix, config).await
    }

    async fn from_parts(
        bucket: String,
        root_prefix: Option<String>,
        config: ProviderConfig,
    ) -> Result<Self> {
        let sdk_config = load_sdk_config(&config).await?;

        let mut service_config = aws_sdk_s3::config::Builder::from(&sdk_config);
        service_config.set_force_path_style(config.target.force_path_style);
        let client = Client::from_conf(service_config.build());

        Ok(Self {
            client,
            bucket,
            root_prefix,
        })
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    pub fn root_prefix(&self) -> Option<&str> {
        self.root_prefix.as_deref()
    }

    fn resolve_key(&self, key: &str) -> Result<String> {
        if key.is_empty() {
            return Err(StorageError::InvalidArgument {
                operation: None,
                argument: "key".to_string(),
                reason: "object key must not be empty".to_string(),
            });
        }

        Ok(join_key(self.root_prefix.as_deref(), key))
    }

    fn resolve_list_prefix(&self, prefix: Option<&str>) -> Option<String> {
        match (self.root_prefix.as_deref(), prefix) {
            (Some(root), Some(prefix)) if !prefix.is_empty() => Some(join_key(Some(root), prefix)),
            (Some(root), _) => Some(root.to_string()),
            (None, Some(prefix)) if !prefix.is_empty() => Some(prefix.to_string()),
            _ => None,
        }
    }

    fn strip_root_prefix<'a>(&self, key: &'a str) -> &'a str {
        match self.root_prefix.as_deref() {
            Some(root) => key
                .strip_prefix(root)
                .unwrap_or(key)
                .trim_start_matches('/'),
            None => key,
        }
    }

    fn resolve_copy_location(&self, value: &str, field: &str) -> Result<S3Location> {
        if value.starts_with("s3://") {
            let uri = StorageUri::parse(value)?;
            if uri.provider != ProviderKind::S3 {
                return Err(StorageError::InvalidArgument {
                    operation: Some(StorageOperation::Copy),
                    argument: field.to_string(),
                    reason: "copy target must be an s3:// URI or provider-relative key".to_string(),
                });
            }

            let bucket = uri.container.ok_or_else(|| StorageError::InvalidArgument {
                operation: Some(StorageOperation::Copy),
                argument: field.to_string(),
                reason: "copy target URI is missing a bucket".to_string(),
            })?;

            return Ok(S3Location {
                bucket,
                key: uri.path,
            });
        }

        Ok(S3Location {
            bucket: self.bucket.clone(),
            key: self.resolve_key(value)?,
        })
    }

    async fn relay_copy(
        &self,
        source: &S3Location,
        destination: &S3Location,
    ) -> Result<CopyResult> {
        let object = self
            .client
            .get_object()
            .bucket(&source.bucket)
            .key(&source.key)
            .send()
            .await
            .map_err(|error| {
                map_sdk_error(
                    ProviderKind::S3,
                    StorageOperation::Get,
                    Some(&source.key),
                    Some(&source.bucket),
                    error,
                )
            })?;

        let size = object
            .content_length()
            .and_then(|len| u64::try_from(len).ok());
        let response = self
            .client
            .put_object()
            .bucket(&destination.bucket)
            .key(&destination.key)
            .set_content_length(size.and_then(|len| i64::try_from(len).ok()))
            .body(object.body)
            .send()
            .await
            .map_err(|error| {
                map_sdk_error(
                    ProviderKind::S3,
                    StorageOperation::Put,
                    Some(&destination.key),
                    Some(&destination.bucket),
                    error,
                )
            })?;

        Ok(CopyResult {
            source: format!("s3://{}/{}", source.bucket, source.key),
            destination: format!("s3://{}/{}", destination.bucket, destination.key),
            strategy: CopyStrategy::Relay,
            bytes_copied: size,
            destination_etag: response.e_tag().map(ToString::to_string),
        })
    }
}

impl StorageProvider for S3Provider {
    fn provider_kind(&self) -> ProviderKind {
        ProviderKind::S3
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![Capability::DelimiterListing, Capability::MultipartUpload]
    }

    fn list(&self, options: ListOptions) -> BoxFuture<'_, ListResult> {
        Box::pin(async move {
            let prefix = self.resolve_list_prefix(options.prefix.as_deref());
            let response = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .set_prefix(prefix.clone())
                .set_continuation_token(options.continuation_token)
                .set_max_keys(options.max_keys.and_then(|value| i32::try_from(value).ok()))
                .send()
                .await
                .map_err(|error| {
                    map_sdk_error(
                        ProviderKind::S3,
                        StorageOperation::List,
                        prefix.as_deref(),
                        Some(&self.bucket),
                        error,
                    )
                })?;

            let objects = response
                .contents()
                .iter()
                .filter_map(|item| {
                    item.key().map(|key| ObjectSummary {
                        path: self.strip_root_prefix(key).to_string(),
                        size: item
                            .size()
                            .and_then(|value| u64::try_from(value).ok())
                            .unwrap_or(0),
                        etag: item.e_tag().map(ToString::to_string),
                        content_type: None,
                        last_modified: item.last_modified().map(|value| value.to_string()),
                    })
                })
                .collect();

            Ok(ListResult {
                objects,
                continuation_token: response.next_continuation_token().map(ToString::to_string),
                is_truncated: response.is_truncated().unwrap_or(false),
            })
        })
    }

    fn head(&self, key: &str) -> BoxFuture<'_, ObjectMetadata> {
        let key = key.to_string();
        Box::pin(async move {
            let resolved_key = self.resolve_key(&key)?;
            let response = self
                .client
                .head_object()
                .bucket(&self.bucket)
                .key(&resolved_key)
                .send()
                .await
                .map_err(|error| {
                    map_sdk_error(
                        ProviderKind::S3,
                        StorageOperation::Head,
                        Some(&resolved_key),
                        Some(&self.bucket),
                        error,
                    )
                })?;

            Ok(ObjectMetadata {
                path: key,
                size: response
                    .content_length()
                    .and_then(|value| u64::try_from(value).ok())
                    .unwrap_or(0),
                etag: response.e_tag().map(ToString::to_string),
                content_type: response.content_type().map(ToString::to_string),
                last_modified: response.last_modified().map(|value| value.to_string()),
                metadata: response
                    .metadata()
                    .map(|values| values.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                    .unwrap_or_default(),
            })
        })
    }

    fn get(&self, key: &str) -> BoxFuture<'_, BoxedByteStream> {
        let key = key.to_string();
        Box::pin(async move {
            let resolved_key = self.resolve_key(&key)?;
            let response = self
                .client
                .get_object()
                .bucket(&self.bucket)
                .key(&resolved_key)
                .send()
                .await
                .map_err(|error| {
                    map_sdk_error(
                        ProviderKind::S3,
                        StorageOperation::Get,
                        Some(&resolved_key),
                        Some(&self.bucket),
                        error,
                    )
                })?;

            Ok(Box::new(response.body.into_async_read()) as BoxedByteStream)
        })
    }

    fn get_range(&self, request: GetRangeRequest) -> BoxFuture<'_, BoxedByteStream> {
        Box::pin(async move {
            if request.length == 0 {
                return Err(StorageError::InvalidArgument {
                    operation: Some(StorageOperation::GetRange),
                    argument: "length".to_string(),
                    reason: "range length must be greater than zero".to_string(),
                });
            }

            let resolved_key = self.resolve_key(&request.key)?;
            let end = request
                .offset
                .checked_add(request.length - 1)
                .ok_or_else(|| StorageError::InvalidArgument {
                    operation: Some(StorageOperation::GetRange),
                    argument: "length".to_string(),
                    reason: "requested range overflows u64".to_string(),
                })?;
            let range = format!("bytes={}-{}", request.offset, end);

            let response = self
                .client
                .get_object()
                .bucket(&self.bucket)
                .key(&resolved_key)
                .range(range)
                .send()
                .await
                .map_err(|error| {
                    map_sdk_error(
                        ProviderKind::S3,
                        StorageOperation::GetRange,
                        Some(&resolved_key),
                        Some(&self.bucket),
                        error,
                    )
                })?;

            Ok(Box::new(response.body.into_async_read()) as BoxedByteStream)
        })
    }

    fn put(
        &self,
        key: &str,
        body: BoxedByteStream,
        options: PutOptions,
    ) -> BoxFuture<'_, PutResult> {
        let key = key.to_string();
        Box::pin(async move {
            let resolved_key = self.resolve_key(&key)?;
            let content_length =
                options
                    .content_length
                    .ok_or_else(|| StorageError::InvalidArgument {
                        operation: Some(StorageOperation::Put),
                        argument: "content_length".to_string(),
                        reason:
                            "S3 uploads require a known content length for streamed request bodies"
                                .to_string(),
                    })?;

            let response = self
                .client
                .put_object()
                .bucket(&self.bucket)
                .key(&resolved_key)
                .content_length(i64::try_from(content_length).map_err(|_| {
                    StorageError::InvalidArgument {
                        operation: Some(StorageOperation::Put),
                        argument: "content_length".to_string(),
                        reason: "content length exceeds S3 request limits".to_string(),
                    }
                })?)
                .set_content_type(options.content_type)
                .set_metadata(Some(options.metadata.into_iter().collect()))
                .body(byte_stream_from_reader(body, content_length))
                .send()
                .await
                .map_err(|error| {
                    map_sdk_error(
                        ProviderKind::S3,
                        StorageOperation::Put,
                        Some(&resolved_key),
                        Some(&self.bucket),
                        error,
                    )
                })?;

            Ok(PutResult {
                path: key,
                etag: response.e_tag().map(ToString::to_string),
                size: None,
            })
        })
    }

    fn delete(&self, key: &str) -> BoxFuture<'_, ()> {
        let key = key.to_string();
        Box::pin(async move {
            let resolved_key = self.resolve_key(&key)?;
            self.client
                .delete_object()
                .bucket(&self.bucket)
                .key(&resolved_key)
                .send()
                .await
                .map_err(|error| {
                    map_sdk_error(
                        ProviderKind::S3,
                        StorageOperation::Delete,
                        Some(&resolved_key),
                        Some(&self.bucket),
                        error,
                    )
                })?;
            Ok(())
        })
    }

    fn copy(&self, request: CopyRequest) -> BoxFuture<'_, CopyResult> {
        Box::pin(async move {
            let source = self.resolve_copy_location(&request.source, "source")?;
            let destination = self.resolve_copy_location(&request.destination, "destination")?;

            if source.bucket == destination.bucket {
                let source_header = encode_copy_source(&source.bucket, &source.key);
                let response = self
                    .client
                    .copy_object()
                    .bucket(&destination.bucket)
                    .key(&destination.key)
                    .copy_source(source_header)
                    .send()
                    .await
                    .map_err(|error| {
                        map_sdk_error(
                            ProviderKind::S3,
                            StorageOperation::Copy,
                            Some(&destination.key),
                            Some(&destination.bucket),
                            error,
                        )
                    })?;

                return Ok(CopyResult {
                    source: request.source,
                    destination: request.destination,
                    strategy: CopyStrategy::Native,
                    bytes_copied: None,
                    destination_etag: response
                        .copy_object_result()
                        .and_then(|value| value.e_tag().map(ToString::to_string)),
                });
            }

            self.relay_copy(&source, &destination).await
        })
    }
}

async fn load_sdk_config(config: &ProviderConfig) -> Result<aws_config::SdkConfig> {
    let mut loader = aws_config::defaults(BehaviorVersion::latest());
    if let Some(region) = &config.target.region {
        loader = loader.region(Region::new(region.clone()));
    }

    if let Some(endpoint) = &config.target.endpoint {
        loader = loader.endpoint_url(endpoint);
    }

    loader = match &config.credentials {
        CredentialSource::DefaultChain => loader,
        CredentialSource::Profile { name } => loader.profile_name(name),
        CredentialSource::InlineStatic { values } => {
            loader.credentials_provider(credentials_from_values(values)?)
        }
        CredentialSource::InlineEnvMap { values } => {
            loader.credentials_provider(credentials_from_values(values)?)
        }
        CredentialSource::Env { variables } => {
            loader.credentials_provider(credentials_from_env(variables)?)
        }
        CredentialSource::CredentialsFile { path } => {
            return Err(StorageError::InvalidArgument {
                operation: Some(StorageOperation::ConfigureProvider),
                argument: path.clone(),
                reason: "credential-file resolution is not implemented for the S3 provider yet"
                    .to_string(),
            })
        }
    };

    Ok(loader.load().await)
}

fn credentials_from_env(variables: &[String]) -> Result<Credentials> {
    let values = variables
        .iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| (key.clone(), value)))
        .collect::<BTreeMap<_, _>>();

    credentials_from_values(&values)
}

fn credentials_from_values(values: &BTreeMap<String, String>) -> Result<Credentials> {
    let access_key_id = find_first(
        values,
        &[
            "AWS_ACCESS_KEY_ID",
            "AWS_ACCESS_KEY",
            "access_key_id",
            "access_key",
        ],
    )?;
    let secret_access_key = find_first(
        values,
        &[
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SECRET_KEY",
            "secret_access_key",
            "secret_key",
        ],
    )?;
    let session_token = find_optional(values, &["AWS_SESSION_TOKEN", "session_token", "token"]);

    Ok(Credentials::new(
        access_key_id,
        secret_access_key,
        session_token,
        None,
        "storageprims-inline",
    ))
}

fn find_first(values: &BTreeMap<String, String>, candidates: &[&str]) -> Result<String> {
    candidates
        .iter()
        .find_map(|candidate| values.get(*candidate).cloned())
        .ok_or_else(|| StorageError::InvalidArgument {
            operation: Some(StorageOperation::ConfigureProvider),
            argument: candidates.join("|"),
            reason: "required AWS credential value is missing".to_string(),
        })
}

fn find_optional(values: &BTreeMap<String, String>, candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .find_map(|candidate| values.get(*candidate).cloned())
}

fn byte_stream_from_reader(reader: BoxedByteStream, content_length: u64) -> ByteStream {
    ByteStream::from_body_1_x(AsyncReadBody::new(reader, content_length))
}

fn encode_copy_source(bucket: &str, key: &str) -> String {
    format!(
        "{}/{}",
        utf8_percent_encode(bucket, NON_ALPHANUMERIC),
        utf8_percent_encode(key, NON_ALPHANUMERIC)
    )
}

fn map_sdk_error<E>(
    provider: ProviderKind,
    operation: StorageOperation,
    target: Option<&str>,
    container: Option<&str>,
    error: aws_sdk_s3::error::SdkError<E>,
) -> StorageError
where
    E: std::error::Error + ProvideErrorMetadata + Send + Sync + 'static,
{
    use aws_sdk_s3::error::SdkError;

    match error {
        SdkError::ServiceError(context) => {
            let err = context.into_err();
            let code = err.code().unwrap_or_default();
            match code {
                "NoSuchKey" | "NotFound" => StorageError::NotFound {
                    provider,
                    operation,
                    path: target.unwrap_or_default().to_string(),
                },
                "NoSuchBucket" => StorageError::ContainerNotFound {
                    provider,
                    operation,
                    container: container.unwrap_or_default().to_string(),
                },
                "AccessDenied" => StorageError::AccessDenied {
                    provider,
                    operation,
                    target: target.map(ToString::to_string),
                    detail: "access denied".to_string(),
                },
                "InvalidAccessKeyId" | "SignatureDoesNotMatch" | "ExpiredToken" => {
                    StorageError::InvalidCredentials {
                        provider,
                        detail: code.to_string(),
                    }
                }
                "SlowDown" | "Throttling" | "TooManyRequestsException" => StorageError::Throttled {
                    provider,
                    operation,
                    retry_after: None,
                },
                "PreconditionFailed" | "ConditionalRequestConflict" => StorageError::Conflict {
                    provider,
                    operation,
                    target: target.map(ToString::to_string),
                    detail: code.to_string(),
                },
                "InvalidRequest" | "InvalidWriteOffset" => StorageError::InvalidArgument {
                    operation: Some(operation),
                    argument: target.unwrap_or_default().to_string(),
                    reason: code.to_string(),
                },
                _ => StorageError::Other {
                    provider: Some(provider),
                    operation: Some(operation),
                    detail: format!("{}", DisplayErrorContext(&err)),
                    source: None,
                },
            }
        }
        SdkError::TimeoutError(_) | SdkError::DispatchFailure(_) => {
            StorageError::ProviderUnavailable {
                provider,
                operation,
                detail: format!("{}", DisplayErrorContext(&error)),
            }
        }
        SdkError::ResponseError(_) => StorageError::ProviderUnavailable {
            provider,
            operation,
            detail: format!("{}", DisplayErrorContext(&error)),
        },
        SdkError::ConstructionFailure(construction_error) => StorageError::InvalidArgument {
            operation: Some(operation),
            argument: target.unwrap_or_default().to_string(),
            reason: format!("{construction_error:?}"),
        },
        other => {
            let message = format!("{}", DisplayErrorContext(&other));
            if message.contains("403") {
                StorageError::AccessDenied {
                    provider,
                    operation,
                    target: target.map(ToString::to_string),
                    detail: "access denied".to_string(),
                }
            } else if message.contains("429") || message.contains("SlowDown") {
                StorageError::Throttled {
                    provider,
                    operation,
                    retry_after: parse_retry_after(&message),
                }
            } else {
                StorageError::Other {
                    provider: Some(provider),
                    operation: Some(operation),
                    detail: message,
                    source: None,
                }
            }
        }
    }
}

fn parse_retry_after(message: &str) -> Option<Duration> {
    let marker = "retry-after:";
    let lower = message.to_ascii_lowercase();
    lower.find(marker).and_then(|offset| {
        lower[offset + marker.len()..]
            .split(|c: char| !c.is_ascii_digit())
            .find(|part| !part.is_empty())
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_secs)
    })
}

fn normalize_prefix(prefix: Option<String>) -> Option<String> {
    prefix
        .map(|value| value.trim_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

fn join_key(root_prefix: Option<&str>, key: &str) -> String {
    let key = key.trim_start_matches('/');
    match root_prefix {
        Some(root) if !root.is_empty() && !key.is_empty() => format!("{root}/{key}"),
        Some(root) if !root.is_empty() => root.to_string(),
        _ => key.to_string(),
    }
}

impl AsyncReadBody {
    fn new(reader: BoxedByteStream, content_length: u64) -> Self {
        Self {
            reader: Mutex::new(reader),
            finished: AtomicBool::new(false),
            remaining: Mutex::new(content_length),
        }
    }
}

impl http_body::Body for AsyncReadBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<std::result::Result<Frame<Self::Data>, Self::Error>>> {
        if self.finished.load(Ordering::Acquire) {
            return Poll::Ready(None);
        }

        let mut reader = self.reader.lock().expect("async read body mutex poisoned");
        let mut remaining = self
            .remaining
            .lock()
            .expect("async read body mutex poisoned");
        let read_limit = if *remaining == 0 {
            1
        } else {
            (*remaining).min(8 * 1024) as usize
        };
        let mut buffer = vec![0_u8; read_limit];
        let mut read_buf = ReadBuf::new(&mut buffer);

        match Pin::new(&mut *reader).poll_read(cx, &mut read_buf) {
            Poll::Ready(Ok(())) => {
                let filled = read_buf.filled();
                if filled.is_empty() {
                    self.finished.store(true, Ordering::Release);
                    if *remaining == 0 {
                        return Poll::Ready(None);
                    }
                    return Poll::Ready(Some(Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        format!(
                            "stream ended before declared content length; {} bytes remaining",
                            *remaining
                        ),
                    ))));
                }

                if *remaining == 0 {
                    self.finished.store(true, Ordering::Release);
                    return Poll::Ready(Some(Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "stream exceeded declared content length",
                    ))));
                }

                *remaining -= filled.len() as u64;
                Poll::Ready(Some(Ok(Frame::data(Bytes::copy_from_slice(filled)))))
            }
            Poll::Ready(Err(error)) => {
                self.finished.store(true, Ordering::Release);
                Poll::Ready(Some(Err(error)))
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    fn size_hint(&self) -> SizeHint {
        let remaining = *self
            .remaining
            .lock()
            .expect("async read body mutex poisoned");
        SizeHint::with_exact(remaining)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;
    use std::io::{Cursor, ErrorKind};

    #[test]
    fn join_key_respects_root_prefix() {
        assert_eq!(
            join_key(Some("nested/root"), "file.txt"),
            "nested/root/file.txt"
        );
        assert_eq!(
            join_key(Some("nested/root"), "/file.txt"),
            "nested/root/file.txt"
        );
        assert_eq!(join_key(None, "file.txt"), "file.txt");
    }

    #[test]
    fn resolve_copy_location_supports_absolute_and_relative_keys() {
        let provider = S3Provider {
            client: Client::from_conf(
                aws_sdk_s3::config::Builder::new()
                    .behavior_version_latest()
                    .region(Region::new("us-east-1"))
                    .credentials_provider(Credentials::from_keys("test", "test", None))
                    .build(),
            ),
            bucket: "bucket-a".to_string(),
            root_prefix: Some("root".to_string()),
        };

        let relative = provider
            .resolve_copy_location("child/file.txt", "source")
            .unwrap();
        assert_eq!(
            relative,
            S3Location {
                bucket: "bucket-a".to_string(),
                key: "root/child/file.txt".to_string(),
            }
        );

        let absolute = provider
            .resolve_copy_location("s3://bucket-b/path/to/file.txt", "destination")
            .unwrap();
        assert_eq!(
            absolute,
            S3Location {
                bucket: "bucket-b".to_string(),
                key: "path/to/file.txt".to_string(),
            }
        );
    }

    #[test]
    fn inline_credentials_accept_multiple_key_names() {
        let mut values = BTreeMap::new();
        values.insert("access_key".to_string(), "abc".to_string());
        values.insert("secret_key".to_string(), "def".to_string());
        values.insert("token".to_string(), "ghi".to_string());

        let credentials = credentials_from_values(&values).unwrap();
        assert_eq!(credentials.access_key_id(), "abc");
        assert_eq!(credentials.secret_access_key(), "def");
        assert_eq!(credentials.session_token(), Some("ghi"));
    }

    #[test]
    fn normalize_prefix_removes_empty_and_slashes() {
        assert_eq!(
            normalize_prefix(Some("/root/path/".to_string())),
            Some("root/path".to_string())
        );
        assert_eq!(normalize_prefix(Some("/".to_string())), None);
        assert_eq!(normalize_prefix(None), None);
    }

    #[test]
    fn encode_copy_source_escapes_reserved_key_characters() {
        assert_eq!(
            encode_copy_source("bucket-a", "path with spaces/a+b#c.txt"),
            "bucket%2Da/path%20with%20spaces%2Fa%2Bb%23c%2Etxt"
        );
    }

    #[tokio::test]
    async fn async_read_body_errors_when_stream_is_shorter_than_declared() {
        let error = byte_stream_from_reader(Box::new(Cursor::new(b"abc".to_vec())), 5)
            .collect()
            .await
            .expect_err("short stream should fail");
        let source = error.source().expect("collect error should expose source");
        let io_error = source
            .downcast_ref::<std::io::Error>()
            .expect("source should be an io error");
        assert_eq!(io_error.kind(), ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn async_read_body_errors_when_stream_is_longer_than_declared() {
        let error = byte_stream_from_reader(Box::new(Cursor::new(b"abc".to_vec())), 2)
            .collect()
            .await
            .expect_err("long stream should fail");
        let source = error.source().expect("collect error should expose source");
        let io_error = source
            .downcast_ref::<std::io::Error>()
            .expect("source should be an io error");
        assert_eq!(io_error.kind(), ErrorKind::InvalidData);
    }
}
