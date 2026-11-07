//! Contained handle-relative local filesystem storage provider.

mod listing;
mod namespace;
#[cfg(unix)]
mod rooted;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use rooted as platform;
#[cfg(windows)]
use windows as platform;

use std::{collections::BTreeMap, io::SeekFrom, path::Path, sync::Arc, time::Instant};
use storageprims_core::{
    AdmittedPutProvider, BoxFuture, BoxedByteStream, Capability, CommitKnowledge, CopyRequest,
    CopyResult, CopyStrategy, CredentialSourceKind, DelimiterListRequest, DelimiterListResult,
    DelimiterListingProvider, Evidence, ExtensionError, ExtensionErrorKind, FutureResult,
    GetRangeRequest, ListOptions, ListResult, ObjectMetadata, OperationControl, PreparedPut,
    ProbeResult, ProbeScope, ProviderKind, PutFailure, PutOptions, PutPrecondition, PutResult,
    PutSupport, ReadEnforcement, ReadMetadata, ReadRequest, ReadResponse, ReadResponseProvider,
    ReadSupport, Result, SourceReceipt, StorageError, StorageOperation, StorageProvider, WriteMode,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

const IO_CHUNK_BYTES: usize = 64 * 1024;

#[cfg(any(unix, windows))]
const SUPPORTS_CREATE_ONLY: bool = cfg!(any(
    windows,
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
));

#[cfg(any(unix, windows))]
pub(crate) enum BaseDir {
    Empty,
    Ready(platform::DirHandle),
}

#[cfg(any(unix, windows))]
pub(crate) enum ScannedEntry {
    Excluded,
    File(String),
    Directory(String),
    Unsupported(&'static str),
}

#[cfg(any(unix, windows))]
pub(crate) enum CandidateStat {
    Missing,
    File {
        size: u64,
        last_modified: Option<String>,
    },
    NotRegular,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SyncFault {
    None = 0,
    BeforePublish = 1,
    AfterPublish = 2,
    CancelAfterPublish = 3,
}

/// Root-contained local filesystem provider.
///
/// Its `Debug` representation and error chains never disclose the configured root path.
#[derive(Clone)]
pub struct LocalProvider {
    #[cfg(any(unix, windows))]
    root: Arc<platform::Root>,
    #[cfg(test)]
    sync_fault: Arc<std::sync::atomic::AtomicU8>,
}

impl LocalProvider {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        #[cfg(any(unix, windows))]
        {
            Ok(Self {
                root: Arc::new(platform::Root::open(root.as_ref())?),
                #[cfg(test)]
                sync_fault: Arc::new(std::sync::atomic::AtomicU8::new(SyncFault::None as u8)),
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = root;
            Err(StorageError::ProviderUnavailable {
                provider: ProviderKind::Local,
                operation: StorageOperation::Probe,
                detail: "rooted backend is not implemented on this platform".into(),
            })
        }
    }

    #[cfg(all(test, any(unix, windows)))]
    pub(crate) fn set_sync_fault(&self, fault: SyncFault) {
        self.sync_fault
            .store(fault as u8, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(all(test, any(unix, windows)))]
    fn current_sync_fault(&self) -> SyncFault {
        match self.sync_fault.load(std::sync::atomic::Ordering::SeqCst) {
            1 => SyncFault::BeforePublish,
            2 => SyncFault::AfterPublish,
            3 => SyncFault::CancelAfterPublish,
            _ => SyncFault::None,
        }
    }
}

impl std::fmt::Debug for LocalProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        #[cfg(any(unix, windows))]
        let _ = &self.root;
        f.debug_struct("LocalProvider").finish_non_exhaustive()
    }
}

#[cfg(any(unix, windows))]
impl StorageProvider for LocalProvider {
    fn read_responses(&self) -> Option<&dyn ReadResponseProvider> {
        Some(self)
    }

    fn admitted_puts(&self) -> Option<&dyn AdmittedPutProvider> {
        Some(self)
    }

    fn provider_kind(&self) -> ProviderKind {
        ProviderKind::Local
    }

    fn capabilities(&self) -> Vec<Capability> {
        let mut caps = vec![Capability::DelimiterListing, Capability::CredentialProbe];
        if SUPPORTS_CREATE_ONLY {
            caps.push(Capability::ConditionalPut);
        }
        caps
    }

    fn delimiter_lists(&self) -> Option<&dyn DelimiterListingProvider> {
        Some(self)
    }

    fn list(&self, options: ListOptions) -> BoxFuture<'_, ListResult> {
        Box::pin(async move {
            let root = Arc::clone(&self.root);
            tokio::task::spawn_blocking(move || {
                let (result, _stats) =
                    listing::list_flat(&root, &options, listing::ListLimits::default())?;
                Ok(result)
            })
            .await
            .map_err(|_| join_error(StorageOperation::List))?
        })
    }

    fn head(&self, key: &str) -> BoxFuture<'_, ObjectMetadata> {
        let key = key.to_string();
        Box::pin(async move {
            let root = Arc::clone(&self.root);
            tokio::task::spawn_blocking(move || root.head_metadata(&key))
                .await
                .map_err(|_| join_error(StorageOperation::Head))?
        })
    }

    fn get(&self, key: &str) -> BoxFuture<'_, BoxedByteStream> {
        let key = key.to_string();
        Box::pin(async move {
            let root = Arc::clone(&self.root);
            let (std_file, _meta) =
                tokio::task::spawn_blocking(move || root.open_read(&key, StorageOperation::Get))
                    .await
                    .map_err(|_| join_error(StorageOperation::Get))??;
            let stream: BoxedByteStream = Box::new(tokio::fs::File::from_std(std_file));
            Ok(stream)
        })
    }

    fn get_range(&self, request: GetRangeRequest) -> BoxFuture<'_, BoxedByteStream> {
        Box::pin(async move {
            if request.length == 0 {
                return Err(StorageError::InvalidArgument {
                    operation: Some(StorageOperation::GetRange),
                    argument: "length".into(),
                    reason: "range length must be greater than zero".into(),
                });
            }
            if request.offset.checked_add(request.length - 1).is_none() {
                return Err(StorageError::InvalidArgument {
                    operation: Some(StorageOperation::GetRange),
                    argument: "length".into(),
                    reason: "range offset and length overflow byte range".into(),
                });
            }
            let root = Arc::clone(&self.root);
            let key = request.key.clone();
            let (std_file, meta) = tokio::task::spawn_blocking(move || {
                root.open_read(&key, StorageOperation::GetRange)
            })
            .await
            .map_err(|_| join_error(StorageOperation::GetRange))??;

            if request.offset >= meta.size {
                return Err(StorageError::InvalidArgument {
                    operation: Some(StorageOperation::GetRange),
                    argument: "offset".into(),
                    reason: "range offset is at or beyond object size".into(),
                });
            }

            let clamped_len = request.length.min(meta.size - request.offset);
            let mut async_file = tokio::fs::File::from_std(std_file);
            async_file
                .seek(SeekFrom::Start(request.offset))
                .await
                .map_err(|err| os_error_key(err, StorageOperation::GetRange, Some(&request.key)))?;
            let stream: BoxedByteStream = Box::new(async_file.take(clamped_len));
            Ok(stream)
        })
    }

    fn put(
        &self,
        key: &str,
        mut body: BoxedByteStream,
        options: PutOptions,
    ) -> BoxFuture<'_, PutResult> {
        let key = key.to_string();
        Box::pin(async move {
            namespace::validate(&key, StorageOperation::Put)?;
            let create_only = validate_put_options(&options)?;

            let root = Arc::clone(&self.root);
            let key_for_open = key.clone();
            let mut staged = tokio::task::spawn_blocking(move || {
                root.create_staged_put(&key_for_open, StorageOperation::Put)
            })
            .await
            .map_err(|_| join_error(StorageOperation::Put))??;

            let mut async_file = tokio::fs::File::from_std(staged.take_file());
            let mut buf = vec![0_u8; IO_CHUNK_BYTES];
            let mut written: u64 = 0;

            loop {
                let n = match body.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(err) => {
                        drop(async_file);
                        staged.cleanup_now();
                        return Err(os_error_key(err, StorageOperation::Put, Some(&key)));
                    }
                };
                let next_written = match written.checked_add(n as u64) {
                    Some(total) => total,
                    None => {
                        drop(async_file);
                        staged.cleanup_now();
                        return Err(StorageError::InvalidArgument {
                            operation: Some(StorageOperation::Put),
                            argument: "content_length".into(),
                            reason: "payload size overflowed u64".into(),
                        });
                    }
                };
                if let Some(expected) = options.content_length {
                    if next_written > expected {
                        drop(async_file);
                        staged.cleanup_now();
                        return Err(StorageError::InvalidArgument {
                            operation: Some(StorageOperation::Put),
                            argument: "content_length".into(),
                            reason: "stream exceeded declared content_length".into(),
                        });
                    }
                }
                if let Err(err) = async_file.write_all(&buf[..n]).await {
                    drop(async_file);
                    staged.cleanup_now();
                    return Err(os_error_key(err, StorageOperation::Put, Some(&key)));
                }
                written = next_written;
            }

            if let Some(expected) = options.content_length {
                if written != expected {
                    drop(async_file);
                    staged.cleanup_now();
                    return Err(StorageError::InvalidArgument {
                        operation: Some(StorageOperation::Put),
                        argument: "content_length".into(),
                        reason: "stream ended short of declared content_length".into(),
                    });
                }
            }

            if let Err(err) = async_file.flush().await {
                drop(async_file);
                staged.cleanup_now();
                return Err(os_error_key(err, StorageOperation::Put, Some(&key)));
            }
            let std_file = async_file.into_std().await;
            staged.restore_file(std_file);

            let key_for_commit = key.clone();
            tokio::task::spawn_blocking(move || -> Result<()> {
                if let Err(err) = staged.sync_temp(&key_for_commit, StorageOperation::Put) {
                    staged.cleanup_now();
                    return Err(err);
                }
                if let Err((_commit, err)) =
                    staged.publish(&key_for_commit, create_only, StorageOperation::Put)
                {
                    return Err(err);
                }
                staged.sync_parent(&key_for_commit, StorageOperation::Put)?;
                Ok(())
            })
            .await
            .map_err(|_| join_error(StorageOperation::Put))??;

            Ok(PutResult {
                path: key,
                etag: None,
                size: Some(written),
            })
        })
    }

    fn delete(&self, key: &str) -> BoxFuture<'_, ()> {
        let key = key.to_string();
        Box::pin(async move {
            let root = Arc::clone(&self.root);
            tokio::task::spawn_blocking(move || root.delete_object(&key))
                .await
                .map_err(|_| join_error(StorageOperation::Delete))?
        })
    }

    fn copy(&self, request: CopyRequest) -> BoxFuture<'_, CopyResult> {
        Box::pin(async move {
            namespace::validate_arg(&request.source, "source", StorageOperation::Copy)?;
            namespace::validate_arg(&request.destination, "destination", StorageOperation::Copy)?;

            let root_for_open = Arc::clone(&self.root);
            let src_key = request.source.clone();
            let dst_key = request.destination.clone();
            let (src_std, mut staged) = tokio::task::spawn_blocking(move || {
                let (src_file, _meta) =
                    root_for_open.open_read(&src_key, StorageOperation::Copy)?;
                let staged = root_for_open.create_staged_put(&dst_key, StorageOperation::Copy)?;
                Ok::<_, StorageError>((src_file, staged))
            })
            .await
            .map_err(|_| join_error(StorageOperation::Copy))??;

            let mut reader = tokio::fs::File::from_std(src_std);
            let mut writer = tokio::fs::File::from_std(staged.take_file());
            let mut buf = vec![0_u8; IO_CHUNK_BYTES];
            let mut copied: u64 = 0;

            loop {
                let n = match reader.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(err) => {
                        drop(writer);
                        staged.cleanup_now();
                        return Err(os_error_key(
                            err,
                            StorageOperation::Copy,
                            Some(&request.source),
                        ));
                    }
                };
                if let Err(err) = writer.write_all(&buf[..n]).await {
                    drop(writer);
                    staged.cleanup_now();
                    return Err(os_error_key(
                        err,
                        StorageOperation::Copy,
                        Some(&request.destination),
                    ));
                }
                copied = copied.saturating_add(n as u64);
            }

            if let Err(err) = writer.flush().await {
                drop(writer);
                staged.cleanup_now();
                return Err(os_error_key(
                    err,
                    StorageOperation::Copy,
                    Some(&request.destination),
                ));
            }
            // Close the source handle before atomic destination publication so self-copy
            // and non-POSIX replacement fallbacks never contend with an open reader.
            let _ = reader.into_std().await;
            staged.restore_file(writer.into_std().await);

            let dst_for_commit = request.destination.clone();
            tokio::task::spawn_blocking(move || -> Result<()> {
                if let Err(err) = staged.sync_temp(&dst_for_commit, StorageOperation::Copy) {
                    staged.cleanup_now();
                    return Err(err);
                }
                if let Err((_commit, err)) =
                    staged.publish(&dst_for_commit, false, StorageOperation::Copy)
                {
                    return Err(err);
                }
                staged.sync_parent(&dst_for_commit, StorageOperation::Copy)?;
                Ok(())
            })
            .await
            .map_err(|_| join_error(StorageOperation::Copy))??;

            Ok(CopyResult {
                source: request.source,
                destination: request.destination,
                strategy: CopyStrategy::Native,
                bytes_copied: Some(copied),
                destination_etag: None,
            })
        })
    }

    fn probe(&self) -> BoxFuture<'_, ProbeResult> {
        Box::pin(async move {
            let started = Instant::now();
            let root = Arc::clone(&self.root);
            let capabilities = self.capabilities();
            tokio::task::spawn_blocking(move || root.probe_root())
                .await
                .map_err(|_| join_error(StorageOperation::Probe))??;
            Ok(ProbeResult {
                provider: ProviderKind::Local,
                endpoint: None,
                credential_source: CredentialSourceKind::None,
                scope: ProbeScope::ConfiguredContainer,
                probe_method: "local:RootHandleStat".into(),
                latency_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                capabilities,
            })
        })
    }
}

