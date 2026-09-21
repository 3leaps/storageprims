//! AWS S3 provider implementation for storageprims.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use aws_config::{BehaviorVersion, Region};
use aws_credential_types::Credentials;
use aws_sdk_s3::error::ProvideErrorMetadata;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use aws_smithy_runtime_api::box_error::BoxError;
use aws_smithy_runtime_api::client::interceptors::context::BeforeDeserializationInterceptorContextRef;
use aws_smithy_runtime_api::client::interceptors::Intercept;
use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
use aws_smithy_types::config_bag::ConfigBag;
use aws_smithy_types::retry::RetryConfig;
use bytes::Bytes;
use http_body::{Frame, SizeHint};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use storageprims_core::{
    sanitize_endpoint, BoxFuture, BoxedByteStream, ByteWindow, Capability, ConflictKind,
    CopyRequest, CopyResult, CopyStrategy, CredentialSource, CredentialSourceKind,
    DelimiterListRequest, DelimiterListResult, DelimiterListingProvider, GetRangeRequest,
    GuardedRangeRequest, GuardedReadProvider, GuardedReadResponse, GuardedReadSelection,
    ListOptions, ListResult, ObjectMetadata, ObjectSummary, ProbeResult, ProbeScope,
    ProviderConfig, ProviderKind, PutOptions, PutPrecondition, PutResult, Result,
    SourceObservation, SourceReceipt, SourceSelector, StorageError, StorageOperation,
    StorageProvider, StorageUri,
};
use tokio::io::{AsyncRead, ReadBuf};

const S3_DEFAULT_MAX_KEYS: u32 = 1_000;

#[derive(Clone)]
pub struct S3Provider {
    client: Client,
    guarded_config: aws_sdk_s3::Config,
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

#[derive(Debug)]
struct ResponseStatusInterceptor {
    status: Arc<Mutex<Option<u16>>>,
}

impl Intercept for ResponseStatusInterceptor {
    fn name(&self) -> &'static str {
        "storageprims_guarded_response_status"
    }

