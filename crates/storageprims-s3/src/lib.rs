//! AWS S3 provider implementation for storageprims.

use std::collections::BTreeMap;
use std::fmt;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use aws_config::{BehaviorVersion, Region};
use aws_credential_types::Credentials;
use aws_sdk_s3::error::{DisplayErrorContext, ProvideErrorMetadata};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use bytes::Bytes;
use http_body::{Frame, SizeHint};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use storageprims_core::{
    sanitize_endpoint, BoxFuture, BoxedByteStream, Capability, ConflictKind, CopyRequest,
    CopyResult, CopyStrategy, CredentialSource, CredentialSourceKind, GetRangeRequest, ListOptions,
    ListResult, ObjectMetadata, ObjectSummary, ProbeResult, ProbeScope, ProviderConfig,
    ProviderKind, PutOptions, PutPrecondition, PutResult, Result, StorageError, StorageOperation,
    StorageProvider, StorageUri,
};
use tokio::io::{AsyncRead, ReadBuf};

#[derive(Clone)]
pub struct S3Provider {
    client: Client,
    bucket: String,
    root_prefix: Option<String>,
    endpoint: Option<String>,
    credential_source: CredentialSourceKind,
}

impl fmt::Debug for S3Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3Provider")
            .field("bucket", &self.bucket)
            .field("root_prefix", &self.root_prefix)
            .field("endpoint", &self.endpoint.as_deref().map(sanitize_endpoint))
            .field("credential_source", &self.credential_source)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct S3Location {
    bucket: String,
    key: String,
}

#[derive(Clone, Copy)]
enum PutPreconditionKind {
    None,
    MustNotExist,
    Match,
}

struct AsyncReadBody {
    reader: Mutex<BoxedByteStream>,
    finished: bool,
    remaining: u64,
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

        let root_prefix = resolve_root_prefix(&uri.path, config.target.root_prefix.as_deref())?;

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
        validate_credential_source(&config.credentials)?;
        let sdk_config = load_sdk_config(&config).await?;

        let mut service_config = aws_sdk_s3::config::Builder::from(&sdk_config);
        service_config.set_force_path_style(config.target.force_path_style);
        let client = Client::from_conf(service_config.build());