#[cfg(any(unix, windows))]
impl DelimiterListingProvider for LocalProvider {
    fn supports_delimiter(&self, delimiter: &str) -> bool {
        delimiter == "/"
    }

    fn list_delimited(&self, request: DelimiterListRequest) -> BoxFuture<'_, DelimiterListResult> {
        Box::pin(async move {
            if request.delimiter != "/" {
                return Err(StorageError::InvalidArgument {
                    operation: Some(StorageOperation::List),
                    argument: "delimiter".into(),
                    reason: "local delimiter listing supports only '/'".into(),
                });
            }
            let root = Arc::clone(&self.root);
            tokio::task::spawn_blocking(move || {
                let (result, _stats) =
                    listing::list_delimited(&root, &request, listing::ListLimits::default())?;
                Ok(result)
            })
            .await
            .map_err(|_| join_error(StorageOperation::List))?
        })
    }
}

#[cfg(any(unix, windows))]
impl ReadResponseProvider for LocalProvider {
    fn read_support(&self) -> ReadSupport {
        ReadSupport::new(true, false, false)
    }

    fn read_response(
        &self,
        request: ReadRequest,
        control: OperationControl,
    ) -> FutureResult<'_, ReadResponse, ExtensionError> {
        Box::pin(async move {
            let key = match request {
                ReadRequest::Current(key) => key,
                ReadRequest::Guarded(_) => {
                    return Err(ExtensionError::new(
                        ExtensionErrorKind::UnsupportedReadSelection,
                        None,
                    ));
                }
                _ => {
                    return Err(ExtensionError::new(
                        ExtensionErrorKind::UnsupportedReadSelection,
                        None,
                    ));
                }
            };

            if let Some(kind) = check_control(&control) {
                return Err(ExtensionError::new(kind, None));
            }
            if let Err(err) = namespace::validate(&key, StorageOperation::Get) {
                return Err(ExtensionError::new(
                    ExtensionErrorKind::InvalidRequest,
                    Some(err),
                ));
            }

            let root = Arc::clone(&self.root);
            let key_for_open = key.clone();
            let opened = tokio::task::spawn_blocking(move || {
                root.open_read(&key_for_open, StorageOperation::Get)
            })
            .await
            .map_err(|_| {
                ExtensionError::new(
                    ExtensionErrorKind::Provider,
                    Some(join_error(StorageOperation::Get)),
                )
            })?;

            let (std_file, meta) = match opened {
                Ok(pair) => pair,
                Err(err) => {
                    let kind = check_control(&control).unwrap_or(ExtensionErrorKind::Provider);
                    return Err(ExtensionError::new(kind, Some(err)));
                }
            };

            if let Some(kind) = check_control(&control) {
                return Err(ExtensionError::new(kind, None));
            }

            let receipt = SourceReceipt {
                path: key,
                native_version: None,
                validator: None,
                total_size: Some(meta.size),
                requested_window: None,
                returned_window: None,
            };
            let metadata = ReadMetadata::new(
                Evidence::Available(None),
                Evidence::Available(BTreeMap::new()),
            );
            let reader: BoxedByteStream = Box::new(tokio::fs::File::from_std(std_file));
            Ok(ReadResponse::new(
                reader,
                receipt,
                ReadEnforcement::Unguarded,
                metadata,
            ))
        })
    }
}