    fn read_before_deserialization(
        &self,
        context: &BeforeDeserializationInterceptorContextRef<'_>,
        _runtime_components: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> std::result::Result<(), BoxError> {
        *self.status.lock().expect("response status mutex poisoned") =
            Some(context.response().status().as_u16());
        Ok(())
    }
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

        // Guarded reads fail closed rather than depending on SDK retry mutation
        // semantics for VersionId, If-Match, or Range headers.
        let mut guarded_service_config = aws_sdk_s3::config::Builder::from(&sdk_config);
        guarded_service_config.set_force_path_style(config.target.force_path_style);
        guarded_service_config.set_retry_config(Some(RetryConfig::standard().with_max_attempts(1)));
        let guarded_config = guarded_service_config.build();

        Ok(Self {
            client,
            guarded_config,
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

    fn guarded_target_identity(&self) -> String {
        let endpoint = self
            .endpoint
            .as_deref()
            .map(sanitize_endpoint)
            .unwrap_or_else(|| "aws".to_string());
        format!(
            "s3|{}|{}|{}",
            endpoint,
            self.bucket,
            self.root_prefix.as_deref().unwrap_or("")
        )
    }

    fn guarded_client(&self, status: Arc<Mutex<Option<u16>>>) -> Client {
        Client::from_conf(
            self.guarded_config
                .to_builder()
                .interceptor(ResponseStatusInterceptor { status })
                .build(),
        )
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
        vec![
            Capability::DelimiterListing,
            Capability::CredentialProbe,
            Capability::ConditionalPut,
            Capability::GuardedRead,
        ]
    }

    fn guarded_reads(&self) -> Option<&dyn GuardedReadProvider> {
        Some(self)
    }

    fn delimiter_lists(&self) -> Option<&dyn DelimiterListingProvider> {
        Some(self)
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

impl DelimiterListingProvider for S3Provider {
    fn list_delimited(&self, request: DelimiterListRequest) -> BoxFuture<'_, DelimiterListResult> {
        Box::pin(async move {
            validate_delimiter(&request.delimiter)?;
            validate_continuation_token(request.continuation_token.as_deref())?;
            let caller_prefix = request.prefix.as_deref().unwrap_or("").to_string();
            let prefix =
                resolve_list_prefix(self.root_prefix.as_deref(), request.prefix.as_deref())?;
            let max_keys = normalize_max_keys(request.max_keys)?;
            let has_continuation = request.continuation_token.is_some();
            let response = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .set_prefix(prefix)
                .delimiter(request.delimiter)
                .set_continuation_token(request.continuation_token)
                .set_max_keys(max_keys)
                .send()
                .await
                .map_err(|error| map_delimiter_list_error(error, has_continuation))?;

            let is_truncated = response.is_truncated().ok_or_else(|| {
                invalid_delimiter_page("S3 returned a delimiter page without a truncation flag")
            })?;

            validate_delimiter_list_page(
                self.root_prefix.as_deref(),
                &caller_prefix,
                request.max_keys.filter(|value| *value != 0),
                response.contents().iter().map(|item| item.key()),
                response.common_prefixes().iter().map(|item| item.prefix()),
                is_truncated,
                response.next_continuation_token(),
            )?;

            let objects = response
                .contents()
                .iter()
                .map(|item| {
                    let key = item.key().expect("delimiter page was validated");
                    ObjectSummary {
                        path: strip_root_prefix(self.root_prefix.as_deref(), key)
                            .expect("delimiter page was validated")
                            .to_string(),
                        size: item
                            .size()
                            .and_then(|value| u64::try_from(value).ok())
                            .unwrap_or(0),
                        etag: item.e_tag().map(ToString::to_string),
                        content_type: None,
                        last_modified: item.last_modified().map(|value| value.to_string()),
                    }
                })
                .collect();
            let common_prefixes = response
                .common_prefixes()
                .iter()
                .map(|item| {
                    strip_root_prefix(
                        self.root_prefix.as_deref(),
                        item.prefix().expect("delimiter page was validated"),
                    )
                    .expect("delimiter page was validated")
                    .to_string()
                })
                .collect();

            Ok(DelimiterListResult {
                objects,
                common_prefixes,
                continuation_token: response.next_continuation_token().map(ToString::to_string),
                is_truncated,
            })
        })
    }
}

impl GuardedReadProvider for S3Provider {
    fn guarded_read_target_identity(&self) -> String {
        self.guarded_target_identity()
    }

    fn observe_source(&self, key: &str) -> BoxFuture<'_, SourceObservation> {
        let key = key.to_string();
        Box::pin(async move {
            let resolved_key = self.resolve_key(&key)?;
            let response = self
                .guarded_client(Arc::new(Mutex::new(None)))
                .head_object()
                .bucket(&self.bucket)
                .key(&resolved_key)
                .send()
                .await
                .map_err(|error| {
                    map_sdk_error(
                        ProviderKind::S3,
                        StorageOperation::ObserveSource,
                        Some(&resolved_key),
                        Some(&self.bucket),
                        error,
                    )
                })?;
            if response.delete_marker().unwrap_or(false) {
                return Err(selected_not_found(StorageOperation::ObserveSource, &key));
            }
            let native_version = observed_native_version(response.version_id());
            let validator = response.e_tag().map(ToString::to_string);
            let total_size = response
                .content_length()
                .and_then(|size| u64::try_from(size).ok());
            Ok(SourceObservation {
                receipt: SourceReceipt {
                    path: key,
                    native_version,
                    validator,
                    total_size,
                    requested_window: None,
                    returned_window: None,
                },
                content_type: response.content_type().map(ToString::to_string),
                last_modified: response.last_modified().map(ToString::to_string),
                metadata: response
                    .metadata()
                    .map(|values| values.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                    .unwrap_or_default(),
            })
        })
    }

    fn guarded_head(&self, selection: GuardedReadSelection) -> BoxFuture<'_, SourceReceipt> {
        Box::pin(async move {
            let key = selection.key().to_string();
            selection.validate_for(
                &self.guarded_target_identity(),
                &key,
                StorageOperation::GuardedHead,
            )?;
            let resolved_key = self.resolve_key(&key)?;
            let request = self
                .guarded_client(Arc::new(Mutex::new(None)))
                .head_object()
                .bucket(&self.bucket)
                .key(&resolved_key);
            let request = apply_head_selector(request, selection.selector());
            let response = request.send().await.map_err(|error| {
                map_guarded_sdk_error(
                    StorageOperation::GuardedHead,
                    selection.selector(),
                    Some(&resolved_key),
                    Some(&self.bucket),
                    error,
                )
            })?;
            if response.delete_marker().unwrap_or(false) {
                return Err(selected_not_found(StorageOperation::GuardedHead, &key));
            }
            checked_receipt(
                &key,
                selection.selector(),
                S3ReceiptEvidence::from_head(&response),
                None,
                None,
                StorageOperation::GuardedHead,
            )
        })
    }

    fn guarded_get(&self, selection: GuardedReadSelection) -> BoxFuture<'_, GuardedReadResponse> {
        Box::pin(async move {
            let key = selection.key().to_string();
            selection.validate_for(
                &self.guarded_target_identity(),
                &key,
                StorageOperation::GuardedGet,
            )?;
            let resolved_key = self.resolve_key(&key)?;
            let response_status = Arc::new(Mutex::new(None));
            let request = self
                .guarded_client(response_status.clone())
                .get_object()
                .bucket(&self.bucket)
                .key(&resolved_key);
            let request = apply_get_selector(request, selection.selector());
            let response = request.send().await.map_err(|error| {
                map_guarded_sdk_error(
                    StorageOperation::GuardedGet,
                    selection.selector(),
                    Some(&resolved_key),
                    Some(&self.bucket),
                    error,
                )
            })?;
            if response.delete_marker().unwrap_or(false) {
                return Err(selected_not_found(StorageOperation::GuardedGet, &key));
            }
            let content_length =
                checked_content_length(response.content_length(), StorageOperation::GuardedGet)?;
            let response_status = response_status
                .lock()
                .expect("response status mutex poisoned")
                .ok_or_else(|| {
                    invalid_response(
                        StorageOperation::GuardedGet,
                        "full response status was not available",
                    )
                })?;
            let window = validate_full_get_response(
                response_status,
                response.content_range(),
                content_length,
            )?;
            let receipt = checked_receipt(
                &key,
                selection.selector(),
                S3ReceiptEvidence::from_get(&response),
                None,
                window,
                StorageOperation::GuardedGet,
            )?;
            let reader: BoxedByteStream = Box::new(DeclaredLengthRead::new(
                Box::new(response.body.into_async_read()),
                content_length,
            ));
            Ok(GuardedReadResponse { receipt, reader })
        })
    }

    fn guarded_get_range(
        &self,
        request: GuardedRangeRequest,
    ) -> BoxFuture<'_, GuardedReadResponse> {
        Box::pin(async move {
            if request.length == 0 {
                return Err(StorageError::InvalidArgument {
                    operation: Some(StorageOperation::GuardedGetRange),
                    argument: "length".to_string(),
                    reason: "range length must be greater than zero".to_string(),
                });
            }
            let requested_end =
                request
                    .offset
                    .checked_add(request.length - 1)
                    .ok_or_else(|| StorageError::InvalidArgument {
                        operation: Some(StorageOperation::GuardedGetRange),
                        argument: "length".to_string(),
                        reason: "requested range overflows u64".to_string(),
                    })?;
            let key = request.selection.key().to_string();
            request.selection.validate_for(
                &self.guarded_target_identity(),
                &key,
                StorageOperation::GuardedGetRange,
            )?;
            let resolved_key = self.resolve_key(&key)?;
            let range = format!("bytes={}-{}", request.offset, requested_end);
            let response_status = Arc::new(Mutex::new(None));
            let request_builder = self
                .guarded_client(response_status.clone())
                .get_object()
                .bucket(&self.bucket)
                .key(&resolved_key)
                .range(range);
            let request_builder = apply_get_selector(request_builder, request.selection.selector());
            let response = request_builder.send().await.map_err(|error| {
                map_guarded_sdk_error(
                    StorageOperation::GuardedGetRange,
                    request.selection.selector(),
                    Some(&resolved_key),
                    Some(&self.bucket),
                    error,
                )
            })?;
            let response_status = response_status
                .lock()
                .expect("response status mutex poisoned")
                .ok_or_else(|| {
                    invalid_response(
                        StorageOperation::GuardedGetRange,
                        "range response status was not available",
                    )
                })?;
            if response.delete_marker().unwrap_or(false) {
                return Err(selected_not_found(StorageOperation::GuardedGetRange, &key));
            }
            let returned =
                parse_content_range(response.content_range(), StorageOperation::GuardedGetRange)?;
            let expected_end = match returned.total_size {
                Some(total) => {
                    if request.offset >= total {
                        return Err(StorageError::InvalidArgument {
                            operation: Some(StorageOperation::GuardedGetRange),
                            argument: "offset".to_string(),
                            reason: "range begins at or beyond the end of the object".to_string(),
                        });
                    }
                    requested_end.min(total - 1)
                }
                None => requested_end,
            };
            let requested = ByteWindow {
                start: request.offset,
                end: requested_end,
            };
            let expected = ByteWindow {
                start: request.offset,
                end: expected_end,
            };
            if returned.window != expected {
                return Err(invalid_response(
                    StorageOperation::GuardedGetRange,
                    "response range does not match the requested window",
                ));
            }
            let returned_length = returned.window.checked_len().ok_or_else(|| {
                invalid_response(
                    StorageOperation::GuardedGetRange,
                    "range response window length overflows u64",
                )
            })?;
            if response_status == 200
                && !(returned.window.start == 0
                    && returned.total_size == Some(returned_length)
                    && expected == returned.window)
            {
                return Err(invalid_response(
                    StorageOperation::GuardedGetRange,
                    "full-status range response does not prove the entire requested object",
                ));
            }
            if response_status != 200 && response_status != 206 {
                return Err(invalid_response(
                    StorageOperation::GuardedGetRange,
                    "range response has an unexpected successful status",
                ));
            }
            let content_length = checked_content_length(
                response.content_length(),
                StorageOperation::GuardedGetRange,
            )?
            .ok_or_else(|| {
                invalid_response(
                    StorageOperation::GuardedGetRange,
                    "range response is missing content length",
                )
            })?;
            if content_length != returned_length {
                return Err(invalid_response(
                    StorageOperation::GuardedGetRange,
                    "range response content length does not match content range",
                ));
            }
            let mut receipt = checked_receipt(
                &key,
                request.selection.selector(),
                S3ReceiptEvidence::from_get(&response),
                Some(requested),
                Some(returned.window),
                StorageOperation::GuardedGetRange,
            )?;
            receipt.total_size = returned.total_size;
            let reader: BoxedByteStream = Box::new(DeclaredLengthRead::new(
                Box::new(response.body.into_async_read()),
                Some(content_length),
            ));
            Ok(GuardedReadResponse { receipt, reader })
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

fn delimiter_page_bound(max_keys: Option<u32>) -> u32 {
    match max_keys {
        None | Some(0) => S3_DEFAULT_MAX_KEYS,
        Some(value) => value,
    }
}

fn validate_delimiter(delimiter: &str) -> Result<()> {
    let bytes = delimiter.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 1024
        || bytes.iter().any(|byte| *byte <= 0x1f || *byte == 0x7f)
    {
        return Err(StorageError::InvalidArgument {
            operation: Some(StorageOperation::List),
            argument: "delimiter".to_string(),
            reason: "delimiter must be 1..=1024 bytes and contain no control characters"
                .to_string(),
        });
    }
    Ok(())
}

fn validate_continuation_token(token: Option<&str>) -> Result<()> {
    if token.is_some_and(|value| value.is_empty() || value.len() > 8192) {
        return Err(StorageError::InvalidArgument {
            operation: Some(StorageOperation::List),
            argument: "continuation_token".to_string(),
            reason: "continuation token must be 1..=8192 bytes when present".to_string(),
        });
    }
    Ok(())
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

fn apply_head_selector(
    request: aws_sdk_s3::operation::head_object::builders::HeadObjectFluentBuilder,
    selector: &SourceSelector,
) -> aws_sdk_s3::operation::head_object::builders::HeadObjectFluentBuilder {
    match selector {
        SourceSelector::NativeVersion { token } => request.version_id(token),
        SourceSelector::ValidatorMatch { token } => request.if_match(token),
    }
}

fn apply_get_selector(
    request: aws_sdk_s3::operation::get_object::builders::GetObjectFluentBuilder,
    selector: &SourceSelector,
) -> aws_sdk_s3::operation::get_object::builders::GetObjectFluentBuilder {
    match selector {
        SourceSelector::NativeVersion { token } => request.version_id(token),
        SourceSelector::ValidatorMatch { token } => request.if_match(token),
    }
}

fn observed_native_version(version: Option<&str>) -> Option<String> {
    version
        .filter(|value| *value != "null")
        .map(ToString::to_string)
}

fn selected_not_found(operation: StorageOperation, key: &str) -> StorageError {
    StorageError::NotFound {
        provider: ProviderKind::S3,
        operation,
        path: key.to_string(),
    }
}

fn invalid_response(operation: StorageOperation, detail: &str) -> StorageError {
    StorageError::Io {
        operation: Some(operation),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, detail),
    }
}

fn checked_content_length(value: Option<i64>, operation: StorageOperation) -> Result<Option<u64>> {
    value
        .map(|length| {
            u64::try_from(length).map_err(|_| {
                invalid_response(operation, "response contains an invalid content length")
            })
        })
        .transpose()
}

struct S3ReceiptEvidence<'a> {
    version_id: Option<&'a str>,
    etag: Option<&'a str>,
    content_length: Option<i64>,
}

impl<'a> S3ReceiptEvidence<'a> {
    fn from_head(response: &'a aws_sdk_s3::operation::head_object::HeadObjectOutput) -> Self {
        Self {
            version_id: response.version_id(),
            etag: response.e_tag(),
            content_length: response.content_length(),
        }
    }

    fn from_get(response: &'a aws_sdk_s3::operation::get_object::GetObjectOutput) -> Self {
        Self {
            version_id: response.version_id(),
            etag: response.e_tag(),
            content_length: response.content_length(),
        }
    }
}

fn checked_receipt(
    path: &str,
    selector: &SourceSelector,
    evidence: S3ReceiptEvidence<'_>,
    requested_window: Option<ByteWindow>,
    returned_window: Option<ByteWindow>,
    operation: StorageOperation,
) -> Result<SourceReceipt> {
    let native_version = observed_native_version(evidence.version_id);
    let validator = evidence.etag.map(ToString::to_string);
    match selector {
        SourceSelector::NativeVersion { token } => {
            if native_version.as_deref() != Some(token) {
                return Err(invalid_response(
                    operation,
                    "response native version does not match the selected source",
                ));
            }
        }
        SourceSelector::ValidatorMatch { token } => {
            let strong_response = validator.as_ref().is_some_and(|value| {
                SourceSelector::ValidatorMatch {
                    token: value.clone(),
                }
                .validate()
                .is_ok()
            });
            if !strong_response || validator.as_deref() != Some(token) {
                return Err(invalid_response(
                    operation,
                    "response validator does not match the selected source",
                ));
            }
        }
    }
    Ok(SourceReceipt {
        path: path.to_string(),
        native_version,
        validator,
        total_size: checked_content_length(evidence.content_length, operation)?,
        requested_window,
        returned_window,
    })
}

struct ParsedContentRange {
    window: ByteWindow,
    total_size: Option<u64>,
}

fn parse_content_range(
    value: Option<&str>,
    operation: StorageOperation,
) -> Result<ParsedContentRange> {
    let value = value
        .ok_or_else(|| invalid_response(operation, "range response is missing content range"))?;
    let fields = value
        .strip_prefix("bytes ")
        .and_then(|value| value.split_once('/'));
    let Some((window, total)) = fields else {
        return Err(invalid_response(
            operation,
            "range response has an invalid content range",
        ));
    };
    let Some((start, end)) = window.split_once('-') else {
        return Err(invalid_response(
            operation,
            "range response has an invalid content range",
        ));
    };
    let start = start
        .parse::<u64>()
        .map_err(|_| invalid_response(operation, "range response has an invalid content range"))?;
    let end = end
        .parse::<u64>()
        .map_err(|_| invalid_response(operation, "range response has an invalid content range"))?;
    if end < start {
        return Err(invalid_response(
            operation,
            "range response has an invalid content range",
        ));
    }
    let total_size = if total == "*" {
        None
    } else {
        let total = total.parse::<u64>().map_err(|_| {
            invalid_response(operation, "range response has an invalid content range")
        })?;
        if total == 0 || end >= total {
            return Err(invalid_response(
                operation,
                "range response has an invalid content range",
            ));
        }
        Some(total)
    };
    Ok(ParsedContentRange {
        window: ByteWindow { start, end },
        total_size,
    })
}

fn validate_full_get_response(
    status: u16,
    content_range: Option<&str>,
    content_length: Option<u64>,
) -> Result<Option<ByteWindow>> {
    if status != 200 {
        return Err(invalid_response(
            StorageOperation::GuardedGet,
            "full guarded read received a partial or unexpected successful status",
        ));
    }
    let Some(content_range) = content_range else {
        return Ok(content_length
            .filter(|length| *length > 0)
            .map(|length| ByteWindow {
                start: 0,
                end: length - 1,
            }));
    };
    let parsed = parse_content_range(Some(content_range), StorageOperation::GuardedGet)?;
    let returned_length = parsed.window.checked_len().ok_or_else(|| {
        invalid_response(
            StorageOperation::GuardedGet,
            "full response window length overflows u64",
        )
    })?;
    if parsed.window.start != 0
        || parsed.total_size != Some(returned_length)
        || content_length != Some(returned_length)
    {
        return Err(invalid_response(
            StorageOperation::GuardedGet,
            "full response content range does not describe the entire object",
        ));
    }
    Ok(Some(parsed.window))
}

fn map_guarded_sdk_error<E>(
    operation: StorageOperation,
    selector: &SourceSelector,
    target: Option<&str>,
    container: Option<&str>,
    error: aws_sdk_s3::error::SdkError<E>,
) -> StorageError
where
    E: std::error::Error + ProvideErrorMetadata + Send + Sync + 'static,
{
    use aws_sdk_s3::error::SdkError;

    if let SdkError::ServiceError(context) = &error {
        let status = context.raw().status().as_u16();
        let delete_marker = context
            .raw()
            .headers()
            .get("x-amz-delete-marker")
            .is_some_and(|value| value.eq_ignore_ascii_case("true"));
        if status == 304 {
            return invalid_response(
                operation,
                "guarded read received an unexpected not-modified response",
            );
        }
        if status == 405 && delete_marker {
            return selected_not_found(operation, target.unwrap_or_default());
        }
        if status == 412 && matches!(selector, SourceSelector::ValidatorMatch { .. }) {
            return StorageError::Conflict {
                provider: ProviderKind::S3,
                operation,
                target: target.map(ToString::to_string),
                kind: ConflictKind::TokenMismatch,
                detail: "source validator did not match".to_string(),
            };
        }
        if status == 416 {
            return StorageError::InvalidArgument {
                operation: Some(operation),
                argument: "offset".to_string(),
                reason: "requested range is outside the object".to_string(),
            };
        }
        if status == 403 {
            return StorageError::AccessDenied {
                provider: ProviderKind::S3,
                operation,
                target: target.map(ToString::to_string),
                detail: "access denied".to_string(),
            };
        }
        match context.err().code().unwrap_or_default() {
            "PreconditionFailed" if matches!(selector, SourceSelector::ValidatorMatch { .. }) => {
                return StorageError::Conflict {
                    provider: ProviderKind::S3,
                    operation,
                    target: target.map(ToString::to_string),
                    kind: ConflictKind::TokenMismatch,
                    detail: "source validator did not match".to_string(),
                };
            }
            "NoSuchVersion" => {
                return selected_not_found(operation, target.unwrap_or_default());
            }
            "InvalidRange" | "RequestedRangeNotSatisfiable" => {
                return StorageError::InvalidArgument {
                    operation: Some(operation),
                    argument: "offset".to_string(),
                    reason: "requested range is outside the object".to_string(),
                };
            }
            _ => {}
        }
    }
    map_sdk_error(ProviderKind::S3, operation, target, container, error)
}

/// An owned reader that exposes declared-length truncation and overlong-body errors.
struct DeclaredLengthRead {
    reader: BoxedByteStream,
    remaining: Option<u64>,
    checked_end: bool,
}

impl DeclaredLengthRead {
    fn new(reader: BoxedByteStream, remaining: Option<u64>) -> Self {
        Self {
            reader,
            remaining,
            checked_end: false,
        }
    }
}

impl AsyncRead for DeclaredLengthRead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let Some(remaining) = self.remaining else {
            return Pin::new(&mut *self.reader).poll_read(cx, buf);
        };
        if remaining == 0 {
            if self.checked_end {
                return Poll::Ready(Ok(()));
            }
            let mut probe = [0_u8; 1];
            let mut probe_buf = ReadBuf::new(&mut probe);
            return match Pin::new(&mut *self.reader).poll_read(cx, &mut probe_buf) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Ok(())) if probe_buf.filled().is_empty() => {
                    self.checked_end = true;
                    Poll::Ready(Ok(()))
                }
                Poll::Ready(Ok(())) => Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "stream exceeded declared response length",
                ))),
                Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            };
        }

        let capacity = buf.remaining().min(remaining as usize);
        if capacity == 0 {
            return Poll::Ready(Ok(()));
        }
        let mut temporary = vec![0_u8; capacity];
        let mut temporary_buf = ReadBuf::new(&mut temporary);
        match Pin::new(&mut *self.reader).poll_read(cx, &mut temporary_buf) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(())) if temporary_buf.filled().is_empty() => {
                Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "stream ended before declared response length",
                )))
            }
            Poll::Ready(Ok(())) => {
                let read = temporary_buf.filled().len();
                self.remaining = Some(remaining - read as u64);
                buf.put_slice(&temporary_buf.filled()[..read]);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
        }
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
                    detail: "S3 request failed".to_string(),
                    source: None,
                },
            }
        }
        SdkError::TimeoutError(_) | SdkError::DispatchFailure(_) => {
            StorageError::ProviderUnavailable {
                provider,
                operation,
                detail: "S3 request transport failed".to_string(),
            }
        }
        SdkError::ResponseError(_) => StorageError::ProviderUnavailable {
            provider,
            operation,
            detail: "S3 response transport failed".to_string(),
        },
        SdkError::ConstructionFailure(construction_error) => StorageError::InvalidArgument {
            operation: Some(operation),
            argument: target.unwrap_or_default().to_string(),
            reason: format!("{construction_error:?}"),
        },
        _ => StorageError::Other {
            provider: Some(provider),
            operation: Some(operation),
            detail: "S3 request failed".to_string(),
            source: None,
        },
    }
}