        Ok(Self {
            client,
            bucket,
            root_prefix,
            endpoint: config.target.endpoint.clone(),
            credential_source: CredentialSourceKind::from_config(&config.credentials),
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
        vec![Capability::CredentialProbe, Capability::ConditionalPut]
    }

    fn list(&self, options: ListOptions) -> BoxFuture<'_, ListResult> {
        Box::pin(async move {
            let prefix =
                resolve_list_prefix(self.root_prefix.as_deref(), options.prefix.as_deref())?;
            let max_keys = normalize_max_keys(options.max_keys)?;
            let response = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .set_prefix(prefix.clone())
                .set_continuation_token(options.continuation_token)
                .set_max_keys(max_keys)
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

            validate_list_page(
                self.root_prefix.as_deref(),
                response.contents().iter().map(|item| item.key()),
            )?;

            let objects = response
                .contents()
                .iter()
                .filter_map(|item| {
                    item.key().map(|key| ObjectSummary {
                        path: strip_root_prefix(self.root_prefix.as_deref(), key)
                            .expect("list page was validated before projection")
                            .to_string(),
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
            let PutOptions {
                content_length,
                content_type,
                metadata,
                precondition,
            } = options;
            let content_length = content_length.ok_or_else(|| StorageError::InvalidArgument {
                operation: Some(StorageOperation::Put),
                argument: "content_length".to_string(),
                reason: "S3 uploads require a known content length for streamed request bodies"
                    .to_string(),
            })?;
            let (if_match, if_none_match, precondition_kind) = match precondition {
                PutPrecondition::None => (None, None, PutPreconditionKind::None),
                PutPrecondition::MustNotExist => (
                    None,
                    Some("*".to_string()),
                    PutPreconditionKind::MustNotExist,
                ),
                PutPrecondition::Match { token } => {
                    validate_match_token(&token)?;
                    (Some(token), None, PutPreconditionKind::Match)
                }
            };

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
                .set_content_type(content_type)
                .set_metadata(Some(metadata.into_iter().collect()))
                .set_if_match(if_match)
                .set_if_none_match(if_none_match)
                .body(byte_stream_from_reader(body, content_length))
                .send()
                .await
                .map_err(|error| {
                    map_put_error(
                        Some(&resolved_key),
                        Some(&self.bucket),
                        precondition_kind,
                        error,
                    )
                })?;

            Ok(PutResult {
                path: key,
                etag: response.e_tag().map(ToString::to_string),
                size: Some(content_length),
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

    fn probe(&self) -> BoxFuture<'_, ProbeResult> {
        Box::pin(async move {
            let started = Instant::now();
            self.probe_head_bucket(started).await
        })
    }
}

impl S3Provider {
    async fn probe_head_bucket(&self, started: Instant) -> Result<ProbeResult> {
        self.client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .map_err(|error| {
                map_sdk_error(
                    ProviderKind::S3,
                    StorageOperation::Probe,
                    None,
                    Some(&self.bucket),
                    error,
                )
            })?;

        Ok(self.probe_result("s3:HeadBucket", started.elapsed()))
    }

    fn probe_result(&self, method: &str, elapsed: Duration) -> ProbeResult {
        ProbeResult {
            provider: ProviderKind::S3,
            endpoint: self.endpoint.as_deref().map(sanitize_endpoint),
            credential_source: self.credential_source.clone(),
            scope: ProbeScope::ConfiguredContainer,
            probe_method: method.to_string(),
            latency_ms: elapsed.as_millis().try_into().unwrap_or(u64::MAX),
            capabilities: self.capabilities(),
        }
    }
}

fn validate_credential_source(credentials: &CredentialSource) -> Result<()> {
    if matches!(credentials, CredentialSource::CredentialsFile { .. }) {
        return Err(StorageError::InvalidArgument {
            operation: Some(StorageOperation::ConfigureProvider),
            argument: "credentials.mode".to_string(),
            reason: "credential-file mode is not supported by the S3 provider".to_string(),
        });
    }
    Ok(())
}

fn resolve_root_prefix(uri_path: &str, configured_root: Option<&str>) -> Result<Option<String>> {
    let uri_root = normalize_prefix(Some(uri_path.to_string()));
    let configured_root = normalize_prefix(configured_root.map(ToString::to_string));

    match (uri_root, configured_root) {
        (Some(_), Some(_)) => Err(StorageError::InvalidArgument {
            operation: Some(StorageOperation::ConfigureProvider),
            argument: "target.root_prefix".to_string(),
            reason: "URI path and configured root prefix cannot both be set".to_string(),
        }),
        (uri_root, configured_root) => Ok(configured_root.or(uri_root)),
    }
}

fn normalize_max_keys(max_keys: Option<u32>) -> Result<Option<i32>> {
    match max_keys {
        None | Some(0) => Ok(None),
        Some(value) => i32::try_from(value)
            .map(Some)
            .map_err(|_| StorageError::InvalidArgument {
                operation: Some(StorageOperation::List),
                argument: "max_keys".to_string(),
                reason: "value exceeds the S3 request limit".to_string(),
            }),
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
        CredentialSource::CredentialsFile { .. } => {
            unreachable!("credential-file mode is rejected before SDK configuration")
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

fn validate_match_token(token: &str) -> Result<()> {
    let entity_tag = token.strip_prefix("W/").unwrap_or(token);
    let valid = entity_tag.len() >= 2
        && entity_tag.starts_with('"')
        && entity_tag.ends_with('"')
        && entity_tag.as_bytes()[1..entity_tag.len() - 1]
            .iter()
            .all(|byte| *byte == 0x21 || (0x23..=0x7e).contains(byte));

    if valid {
        Ok(())
    } else {
        Err(StorageError::InvalidArgument {
            operation: Some(StorageOperation::Put),
            argument: "precondition.match".to_string(),
            reason: "match token is not a valid S3 entity tag".to_string(),
        })
    }
}

fn map_put_error<E>(
    target: Option<&str>,
    container: Option<&str>,
    precondition: PutPreconditionKind,
    error: aws_sdk_s3::error::SdkError<E>,
) -> StorageError
where
    E: std::error::Error + ProvideErrorMetadata + Send + Sync + 'static,
{
    use aws_sdk_s3::error::SdkError;

    match error {
        SdkError::ServiceError(context) => {
            let retry_after = context
                .raw()
                .headers()
                .get("retry-after")
                .and_then(parse_retry_after_value);
            let code = context.err().code().unwrap_or_default();
            match code {
                "PreconditionFailed" => {
                    let kind = match precondition {
                        PutPreconditionKind::MustNotExist => ConflictKind::AlreadyExists,
                        PutPreconditionKind::Match => ConflictKind::TokenMismatch,
                        PutPreconditionKind::None => ConflictKind::Other,
                    };
                    StorageError::Conflict {
                        provider: ProviderKind::S3,
                        operation: StorageOperation::Put,
                        target: target.map(ToString::to_string),
                        kind,
                        detail: "precondition failed".to_string(),
                    }
                }
                "ConditionalRequestConflict" => StorageError::Conflict {
                    provider: ProviderKind::S3,
                    operation: StorageOperation::Put,
                    target: target.map(ToString::to_string),
                    kind: ConflictKind::Other,
                    detail: "conditional request conflict".to_string(),
                },
                "NoSuchKey" | "NotFound" => StorageError::NotFound {
                    provider: ProviderKind::S3,
                    operation: StorageOperation::Put,
                    path: target.unwrap_or_default().to_string(),
                },
                "NoSuchBucket" => StorageError::ContainerNotFound {
                    provider: ProviderKind::S3,
                    operation: StorageOperation::Put,
                    container: container.unwrap_or_default().to_string(),
                },
                "AccessDenied" => StorageError::AccessDenied {
                    provider: ProviderKind::S3,
                    operation: StorageOperation::Put,
                    target: target.map(ToString::to_string),
                    detail: "access denied".to_string(),
                },
                "InvalidAccessKeyId" | "SignatureDoesNotMatch" | "ExpiredToken" => {
                    StorageError::InvalidCredentials {
                        provider: ProviderKind::S3,
                        detail: code.to_string(),
                    }
                }
                "SlowDown" | "Throttling" | "TooManyRequestsException" => StorageError::Throttled {
                    provider: ProviderKind::S3,
                    operation: StorageOperation::Put,
                    retry_after,
                },
                "InvalidRequest" | "InvalidWriteOffset" => StorageError::InvalidArgument {
                    operation: Some(StorageOperation::Put),
                    argument: target.unwrap_or_default().to_string(),
                    reason: code.to_string(),
                },
                _ => StorageError::Other {
                    provider: Some(ProviderKind::S3),
                    operation: Some(StorageOperation::Put),
                    detail: if code.is_empty() {
                        "S3 put failed".to_string()
                    } else {
                        code.to_string()
                    },
                    source: None,
                },
            }
        }
        SdkError::TimeoutError(_) | SdkError::DispatchFailure(_) | SdkError::ResponseError(_) => {
            StorageError::ProviderUnavailable {
                provider: ProviderKind::S3,
                operation: StorageOperation::Put,
                detail: "S3 put request outcome is unknown".to_string(),
            }
        }
        SdkError::ConstructionFailure(_) => StorageError::InvalidArgument {
            operation: Some(StorageOperation::Put),
            argument: target.unwrap_or_default().to_string(),
            reason: "failed to construct S3 put request".to_string(),
        },
        _ => StorageError::Other {
            provider: Some(ProviderKind::S3),
            operation: Some(StorageOperation::Put),
            detail: "S3 put failed".to_string(),
            source: None,
        },
    }
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
            let retry_after = context
                .raw()
                .headers()
                .get("retry-after")
                .and_then(parse_retry_after_value);
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
                    retry_after,
                },
                "PreconditionFailed" | "ConditionalRequestConflict" => StorageError::Conflict {
                    provider,
                    operation,
                    target: target.map(ToString::to_string),
                    kind: ConflictKind::Other,
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
    lower
        .find(marker)
        .and_then(|offset| parse_retry_after_value(&lower[offset + marker.len()..]))
}

fn parse_retry_after_value(value: &str) -> Option<Duration> {
    value
        .split(|c: char| !c.is_ascii_digit())
        .find(|part| !part.is_empty())
        .and_then(|seconds| seconds.parse::<u64>().ok())
        .map(Duration::from_secs)
}

fn normalize_prefix(prefix: Option<String>) -> Option<String> {
    prefix
        .map(|value| value.trim_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

fn list_root_boundary(root_prefix: Option<&str>) -> Option<String> {
    root_prefix
        .filter(|root| !root.is_empty())
        .map(|root| format!("{root}/"))
}

fn resolve_list_prefix(
    root_prefix: Option<&str>,
    caller_prefix: Option<&str>,
) -> Result<Option<String>> {
    if let Some(prefix) = caller_prefix {
        if prefix.starts_with('/') || starts_with_uri_scheme(prefix) {
            return Err(StorageError::InvalidArgument {
                operation: Some(StorageOperation::List),
                argument: "prefix".to_string(),
                reason: "list prefix must be provider-relative".to_string(),
            });
        }
    }

    let caller_prefix = caller_prefix.filter(|prefix| !prefix.is_empty());
    match (list_root_boundary(root_prefix), caller_prefix) {
        (Some(boundary), Some(prefix)) => Ok(Some(format!("{boundary}{prefix}"))),
        (Some(boundary), None) => Ok(Some(boundary)),
        (None, Some(prefix)) => Ok(Some(prefix.to_string())),
        (None, None) => Ok(None),
    }
}

fn starts_with_uri_scheme(value: &str) -> bool {
    let Some((scheme, _)) = value.split_once("://") else {
        return false;
    };
    let mut chars = scheme.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && chars.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        })
}

fn strip_root_prefix<'a>(root_prefix: Option<&str>, key: &'a str) -> Result<&'a str> {
    let Some(boundary) = list_root_boundary(root_prefix) else {
        return Ok(key);
    };

    key.strip_prefix(&boundary)
        .ok_or_else(|| StorageError::Other {
            provider: Some(ProviderKind::S3),
            operation: Some(StorageOperation::List),
            detail: "S3 returned an object outside the configured list boundary".to_string(),
            source: None,
        })
}

fn validate_list_page<'a>(
    root_prefix: Option<&str>,
    keys: impl IntoIterator<Item = Option<&'a str>>,
) -> Result<()> {
    if root_prefix.is_none() {
        return Ok(());
    }

    for key in keys {
        let key = key.ok_or_else(|| StorageError::Other {
            provider: Some(ProviderKind::S3),
            operation: Some(StorageOperation::List),
            detail: "S3 returned a list entry without an object key".to_string(),
            source: None,
        })?;
        strip_root_prefix(root_prefix, key)?;
    }
    Ok(())
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
            finished: false,
            remaining: content_length,
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
        let this = self.get_mut();

        if this.finished {
            return Poll::Ready(None);
        }

        let mut reader = this.reader.lock().expect("async read body mutex poisoned");

        if this.remaining == 0 {
            let mut probe = [0_u8; 1];
            let mut read_buf = ReadBuf::new(&mut probe);

            return match Pin::new(&mut *reader).poll_read(cx, &mut read_buf) {
                Poll::Ready(Ok(())) => {
                    this.finished = true;
                    if read_buf.filled().is_empty() {
                        Poll::Ready(None)
                    } else {
                        Poll::Ready(Some(Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "stream exceeded declared content length",
                        ))))
                    }
                }
                Poll::Ready(Err(error)) => {
                    this.finished = true;
                    Poll::Ready(Some(Err(error)))
                }
                Poll::Pending => Poll::Pending,
            };
        }

        let read_limit = this.remaining.min(8 * 1024) as usize;
        let mut buffer = vec![0_u8; read_limit];
        let mut read_buf = ReadBuf::new(&mut buffer);

        match Pin::new(&mut *reader).poll_read(cx, &mut read_buf) {
            Poll::Ready(Ok(())) => {
                let filled = read_buf.filled();
                if filled.is_empty() {
                    this.finished = true;
                    return Poll::Ready(Some(Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        format!(
                            "stream ended before declared content length; {} bytes remaining",
                            this.remaining
                        ),
                    ))));
                }

                this.remaining -= filled.len() as u64;
                Poll::Ready(Some(Ok(Frame::data(Bytes::copy_from_slice(filled)))))
            }
            Poll::Ready(Err(error)) => {
                this.finished = true;
                Poll::Ready(Some(Err(error)))
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.finished
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.remaining)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_s3::operation::{
        get_object::GetObjectError, head_object::HeadObjectError, put_object::PutObjectError,
    };
    use aws_smithy_runtime_api::http::{Response, StatusCode};
    use aws_smithy_types::body::SdkBody;
    use aws_smithy_types::error::ErrorMetadata;
    use std::error::Error;
    use std::io::{Cursor, ErrorKind};
    use std::sync::{Mutex as StdMutex, MutexGuard, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    use aws_credential_types::provider::ProvideCredentials;
    use storageprims_core::TargetConfig;

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
            endpoint: None,
            credential_source: CredentialSourceKind::DefaultChain,
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
    fn root_prefix_sources_are_normalized_before_ambiguity_check() {
        for (uri_path, configured_root) in [
            ("team", Some("team/")),
            ("team%2Farchive", Some("team/archive")),
            ("team", Some("/archive/")),
        ] {
            let error = resolve_root_prefix(uri_path, configured_root)
                .expect_err("two nonempty root sources must be rejected");
            let display = error.to_string();
            let debug = format!("{error:?}");
            assert!(matches!(
                error,
                StorageError::InvalidArgument {
                    operation: Some(StorageOperation::ConfigureProvider),
                    ref argument,
                    ..
                } if argument == "target.root_prefix"
            ));
            assert!(!display.contains(uri_path));
            assert!(!debug.contains(uri_path));
            if let Some(configured_root) = configured_root {
                assert!(!display.contains(configured_root));
                assert!(!debug.contains(configured_root));
            }
        }

        assert_eq!(
            resolve_root_prefix("team", Some("/")).unwrap(),
            Some("team".to_string())
        );
        assert_eq!(
            resolve_root_prefix("", Some("/team/")).unwrap(),
            Some("team".to_string())
        );
        assert_eq!(resolve_root_prefix("", Some("/")).unwrap(), None);
    }

    #[tokio::test]
    async fn from_uri_accepts_exactly_one_normalized_root_source() {
        for (uri_text, configured_root) in [
            ("s3://bucket/team", "team/"),
            ("s3://bucket/team%2Farchive", "team/archive"),
        ] {
            let uri = StorageUri::parse(uri_text).unwrap();
            let error = S3Provider::from_uri(
                &uri,
                ProviderConfig {
                    provider: ProviderKind::S3,
                    target: TargetConfig {
                        root_prefix: Some(configured_root.to_string()),
                        region: Some("us-east-1".to_string()),
                        ..TargetConfig::default()
                    },
                    credentials: CredentialSource::InlineStatic {
                        values: test_credentials(),
                    },
                },
            )
            .await
            .expect_err("two normalized root sources must fail before SDK setup");
            assert!(matches!(
                error,
                StorageError::InvalidArgument {
                    operation: Some(StorageOperation::ConfigureProvider),
                    ref argument,
                    ..
                } if argument == "target.root_prefix"
            ));
            assert!(!error.to_string().contains(uri_text));
            assert!(!error.to_string().contains(configured_root));
        }

        let uri = StorageUri::parse("s3://bucket/from-uri/").unwrap();
        let provider = S3Provider::from_uri(
            &uri,
            ProviderConfig {
                provider: ProviderKind::S3,
                target: TargetConfig {
                    root_prefix: Some("/".to_string()),
                    region: Some("us-east-1".to_string()),
                    ..TargetConfig::default()
                },
                credentials: CredentialSource::InlineStatic {
                    values: test_credentials(),
                },
            },
        )
        .await
        .expect("an empty configured root leaves the URI root unambiguous");
        assert_eq!(provider.root_prefix(), Some("from-uri"));

        let uri = StorageUri::parse("s3://bucket/").unwrap();
        let provider = S3Provider::from_uri(
            &uri,
            ProviderConfig {
                provider: ProviderKind::S3,
                target: TargetConfig {
                    root_prefix: Some("/from-config/".to_string()),
                    region: Some("us-east-1".to_string()),
                    ..TargetConfig::default()
                },
                credentials: CredentialSource::InlineStatic {
                    values: test_credentials(),
                },
            },
        )
        .await
        .expect("an empty URI root leaves the configured root unambiguous");
        assert_eq!(provider.root_prefix(), Some("from-config"));
    }

    #[tokio::test]
    async fn credentials_file_is_rejected_at_construction_without_disclosure() {
        let sentinel = "/credential-file-path-sentinel";
        let config = ProviderConfig {
            provider: ProviderKind::S3,
            target: TargetConfig {
                container: Some("bucket".to_string()),
                region: Some("us-east-1".to_string()),
                endpoint: Some("http://127.0.0.1:9".to_string()),
                ..TargetConfig::default()
            },
            credentials: CredentialSource::CredentialsFile {
                path: sentinel.to_string(),
            },
        };
        assert!(!format!("{config:?}").contains(sentinel));

        let error = S3Provider::from_config(config)
            .await
            .expect_err("credential-file mode must fail during construction");
        let display = error.to_string();
        let debug = format!("{error:?}");
        assert!(matches!(
            error,
            StorageError::InvalidArgument {
                operation: Some(StorageOperation::ConfigureProvider),
                ref argument,
                ..
            } if argument == "credentials.mode"
        ));
        assert!(!display.contains(sentinel));
        assert!(!debug.contains(sentinel));
    }

    #[test]
    fn list_prefix_uses_exact_root_segment_boundary() {
        assert_eq!(
            resolve_list_prefix(Some("team"), None).unwrap(),
            Some("team/".to_string())
        );
        assert_eq!(
            resolve_list_prefix(Some("team"), Some("docs/")).unwrap(),
            Some("team/docs/".to_string())
        );
        assert_eq!(
            resolve_list_prefix(Some("team/a"), None).unwrap(),
            Some("team/a/".to_string())
        );
        assert_eq!(
            resolve_list_prefix(None, Some("team")).unwrap(),
            Some("team".to_string())
        );
        assert_eq!(resolve_list_prefix(None, None).unwrap(), None);
        let normalized_root = normalize_prefix(Some("team/".to_string()));
        assert_eq!(
            resolve_list_prefix(normalized_root.as_deref(), None).unwrap(),
            Some("team/".to_string())
        );
    }

    #[test]
    fn list_prefix_rejects_non_relative_values_but_allows_dot_dot() {
        for root in [Some("team"), None] {
            for prefix in ["/other", "s3://bucket/key", "https://example.com/key"] {
                assert!(matches!(
                    resolve_list_prefix(root, Some(prefix)),
                    Err(StorageError::InvalidArgument {
                        operation: Some(StorageOperation::List),
                        argument,
                        ..
                    }) if argument == "prefix"
                ));
            }
        }

        assert_eq!(
            resolve_list_prefix(Some("team"), Some("../other")).unwrap(),
            Some("team/../other".to_string())
        );
        assert_eq!(
            resolve_list_prefix(Some("team"), Some("docs/http://archive/")).unwrap(),
            Some("team/docs/http://archive/".to_string())
        );
        assert_eq!(
            resolve_list_prefix(None, Some("docs/http://archive/")).unwrap(),
            Some("docs/http://archive/".to_string())
        );
    }

    #[test]
    fn max_keys_uses_provider_default_for_zero_and_rejects_overflow() {
        assert_eq!(normalize_max_keys(None).unwrap(), None);
        assert_eq!(normalize_max_keys(Some(0)).unwrap(), None);
        assert_eq!(normalize_max_keys(Some(1)).unwrap(), Some(1));
        assert_eq!(
            normalize_max_keys(Some(i32::MAX as u32)).unwrap(),
            Some(i32::MAX)
        );

        let error = normalize_max_keys(Some(u32::MAX))
            .expect_err("values outside S3's representation must fail");
        assert!(matches!(
            error,
            StorageError::InvalidArgument {
                operation: Some(StorageOperation::List),
                ref argument,
                ..
            } if argument == "max_keys"
        ));
        assert!(!error.to_string().contains(&u32::MAX.to_string()));
    }

    #[test]
    fn s3_advertises_only_callable_capabilities() {
        let provider = S3Provider {
            client: Client::from_conf(
                aws_sdk_s3::config::Builder::new()
                    .behavior_version_latest()
                    .region(Region::new("us-east-1"))
                    .credentials_provider(Credentials::from_keys("test", "test", None))
                    .build(),
            ),
            bucket: "bucket-a".to_string(),
            root_prefix: None,
            endpoint: None,
            credential_source: CredentialSourceKind::InlineStatic,
        };

        assert_eq!(
            provider.capabilities(),
            vec![Capability::CredentialProbe, Capability::ConditionalPut]
        );
    }

    #[test]
    fn rooted_list_page_fails_closed_on_any_out_of_root_key() {
        let error = validate_list_page(Some("team"), [Some("team/x"), Some("team2/x")])
            .expect_err("mixed page must fail");
        let rendered = error.to_string();

        assert!(matches!(
            error,
            StorageError::Other {
                provider: Some(ProviderKind::S3),
                operation: Some(StorageOperation::List),
                ..
            }
        ));
        assert!(!rendered.contains("team2/x"));
        assert!(!rendered.contains("team/"));
    }

    #[test]
    fn rooted_list_page_rejects_key_equal_to_bare_root() {
        assert!(validate_list_page(Some("team"), [Some("team")]).is_err());
        assert_eq!(strip_root_prefix(Some("team"), "team/x").unwrap(), "x");
    }

    #[test]
    fn nested_rooted_list_page_rejects_sibling_segment_collision() {
        validate_list_page(Some("team/a"), [Some("team/a/x")]).unwrap();
        let error = validate_list_page(Some("team/a"), [Some("team/a/x"), Some("team/a2/x")])
            .expect_err("nested sibling collision must fail");
        let rendered = error.to_string();

        assert!(!rendered.contains("team/a2/x"));
        assert!(!rendered.contains("team/a/"));
    }

    #[test]
    fn unrooted_list_page_preserves_all_keys() {
        validate_list_page(None, [Some("team/x"), Some("team2/x")]).unwrap();
        assert_eq!(strip_root_prefix(None, "team2/x").unwrap(), "team2/x");
    }

    #[test]
    fn encode_copy_source_escapes_reserved_key_characters() {
        assert_eq!(
            encode_copy_source("bucket-a", "path with spaces/a+b#c.txt"),
            "bucket%2Da/path%20with%20spaces%2Fa%2Bb%23c%2Etxt"
        );
    }

    #[test]
    fn match_token_validation_accepts_opaque_entity_tags_byte_for_byte() {
        for token in [
            "\"opaque\"",
            "W/\"weak-form\"",
            "\"!#$%&'()*+,-./:;<=>?@[]^_`{|}~\"",
        ] {
            validate_match_token(token).expect("valid entity tag should be accepted");
        }
    }

    #[test]
    fn match_token_validation_rejects_invalid_values_without_disclosure() {
        for token in [
            "",
            "unquoted",
            "\"line\nbreak\"",
            "\"nul\0byte\"",
            "\"delete\u{7f}\"",
            "\"non-ascii-\u{e9}\"",
        ] {
            let error = validate_match_token(token).expect_err("invalid entity tag should fail");
            let display = error.to_string();
            let debug = format!("{error:?}");
            assert!(matches!(
                error,
                StorageError::InvalidArgument {
                    operation: Some(StorageOperation::Put),
                    ref argument,
                    ..
                } if argument == "precondition.match"
            ));
            if !token.is_empty() {
                assert!(!display.contains(token));
                assert!(!debug.contains(token));
            }
        }
    }

    #[test]
    fn put_precondition_failure_maps_from_request_intent() {
        for (precondition, expected_kind) in [
            (
                PutPreconditionKind::MustNotExist,
                ConflictKind::AlreadyExists,
            ),
            (PutPreconditionKind::Match, ConflictKind::TokenMismatch),
            (PutPreconditionKind::None, ConflictKind::Other),
        ] {
            let error = map_put_error(
                Some("key"),
                Some("bucket"),
                precondition,
                aws_sdk_s3::error::SdkError::service_error(
                    PutObjectError::generic(
                        ErrorMetadata::builder().code("PreconditionFailed").build(),
                    ),
                    Response::new(
                        StatusCode::try_from(412).expect("valid status"),
                        SdkBody::empty(),
                    ),
                ),
            );

            assert!(matches!(
                error,
                StorageError::Conflict {
                    provider: ProviderKind::S3,
                    operation: StorageOperation::Put,
                    kind,
                    ref detail,
                    ..
                } if kind == expected_kind && detail == "precondition failed"
            ));
        }
    }

    #[test]
    fn conditional_request_conflict_stays_other() {
        let error = map_put_error(
            Some("key"),
            Some("bucket"),
            PutPreconditionKind::MustNotExist,
            aws_sdk_s3::error::SdkError::service_error(
                PutObjectError::generic(
                    ErrorMetadata::builder()
                        .code("ConditionalRequestConflict")
                        .build(),
                ),
                Response::new(
                    StatusCode::try_from(409).expect("valid status"),
                    SdkBody::empty(),
                ),
            ),
        );

        assert!(matches!(
            error,
            StorageError::Conflict {
                kind: ConflictKind::Other,
                ..
            }
        ));
    }

    #[test]
    fn probe_result_sanitizes_endpoint_and_reports_configured_source() {
        let provider = S3Provider {
            client: Client::from_conf(
                aws_sdk_s3::config::Builder::new()
                    .behavior_version_latest()
                    .region(Region::new("us-east-1"))
                    .credentials_provider(Credentials::from_keys("test", "test", None))
                    .build(),
            ),
            bucket: "bucket-a".to_string(),
            root_prefix: None,
            endpoint: Some("https://user:pass@example.com:9000".to_string()),
            credential_source: CredentialSourceKind::InlineStatic,
        };

        let result = provider.probe_result("s3:HeadBucket", Duration::from_millis(17));
        assert_eq!(
            result.endpoint.as_deref(),
            Some("https://***@example.com:9000")
        );
        assert_eq!(result.credential_source, CredentialSourceKind::InlineStatic);
        assert_eq!(result.scope, ProbeScope::ConfiguredContainer);
        assert_eq!(result.probe_method, "s3:HeadBucket");
        assert_eq!(result.latency_ms, 17);
        assert!(result.capabilities.contains(&Capability::CredentialProbe));

        let debug = format!("{provider:?}");
        assert!(!debug.contains("user"));
        assert!(!debug.contains("pass"));
        assert!(debug.contains("https://***@example.com:9000"));
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

    #[test]
    fn map_sdk_error_treats_no_such_bucket_as_container_not_found() {
        let error = map_sdk_error(
            ProviderKind::S3,
            StorageOperation::Head,
            Some("missing.txt"),
            Some("missing-bucket"),
            aws_sdk_s3::error::SdkError::service_error(
                HeadObjectError::generic(ErrorMetadata::builder().code("NoSuchBucket").build()),
                Response::new(
                    StatusCode::try_from(404).expect("valid status"),
                    SdkBody::empty(),
                ),
            ),
        );

        assert!(matches!(
            error,
            StorageError::ContainerNotFound {
                provider: ProviderKind::S3,
                operation: StorageOperation::Head,
                container,
            } if container == "missing-bucket"
        ));
    }

    #[test]
    fn map_sdk_error_treats_invalid_request_as_invalid_argument() {
        let error = map_sdk_error(
            ProviderKind::S3,
            StorageOperation::GetRange,
            Some("ranges/data.bin"),
            Some("bucket"),
            aws_sdk_s3::error::SdkError::service_error(
                GetObjectError::generic(ErrorMetadata::builder().code("InvalidRequest").build()),
                Response::new(
                    StatusCode::try_from(416).expect("valid status"),
                    SdkBody::empty(),
                ),
            ),
        );

        assert!(matches!(
            error,
            StorageError::InvalidArgument {
                operation: Some(StorageOperation::GetRange),
                argument,
                reason,
            } if argument == "ranges/data.bin" && reason == "InvalidRequest"
        ));
    }

    #[test]
    fn map_sdk_error_extracts_retry_after_from_structured_throttle_response() {
        let mut response = Response::new(
            StatusCode::try_from(503).expect("valid status"),
            SdkBody::empty(),
        );
        response.headers_mut().insert("retry-after", "17");

        let error = map_sdk_error(
            ProviderKind::S3,
            StorageOperation::List,
            Some("fixtures/"),
            Some("bucket"),
            aws_sdk_s3::error::SdkError::service_error(
                HeadObjectError::generic(ErrorMetadata::builder().code("SlowDown").build()),
                response,
            ),
        );

        assert!(matches!(
            error,
            StorageError::Throttled {
                provider: ProviderKind::S3,
                operation: StorageOperation::List,
                retry_after: Some(duration),
            } if duration == Duration::from_secs(17)
        ));
    }

    #[tokio::test]
    async fn env_credential_source_uses_only_configured_env_vars() {
        let mut env_guard = AwsTestEnv::acquire();
        env_guard.clear_standard_aws_env();
        env_guard.set("AWS_ACCESS_KEY_ID", "env-akid");
        env_guard.set("AWS_SECRET_ACCESS_KEY", "env-secret");
        env_guard.set("AWS_SESSION_TOKEN", "env-token");
        env_guard.write_profile_files(
            "ignored",
            "[ignored]\naws_access_key_id = profile-akid\naws_secret_access_key = profile-secret\n",
            "[profile ignored]\nregion = us-west-2\n",
        );

        let credentials = resolved_credentials(ProviderConfig {
            provider: ProviderKind::S3,
            target: TargetConfig {
                container: Some("bucket".to_string()),
                region: Some("us-east-1".to_string()),
                ..TargetConfig::default()
            },
            credentials: CredentialSource::Env {
                variables: vec![
                    "AWS_ACCESS_KEY_ID".to_string(),
                    "AWS_SECRET_ACCESS_KEY".to_string(),
                    "AWS_SESSION_TOKEN".to_string(),
                ],
            },
        })
        .await;

        assert_eq!(credentials.access_key_id(), "env-akid");
        assert_eq!(credentials.secret_access_key(), "env-secret");
        assert_eq!(credentials.session_token(), Some("env-token"));
    }

    #[tokio::test]
    async fn profile_credential_source_uses_named_profile_in_isolation() {
        let mut env_guard = AwsTestEnv::acquire();
        env_guard.clear_standard_aws_env();
        env_guard.write_profile_files(
            "isolated",
            "[isolated]\naws_access_key_id = profile-akid\naws_secret_access_key = profile-secret\naws_session_token = profile-token\n",
            "[profile isolated]\nregion = us-west-2\n",
        );

        let credentials = resolved_credentials(ProviderConfig {
            provider: ProviderKind::S3,
            target: TargetConfig {
                container: Some("bucket".to_string()),
                region: Some("us-east-1".to_string()),
                ..TargetConfig::default()
            },
            credentials: CredentialSource::Profile {
                name: "isolated".to_string(),
            },
        })
        .await;

        assert_eq!(credentials.access_key_id(), "profile-akid");
        assert_eq!(credentials.secret_access_key(), "profile-secret");
        assert_eq!(credentials.session_token(), Some("profile-token"));
    }

    #[tokio::test]
    async fn default_chain_prefers_env_over_profile_when_both_are_present() {
        let mut env_guard = AwsTestEnv::acquire();
        env_guard.clear_standard_aws_env();
        env_guard.set("AWS_ACCESS_KEY_ID", "env-akid");
        env_guard.set("AWS_SECRET_ACCESS_KEY", "env-secret");
        env_guard.set("AWS_PROFILE", "chainprofile");
        env_guard.write_profile_files(
            "chainprofile",
            "[chainprofile]\naws_access_key_id = profile-akid\naws_secret_access_key = profile-secret\n",
            "[profile chainprofile]\nregion = us-west-2\n",
        );

        let credentials = resolved_credentials(ProviderConfig {
            provider: ProviderKind::S3,
            target: TargetConfig {
                container: Some("bucket".to_string()),
                region: Some("us-east-1".to_string()),
                ..TargetConfig::default()
            },
            credentials: CredentialSource::DefaultChain,
        })
        .await;

        assert_eq!(credentials.access_key_id(), "env-akid");
        assert_eq!(credentials.secret_access_key(), "env-secret");
    }

    #[tokio::test]
    async fn default_chain_falls_back_to_profile_when_env_is_absent() {
        let mut env_guard = AwsTestEnv::acquire();
        env_guard.clear_standard_aws_env();
        env_guard.set("AWS_PROFILE", "chainprofile");
        env_guard.write_profile_files(
            "chainprofile",
            "[chainprofile]\naws_access_key_id = profile-akid\naws_secret_access_key = profile-secret\n",
            "[profile chainprofile]\nregion = us-west-2\n",
        );

        let credentials = resolved_credentials(ProviderConfig {
            provider: ProviderKind::S3,
            target: TargetConfig {
                container: Some("bucket".to_string()),
                region: Some("us-east-1".to_string()),
                ..TargetConfig::default()
            },
            credentials: CredentialSource::DefaultChain,
        })
        .await;

        assert_eq!(credentials.access_key_id(), "profile-akid");
        assert_eq!(credentials.secret_access_key(), "profile-secret");
    }

    #[test]
    fn inline_static_configuration_errors_do_not_echo_secret_values() {
        let mut values = BTreeMap::new();
        values.insert(
            "AWS_SECRET_ACCESS_KEY".to_string(),
            "super-secret-value".to_string(),
        );

        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
        let message = runtime.block_on(async {
            load_sdk_config(&ProviderConfig {
                provider: ProviderKind::S3,
                target: TargetConfig::default(),
                credentials: CredentialSource::InlineStatic { values },
            })
            .await
            .expect_err("missing access key should fail")
            .to_string()
        });

        assert!(!message.contains("super-secret-value"));
    }

    #[test]
    fn env_configuration_errors_do_not_echo_secret_values() {
        let mut env_guard = AwsTestEnv::acquire();
        env_guard.clear_standard_aws_env();
        env_guard.set("AWS_SECRET_ACCESS_KEY", "super-secret-value");

        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
        let message = runtime.block_on(async {
            load_sdk_config(&ProviderConfig {
                provider: ProviderKind::S3,
                target: TargetConfig::default(),
                credentials: CredentialSource::Env {
                    variables: vec![
                        "AWS_SECRET_ACCESS_KEY".to_string(),
                        "AWS_ACCESS_KEY_ID".to_string(),
                    ],
                },
            })
            .await
            .expect_err("missing access key should fail")
            .to_string()
        });

        assert!(!message.contains("super-secret-value"));
    }

    async fn resolved_credentials(config: ProviderConfig) -> Credentials {
        let sdk_config = load_sdk_config(&config)
            .await
            .expect("sdk config should load");
        let provider = sdk_config
            .credentials_provider()
            .expect("credentials provider should be configured");
        provider
            .provide_credentials()
            .await
            .expect("credentials should resolve")
    }

    fn test_credentials() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("AWS_ACCESS_KEY_ID".to_string(), "test".to_string()),
            ("AWS_SECRET_ACCESS_KEY".to_string(), "test".to_string()),
        ])
    }

    fn aws_env_lock() -> &'static StdMutex<()> {
        static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| StdMutex::new(()))
    }

    struct AwsTestEnv {
        _guard: MutexGuard<'static, ()>,
        saved: Vec<(String, Option<String>)>,
        temp_dir: std::path::PathBuf,
    }

    impl AwsTestEnv {
        fn acquire() -> Self {
            let guard = aws_env_lock()
                .lock()
                .expect("aws env lock should not poison");
            let temp_dir = std::env::temp_dir().join(format!(
                "storageprims-s3-test-{}-{}",
                std::process::id(),
                unique_suffix()
            ));
            std::fs::create_dir_all(&temp_dir).expect("temp dir should be created");

            let mut env = Self {
                _guard: guard,
                saved: Vec::new(),
                temp_dir,
            };
            env.set("AWS_EC2_METADATA_DISABLED", "true");
            env
        }

        fn clear_standard_aws_env(&mut self) {
            for key in [
                "AWS_ACCESS_KEY_ID",
                "AWS_SECRET_ACCESS_KEY",
                "AWS_SESSION_TOKEN",
                "AWS_PROFILE",
                "AWS_CONFIG_FILE",
                "AWS_SHARED_CREDENTIALS_FILE",
                "AWS_DEFAULT_REGION",
                "AWS_REGION",
            ] {
                self.remove(key);
            }
        }

        fn set(&mut self, key: &str, value: &str) {
            self.capture(key);
            std::env::set_var(key, value);
        }

        fn remove(&mut self, key: &str) {
            self.capture(key);
            std::env::remove_var(key);
        }

        fn write_profile_files(&mut self, profile: &str, credentials: &str, config: &str) {
            let credentials_path = self.temp_dir.join("credentials");
            let config_path = self.temp_dir.join("config");
            std::fs::write(&credentials_path, credentials).expect("credentials file should write");
            std::fs::write(&config_path, config).expect("config file should write");

            self.set(
                "AWS_SHARED_CREDENTIALS_FILE",
                credentials_path.to_str().expect("utf-8 temp path"),
            );
            self.set(
                "AWS_CONFIG_FILE",
                config_path.to_str().expect("utf-8 temp path"),
            );
            self.set("AWS_PROFILE", profile);
        }

        fn capture(&mut self, key: &str) {
            if self.saved.iter().any(|(saved_key, _)| saved_key == key) {
                return;
            }
            self.saved.push((key.to_string(), std::env::var(key).ok()));
        }
    }

    impl Drop for AwsTestEnv {
        fn drop(&mut self) {
            for (key, value) in self.saved.iter().rev() {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
            let _ = std::fs::remove_dir_all(&self.temp_dir);
        }
    }

    fn unique_suffix() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be monotonic enough for tests")
            .as_nanos()
    }
}