#[cfg(any(unix, windows))]
struct LocalPreparedPut {
    root: Arc<platform::Root>,
    key: String,
    expected_length: u64,
    create_only: bool,
    #[cfg(test)]
    sync_fault: SyncFault,
}

#[cfg(any(unix, windows))]
impl PreparedPut for LocalPreparedPut {
    fn execute(
        self: Box<Self>,
        mut body: BoxedByteStream,
        control: OperationControl,
    ) -> FutureResult<'static, PutResult, PutFailure> {
        Box::pin(async move {
            let Self {
                root,
                key,
                expected_length,
                create_only,
                #[cfg(test)]
                sync_fault,
            } = *self;

            if let Some(kind) = check_control(&control) {
                return Err(PutFailure::new(
                    CommitKnowledge::NotStarted,
                    ExtensionError::new(kind, None),
                ));
            }

            let root_for_stage = Arc::clone(&root);
            let key_for_stage = key.clone();
            let staged_res = tokio::task::spawn_blocking(move || {
                root_for_stage.create_staged_put(&key_for_stage, StorageOperation::Put)
            })
            .await
            .map_err(|_| {
                PutFailure::new(
                    CommitKnowledge::NotCommitted,
                    ExtensionError::new(
                        ExtensionErrorKind::Provider,
                        Some(join_error(StorageOperation::Put)),
                    ),
                )
            })?;

            let mut staged = match staged_res {
                Ok(staged) => staged,
                Err(err) => {
                    let kind = check_control(&control).unwrap_or(ExtensionErrorKind::Provider);
                    return Err(PutFailure::new(
                        CommitKnowledge::NotCommitted,
                        ExtensionError::new(kind, Some(err)),
                    ));
                }
            };

            let mut async_file = tokio::fs::File::from_std(staged.take_file());
            let mut buf = vec![0_u8; IO_CHUNK_BYTES];
            let mut written: u64 = 0;

            loop {
                if let Some(kind) = check_control(&control) {
                    drop(async_file);
                    staged.cleanup_now();
                    return Err(PutFailure::new(
                        CommitKnowledge::NotCommitted,
                        ExtensionError::new(kind, None),
                    ));
                }

                let read_outcome = tokio::select! {
                    biased;
                    _ = control.cancellation().cancelled() => {
                        drop(async_file);
                        staged.cleanup_now();
                        return Err(PutFailure::new(
                            CommitKnowledge::NotCommitted,
                            ExtensionError::new(ExtensionErrorKind::Cancelled, None),
                        ));
                    }
                    _ = tokio::time::sleep_until(control.deadline().into()) => {
                        drop(async_file);
                        staged.cleanup_now();
                        return Err(PutFailure::new(
                            CommitKnowledge::NotCommitted,
                            ExtensionError::new(ExtensionErrorKind::Deadline, None),
                        ));
                    }
                    res = body.read(&mut buf) => res,
                };

                let n = match read_outcome {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(err) => {
                        drop(async_file);
                        staged.cleanup_now();
                        let kind = check_control(&control).unwrap_or(ExtensionErrorKind::Provider);
                        return Err(PutFailure::new(
                            CommitKnowledge::NotCommitted,
                            ExtensionError::new(
                                kind,
                                Some(os_error_key(err, StorageOperation::Put, Some(&key))),
                            ),
                        ));
                    }
                };

                let next_written = match written.checked_add(n as u64) {
                    Some(total) => total,
                    None => {
                        drop(async_file);
                        staged.cleanup_now();
                        return Err(PutFailure::new(
                            CommitKnowledge::NotCommitted,
                            ExtensionError::new(
                                ExtensionErrorKind::InvalidRequest,
                                Some(StorageError::InvalidArgument {
                                    operation: Some(StorageOperation::Put),
                                    argument: "content_length".into(),
                                    reason: "stream overflowed u64".into(),
                                }),
                            ),
                        ));
                    }
                };
                if next_written > expected_length {
                    drop(async_file);
                    staged.cleanup_now();
                    return Err(PutFailure::new(
                        CommitKnowledge::NotCommitted,
                        ExtensionError::new(
                            ExtensionErrorKind::InvalidRequest,
                            Some(StorageError::InvalidArgument {
                                operation: Some(StorageOperation::Put),
                                argument: "content_length".into(),
                                reason: "stream exceeded admitted content_length".into(),
                            }),
                        ),
                    ));
                }

                if let Err(err) = async_file.write_all(&buf[..n]).await {
                    drop(async_file);
                    staged.cleanup_now();
                    let kind = check_control(&control).unwrap_or(ExtensionErrorKind::Provider);
                    return Err(PutFailure::new(
                        CommitKnowledge::NotCommitted,
                        ExtensionError::new(
                            kind,
                            Some(os_error_key(err, StorageOperation::Put, Some(&key))),
                        ),
                    ));
                }
                written = next_written;
            }

            if written != expected_length {
                drop(async_file);
                staged.cleanup_now();
                return Err(PutFailure::new(
                    CommitKnowledge::NotCommitted,
                    ExtensionError::new(
                        ExtensionErrorKind::InvalidRequest,
                        Some(StorageError::InvalidArgument {
                            operation: Some(StorageOperation::Put),
                            argument: "content_length".into(),
                            reason: "stream ended short of admitted content_length".into(),
                        }),
                    ),
                ));
            }

            if let Err(err) = async_file.flush().await {
                drop(async_file);
                staged.cleanup_now();
                let kind = check_control(&control).unwrap_or(ExtensionErrorKind::Provider);
                return Err(PutFailure::new(
                    CommitKnowledge::NotCommitted,
                    ExtensionError::new(
                        kind,
                        Some(os_error_key(err, StorageOperation::Put, Some(&key))),
                    ),
                ));
            }
            staged.restore_file(async_file.into_std().await);

            if let Some(kind) = check_control(&control) {
                staged.cleanup_now();
                return Err(PutFailure::new(
                    CommitKnowledge::NotCommitted,
                    ExtensionError::new(kind, None),
                ));
            }

            let key_for_commit = key.clone();
            let control_for_commit = control.clone();
            let commit_outcome =
                tokio::task::spawn_blocking(move || -> std::result::Result<(), PutFailure> {
                    #[cfg(test)]
                    if sync_fault == SyncFault::BeforePublish {
                        staged.cleanup_now();
                        return Err(PutFailure::new(
                            CommitKnowledge::NotCommitted,
                            ExtensionError::new(
                                ExtensionErrorKind::Provider,
                                Some(StorageError::Io {
                                    operation: Some(StorageOperation::Put),
                                    source: std::io::Error::other(
                                        "injected pre-publish sync failure",
                                    ),
                                }),
                            ),
                        ));
                    }

                    if let Err(err) = staged.sync_temp(&key_for_commit, StorageOperation::Put) {
                        staged.cleanup_now();
                        let kind = check_control(&control_for_commit)
                            .unwrap_or(ExtensionErrorKind::Provider);
                        return Err(PutFailure::new(
                            CommitKnowledge::NotCommitted,
                            ExtensionError::new(kind, Some(err)),
                        ));
                    }

                    if let Some(kind) = check_control(&control_for_commit) {
                        staged.cleanup_now();
                        return Err(PutFailure::new(
                            CommitKnowledge::NotCommitted,
                            ExtensionError::new(kind, None),
                        ));
                    }

                    if let Err((commit, err)) =
                        staged.publish(&key_for_commit, create_only, StorageOperation::Put)
                    {
                        return Err(PutFailure::new(
                            commit,
                            ExtensionError::new(ExtensionErrorKind::Provider, Some(err)),
                        ));
                    }

                    // Publication has succeeded atomically: any subsequent failure or control
                    // expiry MUST report CommitKnowledge::Committed and never delete destination.
                    #[cfg(test)]
                    match sync_fault {
                        SyncFault::AfterPublish => {
                            return Err(PutFailure::new(
                                CommitKnowledge::Committed,
                                ExtensionError::new(
                                    ExtensionErrorKind::Provider,
                                    Some(StorageError::Io {
                                        operation: Some(StorageOperation::Put),
                                        source: std::io::Error::other(
                                            "injected post-publish parent sync failure",
                                        ),
                                    }),
                                ),
                            ));
                        }
                        SyncFault::CancelAfterPublish => {
                            control_for_commit.cancellation().cancel();
                        }
                        SyncFault::None | SyncFault::BeforePublish => {}
                    }

                    if let Err(err) = staged.sync_parent(&key_for_commit, StorageOperation::Put) {
                        return Err(PutFailure::new(
                            CommitKnowledge::Committed,
                            ExtensionError::new(ExtensionErrorKind::Provider, Some(err)),
                        ));
                    }

                    if let Some(kind) = check_control(&control_for_commit) {
                        return Err(PutFailure::new(
                            CommitKnowledge::Committed,
                            ExtensionError::new(kind, None),
                        ));
                    }

                    Ok(())
                })
                .await
                .map_err(|_| {
                    PutFailure::new(
                        CommitKnowledge::Unknown,
                        ExtensionError::new(
                            ExtensionErrorKind::Provider,
                            Some(join_error(StorageOperation::Put)),
                        ),
                    )
                })?;

            commit_outcome?;

            if let Some(kind) = check_control(&control) {
                return Err(PutFailure::new(
                    CommitKnowledge::Committed,
                    ExtensionError::new(kind, None),
                ));
            }

            Ok(PutResult {
                path: key,
                etag: None,
                size: Some(written),
            })
        })
    }
}