fn map_delimiter_list_error<E>(
    error: aws_sdk_s3::error::SdkError<E>,
    has_continuation: bool,
) -> StorageError
where
    E: std::error::Error + ProvideErrorMetadata + Send + Sync + 'static,
{
    if has_continuation
        && error
            .as_service_error()
            .and_then(ProvideErrorMetadata::code)
            .is_some_and(|code| {
                matches!(code, "InvalidArgument" | "InvalidRequest" | "InvalidToken")
            })
    {
        return StorageError::InvalidArgument {
            operation: Some(StorageOperation::List),
            argument: "continuation_token".to_string(),
            reason: "provider rejected the continuation token".to_string(),
        };
    }

    map_sdk_error(ProviderKind::S3, StorageOperation::List, None, None, error)
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

fn invalid_delimiter_page(detail: &str) -> StorageError {
    StorageError::Other {
        provider: Some(ProviderKind::S3),
        operation: Some(StorageOperation::List),
        detail: detail.to_string(),
        source: None,
    }
}

fn validate_delimiter_list_page<'a>(
    root_prefix: Option<&str>,
    caller_prefix: &str,
    max_keys: Option<u32>,
    object_keys: impl IntoIterator<Item = Option<&'a str>>,
    common_prefixes: impl IntoIterator<Item = Option<&'a str>>,
    is_truncated: bool,
    continuation_token: Option<&str>,
) -> Result<()> {
    if is_truncated != continuation_token.is_some() {
        return Err(invalid_delimiter_page(
            "S3 returned an inconsistent delimiter-listing continuation pair",
        ));
    }
    if continuation_token.is_some_and(|token| token.is_empty() || token.len() > 8192) {
        return Err(invalid_delimiter_page(
            "S3 returned an invalid delimiter-listing continuation token",
        ));
    }

    let page_bound = delimiter_page_bound(max_keys) as usize;
    let mut row_count = 0_usize;
    let mut projected_objects = HashSet::new();
    for key in object_keys {
        row_count = row_count
            .checked_add(1)
            .ok_or_else(|| invalid_delimiter_page("S3 returned an oversized delimiter page"))?;
        if row_count > page_bound {
            return Err(invalid_delimiter_page(
                "S3 returned more delimiter-page rows than requested",
            ));
        }
        let key = key.ok_or_else(|| {
            invalid_delimiter_page("S3 returned a delimiter page entry without an object key")
        })?;
        let projected = strip_root_prefix(root_prefix, key)?;
        if !projected.starts_with(caller_prefix) {
            return Err(invalid_delimiter_page(
                "S3 returned an object outside the requested delimiter prefix",
            ));
        }
        projected_objects.insert(projected);
    }

    for prefix in common_prefixes {
        row_count = row_count
            .checked_add(1)
            .ok_or_else(|| invalid_delimiter_page("S3 returned an oversized delimiter page"))?;
        if row_count > page_bound {
            return Err(invalid_delimiter_page(
                "S3 returned more delimiter-page rows than requested",
            ));
        }
        let prefix = prefix.ok_or_else(|| {
            invalid_delimiter_page("S3 returned a delimiter page entry without a common prefix")
        })?;
        let projected = strip_root_prefix(root_prefix, prefix)?;
        if projected.is_empty() {
            return Err(invalid_delimiter_page(
                "S3 returned an empty projected common prefix",
            ));
        }
        if !projected.starts_with(caller_prefix) {
            return Err(invalid_delimiter_page(
                "S3 returned a common prefix outside the requested delimiter prefix",
            ));
        }
        if projected_objects.contains(projected) {
            return Err(invalid_delimiter_page(
                "S3 returned the same path as an object and common prefix",
            ));
        }
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn scripted_response_server(
        response: impl Into<String>,
    ) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("scripted server binds");
        let address = listener.local_addr().expect("scripted server has address");
        let response = response.into();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener
                .accept()
                .await
                .expect("scripted server accepts request");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 2048];
            loop {
                let read = socket.read(&mut buffer).await.expect("request reads");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            socket
                .write_all(response.as_bytes())
                .await
                .expect("response writes");
            request
        });
        (format!("http://{address}"), task)
    }

    async fn scripted_responses_server(
        responses: &'static [&'static str],
    ) -> (String, tokio::task::JoinHandle<Vec<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("scripted server binds");
        let address = listener.local_addr().expect("scripted server has address");
        let task = tokio::spawn(async move {
            let mut requests = Vec::with_capacity(responses.len());
            for response in responses {
                let (mut socket, _) = listener
                    .accept()
                    .await
                    .expect("scripted server accepts request");
                let mut request = Vec::new();
                let mut buffer = [0_u8; 2048];
                loop {
                    let read = socket.read(&mut buffer).await.expect("request reads");
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                socket
                    .write_all(response.as_bytes())
                    .await
                    .expect("response writes");
                requests.push(request);
            }
            requests
        });
        (format!("http://{address}"), task)
    }

    async fn scripted_owned_responses_server(
        responses: Vec<String>,
    ) -> (String, tokio::task::JoinHandle<Vec<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("scripted server binds");
        let address = listener.local_addr().expect("scripted server has address");
        let task = tokio::spawn(async move {
            let mut requests = Vec::with_capacity(responses.len());
            for response in responses {
                let (mut socket, _) = listener
                    .accept()
                    .await
                    .expect("scripted server accepts request");
                let mut request = Vec::new();
                let mut buffer = [0_u8; 2048];
                loop {
                    let read = socket.read(&mut buffer).await.expect("request reads");
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                socket
                    .write_all(response.as_bytes())
                    .await
                    .expect("response writes");
                requests.push(request);
            }
            requests
        });
        (format!("http://{address}"), task)
    }

    async fn scripted_provider(endpoint: String) -> S3Provider {
        S3Provider::from_config(ProviderConfig {
            provider: ProviderKind::S3,
            target: TargetConfig {
                container: Some("bucket".to_string()),
                region: Some("us-east-1".to_string()),
                endpoint: Some(endpoint),
                force_path_style: Some(true),
                ..TargetConfig::default()
            },
            credentials: CredentialSource::InlineStatic {
                values: test_credentials(),
            },
        })
        .await
        .expect("scripted provider config is valid")
    }

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
            guarded_config: aws_sdk_s3::config::Builder::new()
                .behavior_version_latest()
                .region(Region::new("us-east-1"))
                .credentials_provider(Credentials::from_keys("test", "test", None))
                .build(),
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
            guarded_config: aws_sdk_s3::config::Builder::new()
                .behavior_version_latest()
                .region(Region::new("us-east-1"))
                .credentials_provider(Credentials::from_keys("test", "test", None))
                .build(),
            bucket: "bucket-a".to_string(),
            root_prefix: None,
            endpoint: None,
            credential_source: CredentialSourceKind::InlineStatic,
        };

        assert_eq!(
            provider.capabilities(),
            vec![
                Capability::DelimiterListing,
                Capability::CredentialProbe,
                Capability::ConditionalPut,
                Capability::GuardedRead,
            ]
        );
        assert!(provider.guarded_reads().is_some());
        let provider_dyn: &dyn StorageProvider = &provider;
        assert!(storageprims_core::require_delimiter_listing(provider_dyn).is_ok());
        let guarded =
            storageprims_core::require_guarded_reads(provider_dyn, StorageOperation::GuardedGet)
                .expect("S3 guarded reads are callable through a trait object");
        let selection = guarded
            .bind_guarded_read(
                "object-a",
                SourceSelector::NativeVersion {
                    token: "version-a".to_string(),
                },
            )
            .expect("valid selector binds to this provider");
        assert_eq!(selection.key(), "object-a");
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
    fn delimiter_and_token_admission_enforce_frozen_bounds() {
        for delimiter in ["/", ".", "..", "🦀", &"x".repeat(1024)] {
            validate_delimiter(delimiter).expect("admitted delimiter");
        }
        for delimiter in [
            "".to_string(),
            "x".repeat(1025),
            "a\nb".to_string(),
            "\u{7f}".to_string(),
        ] {
            let error = validate_delimiter(&delimiter).expect_err("invalid delimiter");
            assert!(matches!(
                error,
                StorageError::InvalidArgument { ref argument, .. } if argument == "delimiter"
            ));
            if !delimiter.is_empty() {
                assert!(!error.to_string().contains(&delimiter));
            }
        }

        validate_continuation_token(None).unwrap();
        validate_continuation_token(Some(&"t".repeat(8192))).unwrap();
        for token in ["".to_string(), "t".repeat(8193)] {
            let error = validate_continuation_token(Some(&token)).expect_err("invalid token");
            assert!(matches!(
                error,
                StorageError::InvalidArgument { ref argument, .. }
                    if argument == "continuation_token"
            ));
            if !token.is_empty() {
                assert!(!error.to_string().contains(&token));
            }
        }
    }

    #[test]
    fn delimiter_page_accepts_mixed_and_common_prefix_only_pages() {
        validate_delimiter_list_page(
            Some("team"),
            "docs/",
            Some(3),
            [Some("team/docs/marker/"), Some("team/docs/é%25.txt")],
            [Some("team/docs/sub/")],
            false,
            None,
        )
        .expect("literal mixed page is valid");
        validate_delimiter_list_page(
            Some("team"),
            "",
            Some(1),
            std::iter::empty(),
            [Some("team/docs|")],
            true,
            Some("opaque"),
        )
        .expect("common-prefix-only page is valid");
    }

    #[test]
    fn delimiter_page_rejects_malformed_rows_whole_page() {
        let failures = [
            validate_delimiter_list_page(
                Some("team"),
                "",
                None,
                [None],
                std::iter::empty(),
                false,
                None,
            ),
            validate_delimiter_list_page(
                Some("team"),
                "",
                None,
                [Some("team-other/x")],
                std::iter::empty(),
                false,
                None,
            ),
            validate_delimiter_list_page(
                Some("team"),
                "docs/",
                None,
                [Some("team/images/x")],
                std::iter::empty(),
                false,
                None,
            ),
            validate_delimiter_list_page(
                Some("team"),
                "",
                None,
                std::iter::empty(),
                [None],
                false,
                None,
            ),
            validate_delimiter_list_page(
                Some("team"),
                "",
                None,
                std::iter::empty(),
                [Some("team/")],
                false,
                None,
            ),
            validate_delimiter_list_page(
                Some("team"),
                "",
                None,
                [Some("team/docs/")],
                [Some("team/docs/")],
                false,
                None,
            ),
            validate_delimiter_list_page(
                Some("team"),
                "",
                Some(1),
                [Some("team/a")],
                [Some("team/b/")],
                false,
                None,
            ),
            validate_delimiter_list_page(
                Some("team"),
                "",
                None,
                std::iter::empty(),
                std::iter::empty(),
                true,
                None,
            ),
            validate_delimiter_list_page(
                Some("team"),
                "",
                None,
                std::iter::empty(),
                std::iter::empty(),
                false,
                Some("token"),
            ),
        ];
        for failure in failures {
            assert!(matches!(
                failure,
                Err(StorageError::Other {
                    provider: Some(ProviderKind::S3),
                    operation: Some(StorageOperation::List),
                    ..
                })
            ));
        }
    }

    #[test]
    fn delimiter_page_enforces_default_bound_for_omitted_and_zero_max_keys() {
        for max_keys in [None, Some(0)] {
            let error = validate_delimiter_list_page(
                Some("team"),
                "",
                max_keys,
                std::iter::repeat_n(Some("team/repeated"), 1_001),
                std::iter::empty(),
                false,
                None,
            )
            .expect_err("default S3 page bound must be enforced after parse");
            assert!(matches!(error, StorageError::Other { .. }));
        }
    }

    fn list_bucket_response(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/xml\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn delimiter_page_response(is_truncated: bool, token: Option<&str>) -> String {
        let token = token
            .map(|value| format!("<NextContinuationToken>{value}</NextContinuationToken>"))
            .unwrap_or_default();
        let body = format!(
            concat!(
                "<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">",
                "<Name>bucket</Name><Prefix>docs/</Prefix><KeyCount>1</KeyCount>",
                "<MaxKeys>1</MaxKeys><Delimiter>/</Delimiter><IsTruncated>{}</IsTruncated>",
                "<Contents><Key>docs/root.txt</Key><LastModified>2026-09-21T00:00:00Z</LastModified>",
                "<ETag>&quot;etag&quot;</ETag><Size>1</Size><StorageClass>STANDARD</StorageClass></Contents>",
                "{}</ListBucketResult>"
            ),
            is_truncated, token
        );
        list_bucket_response(&body)
    }

    #[tokio::test]
    async fn delimiter_listing_sends_native_request_shape_on_first_and_continued_pages() {
        let body = concat!(
            "<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">",
            "<Name>bucket</Name><Prefix>docs/</Prefix><KeyCount>2</KeyCount>",
            "<MaxKeys>2</MaxKeys><Delimiter>/</Delimiter><IsTruncated>false</IsTruncated>",
            "<Contents><Key>docs/root.txt</Key><LastModified>2026-09-21T00:00:00Z</LastModified>",
            "<ETag>&quot;etag&quot;</ETag><Size>1</Size><StorageClass>STANDARD</StorageClass></Contents>",
            "<CommonPrefixes><Prefix>docs/sub/</Prefix></CommonPrefixes></ListBucketResult>"
        );

        for token in [None, Some("opaque+token")] {
            let (endpoint, request_task) =
                scripted_response_server(list_bucket_response(body)).await;
            let provider = scripted_provider(endpoint).await;
            let result = provider
                .list_delimited(DelimiterListRequest {
                    prefix: Some("docs/".to_string()),
                    delimiter: "/".to_string(),
                    continuation_token: token.map(ToString::to_string),
                    max_keys: Some(2),
                })
                .await
                .expect("scripted delimiter listing succeeds");
            assert_eq!(result.objects[0].path, "docs/root.txt");
            assert_eq!(result.common_prefixes, vec!["docs/sub/"]);

            let request = String::from_utf8(request_task.await.expect("request task joins"))
                .expect("request is UTF-8");
            let request_line = request.lines().next().expect("request line exists");
            assert!(request_line.contains("list-type=2"));
            assert!(request_line.contains("delimiter=%2F"));
            assert!(request_line.contains("prefix=docs%2F"));
            assert!(request_line.contains("max-keys=2"));
            assert_eq!(
                request_line.contains("continuation-token=opaque%2Btoken"),
                token.is_some()
            );
        }
    }

    #[tokio::test]
    async fn delimiter_listing_rejects_a_missing_truncation_flag() {
        let body = concat!(
            "<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">",
            "<Name>bucket</Name><Prefix>docs/</Prefix><KeyCount>1</KeyCount>",
            "<MaxKeys>2</MaxKeys><Delimiter>/</Delimiter>",
            "<Contents><Key>docs/root.txt</Key><LastModified>2026-09-21T00:00:00Z</LastModified>",
            "<ETag>&quot;etag&quot;</ETag><Size>1</Size><StorageClass>STANDARD</StorageClass></Contents>",
            "</ListBucketResult>"
        );
        let (endpoint, _request_task) = scripted_response_server(list_bucket_response(body)).await;
        let provider = scripted_provider(endpoint).await;
        let error = provider
            .list_delimited(DelimiterListRequest {
                prefix: Some("docs/".to_string()),
                delimiter: "/".to_string(),
                continuation_token: None,
                max_keys: Some(2),
            })
            .await
            .expect_err("missing truncation flag must fail the whole page");
        assert!(matches!(
            error,
            StorageError::Other {
                provider: Some(ProviderKind::S3),
                operation: Some(StorageOperation::List),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn delimiter_listing_rejects_unreplayable_provider_tokens_as_other() {
        for token in [String::new(), "t".repeat(8193)] {
            let (endpoint, request_task) =
                scripted_response_server(delimiter_page_response(true, Some(&token))).await;
            let provider = scripted_provider(endpoint).await;
            let error = provider
                .list_delimited(DelimiterListRequest {
                    prefix: Some("docs/".to_string()),
                    delimiter: "/".to_string(),
                    continuation_token: None,
                    max_keys: Some(1),
                })
                .await
                .expect_err("malformed provider token must fail the whole page");
            assert!(matches!(error, StorageError::Other { .. }));
            if !token.is_empty() {
                assert!(!error.to_string().contains(&token));
            }
            let _single_request = request_task.await.expect("request task joins");
        }
    }

    #[tokio::test]
    async fn delimiter_listing_round_trips_boundary_provider_tokens_exactly() {
        for token in ["t".to_string(), "t".repeat(8192)] {
            let responses = vec![
                delimiter_page_response(true, Some(&token)),
                delimiter_page_response(false, None),
            ];
            let (endpoint, requests_task) = scripted_owned_responses_server(responses).await;
            let provider = scripted_provider(endpoint).await;
            let first = provider
                .list_delimited(DelimiterListRequest {
                    prefix: Some("docs/".to_string()),
                    delimiter: "/".to_string(),
                    continuation_token: None,
                    max_keys: Some(1),
                })
                .await
                .expect("boundary provider token is admitted");
            assert_eq!(first.continuation_token.as_deref(), Some(token.as_str()));
            provider
                .list_delimited(DelimiterListRequest {
                    prefix: Some("docs/".to_string()),
                    delimiter: "/".to_string(),
                    continuation_token: first.continuation_token,
                    max_keys: Some(1),
                })
                .await
                .expect("returned boundary token is replayable");

            let requests = requests_task.await.expect("request task joins");
            assert_eq!(requests.len(), 2);
            for request in &requests {
                let request = std::str::from_utf8(request).expect("request is UTF-8");
                let request_line = request.lines().next().expect("request line exists");
                assert!(request_line.contains("delimiter=%2F"));
                assert!(request_line.contains("prefix=docs%2F"));
                assert!(request_line.contains("max-keys=1"));
            }
            let continued = std::str::from_utf8(&requests[1]).expect("request is UTF-8");
            assert!(continued
                .lines()
                .next()
                .expect("request line exists")
                .contains(&format!("continuation-token={token}")));
        }
    }

    #[tokio::test]
    async fn delimiter_listing_keeps_invalid_caller_tokens_as_invalid_argument() {
        let provider = scripted_provider("http://127.0.0.1:9".to_string()).await;
        for token in [String::new(), "t".repeat(8193)] {
            let error = provider
                .list_delimited(DelimiterListRequest {
                    prefix: Some("docs/".to_string()),
                    delimiter: "/".to_string(),
                    continuation_token: Some(token),
                    max_keys: Some(1),
                })
                .await
                .expect_err("invalid caller token is rejected before transport");
            assert!(matches!(
                error,
                StorageError::InvalidArgument { ref argument, .. }
                    if argument == "continuation_token"
            ));
        }
    }

    #[tokio::test]
    async fn delimiter_listing_maps_rejected_continuation_without_disclosure() {
        let response_body =
            "<Error><Code>InvalidArgument</Code><Message>bad token</Message></Error>";
        let response = format!(
            "HTTP/1.1 400 Bad Request\r\ncontent-type: application/xml\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response_body}",
            response_body.len()
        );
        let (endpoint, _request_task) = scripted_response_server(response).await;
        let provider = scripted_provider(endpoint).await;
        let sentinel = "opaque-token-sentinel";
        let error = provider
            .list_delimited(DelimiterListRequest {
                prefix: Some("docs/".to_string()),
                delimiter: "/".to_string(),
                continuation_token: Some(sentinel.to_string()),
                max_keys: Some(2),
            })
            .await
            .expect_err("provider token rejection must fail");
        assert!(matches!(
            error,
            StorageError::InvalidArgument { ref argument, .. }
                if argument == "continuation_token"
        ));
        assert!(!error.to_string().contains(sentinel));
        assert!(!format!("{error:?}").contains(sentinel));
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
            guarded_config: aws_sdk_s3::config::Builder::new()
                .behavior_version_latest()
                .region(Region::new("us-east-1"))
                .credentials_provider(Credentials::from_keys("test", "test", None))
                .build(),
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
    fn guarded_receipt_rejects_wrong_or_weak_response_identity() {
        let native = SourceSelector::NativeVersion {
            token: "version-a".to_string(),
        };
        assert!(matches!(
            checked_receipt(
                "item",
                &native,
                S3ReceiptEvidence {
                    version_id: Some("version-b"),
                    etag: Some("\"etag-a\""),
                    content_length: Some(4),
                },
                None,
                None,
                StorageOperation::GuardedGet,
            ),
            Err(StorageError::Io { .. })
        ));

        let validator = SourceSelector::ValidatorMatch {
            token: "\"etag-a\"".to_string(),
        };
        assert!(matches!(
            checked_receipt(
                "item",
                &validator,
                S3ReceiptEvidence {
                    version_id: Some("version-a"),
                    etag: Some("W/\"etag-a\""),
                    content_length: Some(4),
                },
                None,
                None,
                StorageOperation::GuardedGet,
            ),
            Err(StorageError::Io { .. })
        ));
    }

    #[test]
    fn guarded_range_parser_requires_a_valid_content_range() {
        let parsed = parse_content_range(Some("bytes 3-5/8"), StorageOperation::GuardedGetRange)
            .expect("known total range parses");
        assert_eq!(parsed.window, ByteWindow { start: 3, end: 5 });
        assert_eq!(parsed.total_size, Some(8));

        let unknown = parse_content_range(Some("bytes 3-5/*"), StorageOperation::GuardedGetRange)
            .expect("unknown total range parses");
        assert_eq!(unknown.total_size, None);

        for content_range in [None, Some("bytes 5-3/8"), Some("bytes 0-3/0")] {
            assert!(matches!(
                parse_content_range(content_range, StorageOperation::GuardedGetRange),
                Err(StorageError::Io { .. })
            ));
        }
    }

    #[tokio::test]
    async fn guarded_range_uses_one_selected_request_and_rejects_ignore_range() {
        let (endpoint, request_task) = scripted_response_server(
            "HTTP/1.1 206 Partial Content\r\ncontent-length: 1\r\ncontent-range: bytes 0-0/1\r\netag: \"etag-a\"\r\nx-amz-version-id: version-a\r\nconnection: close\r\n\r\nA",
        )
        .await;
        let provider = scripted_provider(endpoint).await;
        let selection = provider
            .bind_guarded_read(
                "object-a",
                SourceSelector::NativeVersion {
                    token: "version-a".to_string(),
                },
            )
            .expect("selection binds");
        let mut response = provider
            .guarded_get_range(GuardedRangeRequest {
                selection,
                offset: 0,
                length: 1,
            })
            .await
            .expect("matching selected range succeeds");
        let mut bytes = Vec::new();
        response
            .reader
            .read_to_end(&mut bytes)
            .await
            .expect("declared body reads");
        assert_eq!(bytes, b"A");
        assert_eq!(response.receipt.total_size, Some(1));
        assert_eq!(
            response.receipt.returned_window,
            Some(ByteWindow { start: 0, end: 0 })
        );
        let request = String::from_utf8(request_task.await.expect("server task finishes"))
            .expect("request is HTTP text")
            .to_ascii_lowercase();
        assert!(request.contains("range: bytes=0-0"));
        assert!(request.contains("versionid=version-a"));

        let (endpoint, _request_task) = scripted_response_server(
            "HTTP/1.1 200 OK\r\ncontent-length: 1\r\netag: \"etag-a\"\r\nx-amz-version-id: version-a\r\nconnection: close\r\n\r\nA",
        )
        .await;
        let provider = scripted_provider(endpoint).await;
        let selection = provider
            .bind_guarded_read(
                "object-a",
                SourceSelector::NativeVersion {
                    token: "version-a".to_string(),
                },
            )
            .expect("selection binds");
        let error = provider
            .guarded_get_range(GuardedRangeRequest {
                selection,
                offset: 0,
                length: 1,
            })
            .await
            .expect_err("an endpoint ignoring range must fail before exposing a reader");
        assert!(matches!(
            error,
            StorageError::Io {
                source,
                ..
            } if source.kind() == ErrorKind::InvalidData
        ));

        let (endpoint, _request_task) = scripted_response_server(
            "HTTP/1.1 200 OK\r\ncontent-length: 1\r\ncontent-range: bytes 0-0/2\r\netag: \"etag-a\"\r\nx-amz-version-id: version-a\r\nconnection: close\r\n\r\nA",
        )
        .await;
        let provider = scripted_provider(endpoint).await;
        let selection = provider
            .bind_guarded_read(
                "object-a",
                SourceSelector::NativeVersion {
                    token: "version-a".to_string(),
                },
            )
            .expect("selection binds");
        assert!(matches!(
            provider
                .guarded_get_range(GuardedRangeRequest {
                    selection,
                    offset: 0,
                    length: 1,
                })
                .await,
            Err(StorageError::Io { .. })
        ));

        let (endpoint, _request_task) = scripted_response_server(
            "HTTP/1.1 200 OK\r\ncontent-length: 1\r\ncontent-range: bytes 0-0/1\r\netag: \"etag-a\"\r\nx-amz-version-id: version-a\r\nconnection: close\r\n\r\nA",
        )
        .await;
        let provider = scripted_provider(endpoint).await;
        let selection = provider
            .bind_guarded_read(
                "object-a",
                SourceSelector::NativeVersion {
                    token: "version-a".to_string(),
                },
            )
            .expect("selection binds");
        provider
            .guarded_get_range(GuardedRangeRequest {
                selection,
                offset: 0,
                length: 1,
            })
            .await
            .expect("a whole-object 200 range exception is accepted");

        let (endpoint, _request_task) = scripted_response_server(
            "HTTP/1.1 206 Partial Content\r\ncontent-length: 1\r\ncontent-range: bytes 0-18446744073709551615/*\r\netag: \"etag-a\"\r\nx-amz-version-id: version-a\r\nconnection: close\r\n\r\nA",
        )
        .await;
        let provider = scripted_provider(endpoint).await;
        let selection = provider
            .bind_guarded_read(
                "object-a",
                SourceSelector::NativeVersion {
                    token: "version-a".to_string(),
                },
            )
            .expect("selection binds");
        assert!(matches!(
            provider
                .guarded_get_range(GuardedRangeRequest {
                    selection,
                    offset: 0,
                    length: 1,
                })
                .await,
            Err(StorageError::Io {
                source,
                ..
            }) if source.kind() == ErrorKind::InvalidData
        ));
    }

    #[tokio::test]
    async fn guarded_full_get_refuses_partial_responses_and_receipts_are_truthful() {
        for response in [
            "HTTP/1.1 206 Partial Content\r\ncontent-length: 1\r\ncontent-range: bytes 1-1/2\r\netag: \"etag-a\"\r\nx-amz-version-id: version-a\r\nconnection: close\r\n\r\nB",
            "HTTP/1.1 200 OK\r\ncontent-length: 1\r\ncontent-range: bytes 1-1/2\r\netag: \"etag-a\"\r\nx-amz-version-id: version-a\r\nconnection: close\r\n\r\nB",
        ] {
            let (endpoint, _request_task) = scripted_response_server(response).await;
            let provider = scripted_provider(endpoint).await;
            let selection = provider
                .bind_guarded_read(
                    "object-a",
                    SourceSelector::NativeVersion {
                        token: "version-a".to_string(),
                    },
                )
                .expect("selection binds");
            assert!(matches!(
                provider.guarded_get(selection).await,
                Err(StorageError::Io {
                    source,
                    ..
                }) if source.kind() == ErrorKind::InvalidData
            ));
        }

        let (endpoint, _request_task) = scripted_response_server(
            "HTTP/1.1 200 OK\r\ncontent-length: 2\r\netag: \"etag-a\"\r\nx-amz-version-id: version-a\r\nconnection: close\r\n\r\nAB",
        )
        .await;
        let provider = scripted_provider(endpoint).await;
        let selection = provider
            .bind_guarded_read(
                "object-a",
                SourceSelector::NativeVersion {
                    token: "version-a".to_string(),
                },
            )
            .expect("selection binds");
        let mut response = provider
            .guarded_get(selection)
            .await
            .expect("a full response succeeds");
        let mut body = Vec::new();
        response
            .reader
            .read_to_end(&mut body)
            .await
            .expect("complete body reads");
        assert_eq!(body, b"AB");
        assert_eq!(response.receipt.total_size, Some(2));
        assert_eq!(
            response.receipt.returned_window,
            Some(ByteWindow { start: 0, end: 1 })
        );

        let (endpoint, _request_task) = scripted_response_server(
            "HTTP/1.1 200 OK\r\ncontent-length: 0\r\netag: \"etag-a\"\r\nx-amz-version-id: version-a\r\nconnection: close\r\n\r\n",
        )
        .await;
        let provider = scripted_provider(endpoint).await;
        let selection = provider
            .bind_guarded_read(
                "object-a",
                SourceSelector::NativeVersion {
                    token: "version-a".to_string(),
                },
            )
            .expect("selection binds");
        let mut response = provider
            .guarded_get(selection)
            .await
            .expect("a selected empty object succeeds");
        let mut body = Vec::new();
        response
            .reader
            .read_to_end(&mut body)
            .await
            .expect("empty body completes");
        assert!(body.is_empty());
        assert_eq!(response.receipt.total_size, Some(0));
        assert_eq!(response.receipt.returned_window, None);
    }

    #[tokio::test]
    async fn guarded_range_receipt_distinguishes_requested_and_returned_windows() {
        let (endpoint, _request_task) = scripted_response_server(
            "HTTP/1.1 206 Partial Content\r\ncontent-length: 1\r\ncontent-range: bytes 1-1/2\r\netag: \"etag-a\"\r\nx-amz-version-id: version-a\r\nconnection: close\r\n\r\nB",
        )
        .await;
        let provider = scripted_provider(endpoint).await;
        let selection = provider
            .bind_guarded_read(
                "object-a",
                SourceSelector::NativeVersion {
                    token: "version-a".to_string(),
                },
            )
            .expect("selection binds");
        let mut response = provider
            .guarded_get_range(GuardedRangeRequest {
                selection,
                offset: 1,
                length: 10,
            })
            .await
            .expect("EOF clip succeeds");
        let mut body = Vec::new();
        response
            .reader
            .read_to_end(&mut body)
            .await
            .expect("clipped body completes");
        assert_eq!(body, b"B");
        assert_eq!(
            response.receipt.requested_window,
            Some(ByteWindow { start: 1, end: 10 })
        );
        assert_eq!(
            response.receipt.returned_window,
            Some(ByteWindow { start: 1, end: 1 })
        );
        assert_eq!(response.receipt.total_size, Some(2));

        let (endpoint, _request_task) = scripted_response_server(
            "HTTP/1.1 206 Partial Content\r\ncontent-length: 2\r\ncontent-range: bytes 1-2/3\r\netag: \"etag-a\"\r\nx-amz-version-id: version-a\r\nconnection: close\r\n\r\nBC",
        )
        .await;
        let provider = scripted_provider(endpoint).await;
        let selection = provider
            .bind_guarded_read(
                "object-a",
                SourceSelector::NativeVersion {
                    token: "version-a".to_string(),
                },
            )
            .expect("selection binds");
        let response = provider
            .guarded_get_range(GuardedRangeRequest {
                selection,
                offset: 1,
                length: 2,
            })
            .await
            .expect("unclipped range succeeds");
        assert_eq!(
            response.receipt.requested_window,
            Some(ByteWindow { start: 1, end: 2 })
        );
        assert_eq!(
            response.receipt.returned_window,
            response.receipt.requested_window
        );

        let (endpoint, _request_task) = scripted_response_server(
            "HTTP/1.1 206 Partial Content\r\ncontent-length: 2\r\ncontent-range: bytes 1-2/*\r\netag: \"etag-a\"\r\nx-amz-version-id: version-a\r\nconnection: close\r\n\r\nBC",
        )
        .await;
        let provider = scripted_provider(endpoint).await;
        let selection = provider
            .bind_guarded_read(
                "object-a",
                SourceSelector::NativeVersion {
                    token: "version-a".to_string(),
                },
            )
            .expect("selection binds");
        let response = provider
            .guarded_get_range(GuardedRangeRequest {
                selection,
                offset: 1,
                length: 2,
            })
            .await
            .expect("unknown-total range succeeds without clipping");
        assert_eq!(
            response.receipt.requested_window,
            Some(ByteWindow { start: 1, end: 2 })
        );
        assert_eq!(
            response.receipt.returned_window,
            response.receipt.requested_window
        );
        assert_eq!(response.receipt.total_size, None);

        let (endpoint, _request_task) = scripted_response_server(
            "HTTP/1.1 206 Partial Content\r\ncontent-length: 1\r\ncontent-range: bytes 1-1/3\r\netag: \"etag-a\"\r\nx-amz-version-id: version-a\r\nconnection: close\r\n\r\nB",
        )
        .await;
        let provider = scripted_provider(endpoint).await;
        let selection = provider
            .bind_guarded_read(
                "object-a",
                SourceSelector::NativeVersion {
                    token: "version-a".to_string(),
                },
            )
            .expect("selection binds");
        assert!(matches!(
            provider
                .guarded_get_range(GuardedRangeRequest {
                    selection,
                    offset: 1,
                    length: 2,
                })
                .await,
            Err(StorageError::Io {
                source,
                ..
            }) if source.kind() == ErrorKind::InvalidData
        ));
    }

    #[tokio::test]
    async fn guarded_validator_read_refuses_a_replaced_source_without_exposing_bytes() {
        let (endpoint, request_task) = scripted_response_server(
            "HTTP/1.1 412 Precondition Failed\r\ncontent-type: application/xml\r\ncontent-length: 72\r\nconnection: close\r\n\r\n<Error><Code>PreconditionFailed</Code><Message>changed</Message></Error>",
        )
        .await;
        let provider = scripted_provider(endpoint).await;
        let selection = provider
            .bind_guarded_read(
                "object-a",
                SourceSelector::ValidatorMatch {
                    token: "\"etag-a\"".to_string(),
                },
            )
            .expect("selection binds");
        let error = provider
            .guarded_get(selection)
            .await
            .expect_err("a replacement that fails If-Match must expose no bytes");
        assert!(matches!(
            error,
            StorageError::Conflict {
                kind: ConflictKind::TokenMismatch,
                ..
            }
        ));
        let request = String::from_utf8(request_task.await.expect("server task finishes"))
            .expect("request is HTTP text")
            .to_ascii_lowercase();
        assert!(request.contains("if-match: \"etag-a\""));
    }

    #[tokio::test]
    async fn observed_validator_refuses_a_replacement_without_exposing_bytes() {
        let (endpoint, requests_task) = scripted_responses_server(&[
            "HTTP/1.1 200 OK\r\ncontent-length: 1\r\netag: \"etag-a\"\r\nx-amz-version-id: version-a\r\nconnection: close\r\n\r\n",
            "HTTP/1.1 412 Precondition Failed\r\ncontent-type: application/xml\r\ncontent-length: 72\r\nconnection: close\r\n\r\n<Error><Code>PreconditionFailed</Code><Message>changed</Message></Error>",
        ])
        .await;
        let provider = scripted_provider(endpoint).await;
        let observation = provider
            .observe_source("object-a")
            .await
            .expect("the initial source observation succeeds");
        let validator = observation
            .receipt
            .validator
            .expect("the observation returns its strong validator");
        let selection = provider
            .bind_guarded_read(
                "object-a",
                SourceSelector::ValidatorMatch { token: validator },
            )
            .expect("the observed validator binds to the same target and key");
        let error = provider
            .guarded_get(selection)
            .await
            .expect_err("a replacement after observation must not expose bytes");
        assert!(
            matches!(
                error,
                StorageError::Conflict {
                    kind: ConflictKind::TokenMismatch,
                    ..
                }
            ),
            "unexpected guarded replacement error: {error:?}"
        );

        let requests = requests_task.await.expect("server task finishes");
        assert_eq!(requests.len(), 2);
        let observe_request = String::from_utf8(requests[0].clone())
            .expect("observation request is HTTP text")
            .to_ascii_lowercase();
        let guarded_request = String::from_utf8(requests[1].clone())
            .expect("guarded request is HTTP text")
            .to_ascii_lowercase();
        assert!(observe_request.starts_with("head "));
        assert!(guarded_request.starts_with("get "));
        assert!(guarded_request.contains("if-match: \"etag-a\""));
    }

    #[tokio::test]
    async fn source_observation_preserves_an_unknown_total_size() {
        let (endpoint, _request_task) = scripted_response_server(
            "HTTP/1.1 200 OK\r\netag: \"etag-a\"\r\nx-amz-version-id: version-a\r\nconnection: close\r\n\r\n",
        )
        .await;
        let provider = scripted_provider(endpoint).await;
        let observation = provider
            .observe_source("object-a")
            .await
            .expect("observation succeeds without a declared length");
        assert_eq!(observation.receipt.total_size, None);
        assert_eq!(
            observation.receipt.native_version.as_deref(),
            Some("version-a")
        );
    }

    #[tokio::test]
    async fn guarded_http_errors_preserve_range_and_access_classes() {
        for (response, expected_range_error) in [
            (
                "HTTP/1.1 416 Range Not Satisfiable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                true,
            ),
            (
                "HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                false,
            ),
        ] {
            let (endpoint, _request_task) = scripted_response_server(response).await;
            let provider = scripted_provider(endpoint).await;
            let selection = provider
                .bind_guarded_read(
                    "object-a",
                    SourceSelector::NativeVersion {
                        token: "version-a".to_string(),
                    },
                )
                .expect("selection binds");
            let error = provider
                .guarded_get_range(GuardedRangeRequest {
                    selection,
                    offset: 0,
                    length: 1,
                })
                .await
                .expect_err("scripted error must not expose a reader");
            if expected_range_error {
                assert!(matches!(error, StorageError::InvalidArgument { .. }));
            } else {
                assert!(matches!(error, StorageError::AccessDenied { .. }));
            }
        }
    }

    #[tokio::test]
    async fn guarded_bodyless_412_and_403_responses_keep_canonical_classes() {
        const PRECONDITION_FAILED: &str =
            "HTTP/1.1 412 Precondition Failed\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
        const ACCESS_DENIED: &str =
            "HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";

        let (endpoint, _request_task) = scripted_response_server(PRECONDITION_FAILED).await;
        let provider = scripted_provider(endpoint).await;
        let selection = provider
            .bind_guarded_read(
                "object-a",
                SourceSelector::ValidatorMatch {
                    token: "\"etag-a\"".to_string(),
                },
            )
            .expect("selection binds");
        assert!(matches!(
            provider.guarded_head(selection).await,
            Err(StorageError::Conflict {
                kind: ConflictKind::TokenMismatch,
                ..
            })
        ));

        let (endpoint, _request_task) = scripted_response_server(PRECONDITION_FAILED).await;
        let provider = scripted_provider(endpoint).await;
        let selection = provider
            .bind_guarded_read(
                "object-a",
                SourceSelector::ValidatorMatch {
                    token: "\"etag-a\"".to_string(),
                },
            )
            .expect("selection binds");
        assert!(matches!(
            provider.guarded_get(selection).await,
            Err(StorageError::Conflict {
                kind: ConflictKind::TokenMismatch,
                ..
            })
        ));

        let (endpoint, _request_task) = scripted_response_server(PRECONDITION_FAILED).await;
        let provider = scripted_provider(endpoint).await;
        let selection = provider
            .bind_guarded_read(
                "object-a",
                SourceSelector::ValidatorMatch {
                    token: "\"etag-a\"".to_string(),
                },
            )
            .expect("selection binds");
        assert!(matches!(
            provider
                .guarded_get_range(GuardedRangeRequest {
                    selection,
                    offset: 0,
                    length: 1,
                })
                .await,
            Err(StorageError::Conflict {
                kind: ConflictKind::TokenMismatch,
                ..
            })
        ));

        let (endpoint, _request_task) = scripted_response_server(ACCESS_DENIED).await;
        let provider = scripted_provider(endpoint).await;
        let selection = provider
            .bind_guarded_read(
                "object-a",
                SourceSelector::ValidatorMatch {
                    token: "\"etag-a\"".to_string(),
                },
            )
            .expect("selection binds");
        assert!(matches!(
            provider.guarded_head(selection).await,
            Err(StorageError::AccessDenied { .. })
        ));

        let (endpoint, _request_task) = scripted_response_server(ACCESS_DENIED).await;
        let provider = scripted_provider(endpoint).await;
        let selection = provider
            .bind_guarded_read(
                "object-a",
                SourceSelector::ValidatorMatch {
                    token: "\"etag-a\"".to_string(),
                },
            )
            .expect("selection binds");
        assert!(matches!(
            provider.guarded_get(selection).await,
            Err(StorageError::AccessDenied { .. })
        ));
    }

    #[tokio::test]
    async fn declared_length_reader_exposes_truncation_and_overlong_body() {
        use tokio::io::AsyncReadExt;

        let mut short = DeclaredLengthRead::new(Box::new(Cursor::new(b"abc".to_vec())), Some(5));
        let mut data = Vec::new();
        let error = short
            .read_to_end(&mut data)
            .await
            .expect_err("truncated body must remain observable");
        assert_eq!(error.kind(), ErrorKind::UnexpectedEof);

        let mut long = DeclaredLengthRead::new(Box::new(Cursor::new(b"abc".to_vec())), Some(2));
        let mut data = Vec::new();
        let error = long
            .read_to_end(&mut data)
            .await
            .expect_err("overlong body must remain observable");
        assert_eq!(error.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn guarded_error_mapping_distinguishes_precondition_and_missing_version() {
        let mismatch = map_guarded_sdk_error(
            StorageOperation::GuardedGet,
            &SourceSelector::ValidatorMatch {
                token: "\"etag-a\"".to_string(),
            },
            Some("item"),
            Some("bucket"),
            aws_sdk_s3::error::SdkError::service_error(
                GetObjectError::generic(
                    ErrorMetadata::builder().code("PreconditionFailed").build(),
                ),
                Response::new(
                    StatusCode::try_from(412).expect("valid status"),
                    SdkBody::empty(),
                ),
            ),
        );
        assert!(matches!(
            mismatch,
            StorageError::Conflict {
                kind: ConflictKind::TokenMismatch,
                ..
            }
        ));

        let missing = map_guarded_sdk_error(
            StorageOperation::GuardedGet,
            &SourceSelector::NativeVersion {
                token: "version-a".to_string(),
            },
            Some("item"),
            Some("bucket"),
            aws_sdk_s3::error::SdkError::service_error(
                GetObjectError::generic(ErrorMetadata::builder().code("NoSuchVersion").build()),
                Response::new(
                    StatusCode::try_from(404).expect("valid status"),
                    SdkBody::empty(),
                ),
            ),
        );
        assert!(matches!(missing, StorageError::NotFound { .. }));
    }

    #[test]
    fn guarded_error_mapping_handles_only_evidenced_delete_markers_and_304() {
        let selector = SourceSelector::NativeVersion {
            token: "version-a".to_string(),
        };
        let mut delete_marker_response = Response::new(
            StatusCode::try_from(405).expect("valid status"),
            SdkBody::empty(),
        );
        delete_marker_response
            .headers_mut()
            .insert("x-amz-delete-marker", "true");
        let marker = map_guarded_sdk_error(
            StorageOperation::GuardedGet,
            &selector,
            Some("item"),
            Some("bucket"),
            aws_sdk_s3::error::SdkError::service_error(
                GetObjectError::generic(ErrorMetadata::builder().code("MethodNotAllowed").build()),
                delete_marker_response,
            ),
        );
        assert!(matches!(marker, StorageError::NotFound { .. }));

        let ordinary_405 = map_guarded_sdk_error(
            StorageOperation::GuardedGet,
            &selector,
            Some("item"),
            Some("bucket"),
            aws_sdk_s3::error::SdkError::service_error(
                GetObjectError::generic(ErrorMetadata::builder().code("MethodNotAllowed").build()),
                Response::new(
                    StatusCode::try_from(405).expect("valid status"),
                    SdkBody::empty(),
                ),
            ),
        );
        assert!(!matches!(ordinary_405, StorageError::NotFound { .. }));

        let not_modified = map_guarded_sdk_error(
            StorageOperation::GuardedGet,
            &selector,
            Some("item"),
            Some("bucket"),
            aws_sdk_s3::error::SdkError::service_error(
                GetObjectError::generic(ErrorMetadata::builder().code("NotModified").build()),
                Response::new(
                    StatusCode::try_from(304).expect("valid status"),
                    SdkBody::empty(),
                ),
            ),
        );
        assert!(matches!(
            not_modified,
            StorageError::Io {
                source,
                ..
            } if source.kind() == ErrorKind::InvalidData
        ));
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
