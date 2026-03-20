use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use storageprims_core::{
    ProviderConfig, ProviderKind, Result, StorageError, StorageOperation, StorageProvider,
};
use storageprims_s3::S3Provider;

pub(crate) struct FfiRuntime {
    runtime: tokio::runtime::Runtime,
    providers: Mutex<HashMap<u64, Arc<dyn StorageProvider>>>,
    streams: Mutex<HashMap<u64, StreamState>>,
    next_provider_id: AtomicU64,
    next_stream_id: AtomicU64,
}

pub(crate) enum StreamState {
    PendingRead {
        receiver: tokio::sync::oneshot::Receiver<Result<()>>,
        operation: StorageOperation,
    },
    PendingPut(tokio::sync::oneshot::Receiver<Result<storageprims_core::PutResult>>),
}

impl FfiRuntime {
    fn new() -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|error| StorageError::Other {
                provider: None,
                operation: Some(StorageOperation::ConfigureProvider),
                detail: format!("failed to initialize tokio runtime: {error}"),
                source: None,
            })?;

        Ok(Self {
            runtime,
            providers: Mutex::new(HashMap::new()),
            streams: Mutex::new(HashMap::new()),
            next_provider_id: AtomicU64::new(1),
            next_stream_id: AtomicU64::new(1),
        })
    }

    pub(crate) fn block_on<F, T>(&self, future: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        self.runtime.block_on(future)
    }

    pub(crate) fn spawn<F>(&self, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.runtime.spawn(future);
    }

    pub(crate) fn insert_provider(&self, provider: Arc<dyn StorageProvider>) -> u64 {
        let provider_id = self.next_provider_id.fetch_add(1, Ordering::Relaxed);
        self.providers
            .lock()
            .expect("ffi provider registry mutex poisoned")
            .insert(provider_id, provider);
        provider_id
    }

    pub(crate) fn remove_provider(&self, provider_id: u64) -> bool {
        self.providers
            .lock()
            .expect("ffi provider registry mutex poisoned")
            .remove(&provider_id)
            .is_some()
    }

    pub(crate) fn provider(&self, provider_id: u64) -> Result<Arc<dyn StorageProvider>> {
        self.providers
            .lock()
            .expect("ffi provider registry mutex poisoned")
            .get(&provider_id)
            .cloned()
            .ok_or_else(|| StorageError::InvalidArgument {
                operation: None,
                argument: "provider_id".to_string(),
                reason: format!("unknown provider id {provider_id}"),
            })
    }

    pub(crate) fn insert_stream(&self, stream: StreamState) -> u64 {
        let stream_id = self.next_stream_id.fetch_add(1, Ordering::Relaxed);
        self.streams
            .lock()
            .expect("ffi stream registry mutex poisoned")
            .insert(stream_id, stream);
        stream_id
    }

    pub(crate) fn take_stream(&self, stream_id: u64) -> Result<StreamState> {
        self.streams
            .lock()
            .expect("ffi stream registry mutex poisoned")
            .remove(&stream_id)
            .ok_or_else(|| StorageError::InvalidArgument {
                operation: Some(StorageOperation::Put),
                argument: "stream_id".to_string(),
                reason: format!("unknown stream id {stream_id}"),
            })
    }

    pub(crate) fn remove_stream(&self, stream_id: u64) -> bool {
        self.streams
            .lock()
            .expect("ffi stream registry mutex poisoned")
            .remove(&stream_id)
            .is_some()
    }
}

static NEXT_HANDLE_ID: AtomicU64 = AtomicU64::new(1);
static RUNTIMES: OnceLock<Mutex<HashMap<u64, Arc<FfiRuntime>>>> = OnceLock::new();

fn runtimes() -> &'static Mutex<HashMap<u64, Arc<FfiRuntime>>> {
    RUNTIMES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn init_runtime() -> Result<u64> {
    let handle = NEXT_HANDLE_ID.fetch_add(1, Ordering::Relaxed);
    let runtime = Arc::new(FfiRuntime::new()?);
    runtimes()
        .lock()
        .expect("ffi runtime registry mutex poisoned")
        .insert(handle, runtime);
    Ok(handle)
}

pub(crate) fn shutdown_runtime(handle: u64) -> Result<()> {
    let removed = runtimes()
        .lock()
        .expect("ffi runtime registry mutex poisoned")
        .remove(&handle);
    if removed.is_some() {
        Ok(())
    } else {
        Err(StorageError::InvalidArgument {
            operation: None,
            argument: "handle".to_string(),
            reason: format!("unknown handle {handle}"),
        })
    }
}

pub(crate) fn get_runtime(handle: u64) -> Result<Arc<FfiRuntime>> {
    runtimes()
        .lock()
        .expect("ffi runtime registry mutex poisoned")
        .get(&handle)
        .cloned()
        .ok_or_else(|| StorageError::InvalidArgument {
            operation: None,
            argument: "handle".to_string(),
            reason: format!("unknown handle {handle}"),
        })
}

pub(crate) async fn build_provider(config: ProviderConfig) -> Result<Arc<dyn StorageProvider>> {
    match config.provider {
        ProviderKind::S3 => Ok(Arc::new(S3Provider::from_config(config).await?)),
        other => Err(StorageError::InvalidArgument {
            operation: Some(StorageOperation::ConfigureProvider),
            argument: "provider".to_string(),
            reason: format!("provider {other} is not yet exposed through storageprims-ffi"),
        }),
    }
}