#[cfg(any(unix, windows))]
impl AdmittedPutProvider for LocalProvider {
    fn put_support(&self) -> PutSupport {
        PutSupport::new(
            WriteMode::SinglePart,
            true,
            SUPPORTS_CREATE_ONLY,
            false,
            false,
            false,
            u64::MAX,
        )
    }

    fn admit_put(
        &self,
        key: &str,
        options: PutOptions,
        mode: WriteMode,
    ) -> std::result::Result<Box<dyn PreparedPut>, ExtensionError> {
        match mode {
            WriteMode::SinglePart => {}
            _ => {
                return Err(ExtensionError::new(
                    ExtensionErrorKind::UnsupportedWriteMode,
                    None,
                ));
            }
        }

        if let Err(err) = namespace::validate(key, StorageOperation::Put) {
            return Err(ExtensionError::new(
                ExtensionErrorKind::InvalidRequest,
                Some(err),
            ));
        }

        let create_only = match &options.precondition {
            PutPrecondition::None => false,
            PutPrecondition::MustNotExist => {
                if !SUPPORTS_CREATE_ONLY {
                    return Err(ExtensionError::new(
                        ExtensionErrorKind::UnsupportedPredicate,
                        Some(StorageError::UnsupportedCapability {
                            provider: ProviderKind::Local,
                            operation: StorageOperation::Put,
                            capability: Capability::ConditionalPut,
                        }),
                    ));
                }
                true
            }
            PutPrecondition::Match { .. } => {
                return Err(ExtensionError::new(
                    ExtensionErrorKind::UnsupportedPredicate,
                    Some(StorageError::InvalidArgument {
                        operation: Some(StorageOperation::Put),
                        argument: "precondition".into(),
                        reason: "local provider does not support validator-match preconditions"
                            .into(),
                    }),
                ));
            }
        };

        if options.content_type.is_some() || !options.metadata.is_empty() {
            let argument = if options.content_type.is_some() {
                "content_type"
            } else {
                "metadata"
            };
            return Err(ExtensionError::new(
                ExtensionErrorKind::UnsupportedMetadata,
                Some(StorageError::InvalidArgument {
                    operation: Some(StorageOperation::Put),
                    argument: argument.into(),
                    reason: "local provider does not persist object metadata".into(),
                }),
            ));
        }

        let Some(expected_length) = options.content_length else {
            return Err(ExtensionError::new(
                ExtensionErrorKind::LengthRequired,
                Some(StorageError::InvalidArgument {
                    operation: Some(StorageOperation::Put),
                    argument: "content_length".into(),
                    reason: "admitted local write requires an explicit content_length".into(),
                }),
            ));
        };

        Ok(Box::new(LocalPreparedPut {
            root: Arc::clone(&self.root),
            key: key.to_string(),
            expected_length,
            create_only,
            #[cfg(test)]
            sync_fault: self.current_sync_fault(),
        }))
    }
}

#[cfg(any(unix, windows))]
fn validate_put_options(options: &PutOptions) -> Result<bool> {
    if options.content_type.is_some() {
        return Err(StorageError::InvalidArgument {
            operation: Some(StorageOperation::Put),
            argument: "content_type".into(),
            reason: "local provider does not persist content_type".into(),
        });
    }
    if !options.metadata.is_empty() {
        return Err(StorageError::InvalidArgument {
            operation: Some(StorageOperation::Put),
            argument: "metadata".into(),
            reason: "local provider does not persist user metadata".into(),
        });
    }
    match &options.precondition {
        PutPrecondition::None => Ok(false),
        PutPrecondition::MustNotExist => {
            if !SUPPORTS_CREATE_ONLY {
                return Err(StorageError::UnsupportedCapability {
                    provider: ProviderKind::Local,
                    operation: StorageOperation::Put,
                    capability: Capability::ConditionalPut,
                });
            }
            Ok(true)
        }
        PutPrecondition::Match { .. } => Err(StorageError::InvalidArgument {
            operation: Some(StorageOperation::Put),
            argument: "precondition".into(),
            reason: "local provider does not support validator-match preconditions".into(),
        }),
    }
}

#[cfg(any(unix, windows))]
fn check_control(control: &OperationControl) -> Option<ExtensionErrorKind> {
    if control.cancellation().is_cancelled() {
        Some(ExtensionErrorKind::Cancelled)
    } else if Instant::now() >= control.deadline() {
        Some(ExtensionErrorKind::Deadline)
    } else {
        None
    }
}

#[cfg(any(unix, windows))]
fn join_error(operation: StorageOperation) -> StorageError {
    StorageError::Io {
        operation: Some(operation),
        source: std::io::Error::other("contained filesystem worker cancelled"),
    }
}

/// Preserve the OS error kind/code without retaining caller paths or OS strings.
#[cfg(any(unix, windows))]
pub(crate) fn os_error(error: std::io::Error, operation: StorageOperation) -> StorageError {
    os_error_key(error, operation, None)
}

#[cfg(any(unix, windows))]
pub(crate) fn os_error_key(
    error: std::io::Error,
    operation: StorageOperation,
    validated_key: Option<&str>,
) -> StorageError {
    match error.kind() {
        std::io::ErrorKind::NotFound => StorageError::NotFound {
            provider: ProviderKind::Local,
            operation,
            path: validated_key.unwrap_or("[relative object]").into(),
        },
        std::io::ErrorKind::PermissionDenied => StorageError::AccessDenied {
            provider: ProviderKind::Local,
            operation,
            target: validated_key.map(ToOwned::to_owned),
            detail: "contained filesystem access denied".into(),
        },
        _ => StorageError::Io {
            operation: Some(operation),
            source: error
                .raw_os_error()
                .map(std::io::Error::from_raw_os_error)
                .unwrap_or_else(|| {
                    std::io::Error::new(error.kind(), "contained filesystem operation failed")
                }),
        },
    }
}

#[cfg(any(unix, windows))]
pub(crate) fn non_regular_object_error(operation: StorageOperation) -> StorageError {
    StorageError::InvalidArgument {
        operation: Some(operation),
        argument: "key".into(),
        reason: "local byte objects must be regular files".into(),
    }
}

#[cfg(any(unix, windows))]
pub(crate) fn symlink_refused_error(operation: StorageOperation) -> StorageError {
    StorageError::Io {
        operation: Some(operation),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "contained symlink or reparse point refused",
        ),
    }
}

#[cfg(all(test, any(unix, windows)))]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };
    use storageprims_core::{Cancellation, InspectionKind, InspectionLimitDimension};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    struct TempRoot(std::path::PathBuf);
    impl TempRoot {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "storageprims-local-unit-{}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn post_publish_sync_and_cancel_faults_report_committed_and_preserve_object() {
        let root_dir = TempRoot::new();
        let provider = LocalProvider::new(&root_dir.0).unwrap();

        // Pre-publish sync failure cleans up staging file and reports NotCommitted.
        provider.set_sync_fault(SyncFault::BeforePublish);
        let prep_before = provider
            .admit_put(
                "before.bin",
                PutOptions {
                    content_length: Some(5),
                    ..PutOptions::default()
                },
                WriteMode::SinglePart,
            )
            .unwrap();
        let ctrl =
            OperationControl::new(Instant::now() + Duration::from_secs(5), Cancellation::new());
        let fail_before = prep_before
            .execute(Box::new(std::io::Cursor::new(b"alpha".to_vec())), ctrl)
            .await
            .unwrap_err();
        assert_eq!(fail_before.commit(), CommitKnowledge::NotCommitted);
        assert!(!root_dir.0.join("before.bin").exists());

        // Post-publish parent sync failure reports Committed and preserves destination.
        provider.set_sync_fault(SyncFault::AfterPublish);
        let prep_after = provider
            .admit_put(
                "after.bin",
                PutOptions {
                    content_length: Some(4),
                    ..PutOptions::default()
                },
                WriteMode::SinglePart,
            )
            .unwrap();
        let ctrl2 =
            OperationControl::new(Instant::now() + Duration::from_secs(5), Cancellation::new());
        let fail_after = prep_after
            .execute(Box::new(std::io::Cursor::new(b"beta".to_vec())), ctrl2)
            .await
            .unwrap_err();
        assert_eq!(fail_after.commit(), CommitKnowledge::Committed);
        assert_eq!(
            std::fs::read(root_dir.0.join("after.bin")).unwrap(),
            b"beta"
        );

        // Cancellation after atomic publish reports Committed with Cancelled kind.
        provider.set_sync_fault(SyncFault::CancelAfterPublish);
        let prep_cancel = provider
            .admit_put(
                "cancel-after.bin",
                PutOptions {
                    content_length: Some(5),
                    ..PutOptions::default()
                },
                WriteMode::SinglePart,
            )
            .unwrap();
        let ctrl3 =
            OperationControl::new(Instant::now() + Duration::from_secs(5), Cancellation::new());
        let fail_cancel = prep_cancel
            .execute(Box::new(std::io::Cursor::new(b"gamma".to_vec())), ctrl3)
            .await
            .unwrap_err();
        assert_eq!(fail_cancel.commit(), CommitKnowledge::Committed);
        assert_eq!(fail_cancel.error().kind(), ExtensionErrorKind::Cancelled);
        assert_eq!(
            std::fs::read(root_dir.0.join("cancel-after.bin")).unwrap(),
            b"gamma"
        );
    }

    #[test]
    fn exact_entry_budget_boundary_and_window_slot_cap() {
        let root_dir = TempRoot::new();
        std::fs::write(root_dir.0.join("a"), "1").unwrap();
        std::fs::write(root_dir.0.join("b"), "2").unwrap();
        let provider = LocalProvider::new(&root_dir.0).unwrap();

        // 2 entries with max_entries = 2 succeeds at exact boundary.
        let (res, stats) = listing::list_flat(
            &provider.root,
            &ListOptions {
                max_keys: Some(2),
                ..ListOptions::default()
            },
            listing::ListLimits {
                max_entries: 2,
                max_duration: Duration::from_secs(10),
            },
        )
        .unwrap();
        assert!(!res.is_truncated);
        assert_eq!(res.objects.len(), 2);
        assert_eq!(stats.examined, 2);
        assert!(stats.peak_slots <= 3);

        // 3 entries with max_entries = 2 fails on 3rd entry with consumed = 3, configured_limit = 2.
        std::fs::write(root_dir.0.join("c"), "3").unwrap();
        let err = listing::list_flat(
            &provider.root,
            &ListOptions {
                max_keys: Some(2),
                ..ListOptions::default()
            },
            listing::ListLimits {
                max_entries: 2,
                max_duration: Duration::from_secs(10),
            },
        )
        .unwrap_err();
        match err {
            StorageError::Inspection {
                provider: Some(ProviderKind::Local),
                operation: StorageOperation::List,
                kind: InspectionKind::LimitExceeded,
                limit_dimension: Some(InspectionLimitDimension::Entries),
                configured_limit: 2,
                consumed: 3,
            } => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
