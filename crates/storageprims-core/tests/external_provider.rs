//! Downstream-style implementations compiled against the actual exported traits.
//! Legacy implementations deliberately omit every newly defaulted hook.

use std::collections::BTreeMap;
use storageprims_core::*;

struct Legacy;
impl StorageProvider for Legacy {
    fn provider_kind(&self) -> ProviderKind {
        ProviderKind::Local
    }
    fn capabilities(&self) -> Vec<Capability> {
        vec![]
    }
    fn list(&self, _: ListOptions) -> BoxFuture<'_, ListResult> {
        unreachable!()
    }
    fn head(&self, _: &str) -> BoxFuture<'_, ObjectMetadata> {
        unreachable!()
    }
    fn get(&self, _: &str) -> BoxFuture<'_, BoxedByteStream> {
        unreachable!()
    }
    fn get_range(&self, _: GetRangeRequest) -> BoxFuture<'_, BoxedByteStream> {
        unreachable!()
    }
    fn put(&self, _: &str, _: BoxedByteStream, _: PutOptions) -> BoxFuture<'_, PutResult> {
        unreachable!()
    }
    fn delete(&self, _: &str) -> BoxFuture<'_, ()> {
        unreachable!()
    }
    fn copy(&self, _: CopyRequest) -> BoxFuture<'_, CopyResult> {
        unreachable!()
    }
}

struct LegacyDelimiter;
impl DelimiterListingProvider for LegacyDelimiter {
    // No supports_delimiter implementation: direct callability is unchanged.
    fn list_delimited(&self, _: DelimiterListRequest) -> BoxFuture<'_, DelimiterListResult> {
        Box::pin(async {
            Ok(DelimiterListResult {
                objects: vec![],
                common_prefixes: vec![],
                continuation_token: None,
                is_truncated: false,
            })
        })
    }
}

struct Prepared {
    key: String,
}
impl PreparedPut for Prepared {
    fn execute(
        self: Box<Self>,
        body: BoxedByteStream,
        _: OperationControl,
    ) -> FutureResult<'static, PutResult, PutFailure> {
        Box::pin(async move {
            let _body = body;
            Ok(PutResult {
                path: self.key,
                etag: None,
                size: Some(0),
            })
        })
    }
}

struct ExternalExtensions;
impl StorageProvider for ExternalExtensions {
    fn provider_kind(&self) -> ProviderKind {
        ProviderKind::Local
    }
    fn capabilities(&self) -> Vec<Capability> {
        vec![]
    }
    fn read_responses(&self) -> Option<&dyn ReadResponseProvider> {
        Some(self)
    }
    fn admitted_puts(&self) -> Option<&dyn AdmittedPutProvider> {
        Some(self)
    }
    fn list(&self, _: ListOptions) -> BoxFuture<'_, ListResult> {
        unreachable!()
    }
    fn head(&self, _: &str) -> BoxFuture<'_, ObjectMetadata> {
        unreachable!()
    }
    fn get(&self, _: &str) -> BoxFuture<'_, BoxedByteStream> {
        unreachable!()
    }
    fn get_range(&self, _: GetRangeRequest) -> BoxFuture<'_, BoxedByteStream> {
        unreachable!()
    }
    fn put(&self, _: &str, _: BoxedByteStream, _: PutOptions) -> BoxFuture<'_, PutResult> {
        unreachable!()
    }
    fn delete(&self, _: &str) -> BoxFuture<'_, ()> {
        unreachable!()
    }
    fn copy(&self, _: CopyRequest) -> BoxFuture<'_, CopyResult> {
        unreachable!()
    }
}
impl ReadResponseProvider for ExternalExtensions {
    fn read_support(&self) -> ReadSupport {
        ReadSupport::new(true, false, false)
    }
    fn read_response(
        &self,
        _: ReadRequest,
        _: OperationControl,
    ) -> FutureResult<'_, ReadResponse, ExtensionError> {
        Box::pin(async {
            Ok(ReadResponse::new(
                Box::new(tokio::io::empty()),
                SourceReceipt {
                    path: "empty.bin".into(),
                    native_version: None,
                    validator: None,
                    total_size: Some(0),
                    requested_window: None,
                    returned_window: None,
                },
                ReadEnforcement::Unguarded,
                ReadMetadata::new(
                    Evidence::Available(None),
                    Evidence::Available(BTreeMap::new()),
                ),
            ))
        })
    }
}
impl AdmittedPutProvider for ExternalExtensions {
    fn put_support(&self) -> PutSupport {
        PutSupport::new(
            WriteMode::SinglePart,
            true,
            true,
            false,
            false,
            false,
            u64::MAX,
        )
    }
    fn admit_put(
        &self,
        key: &str,
        _: PutOptions,
        _: WriteMode,
    ) -> std::result::Result<Box<dyn PreparedPut>, ExtensionError> {
        Ok(Box::new(Prepared { key: key.into() }))
    }
}

#[test]
fn untouched_legacy_provider_uses_real_default_hooks() {
    let provider: Box<dyn StorageProvider> = Box::new(Legacy);
    assert!(provider.read_responses().is_none());
    assert!(provider.admitted_puts().is_none());
    assert!(provider.guarded_reads().is_none());
    assert!(provider.delimiter_lists().is_none());
}

#[tokio::test]
async fn legacy_delimiter_defaults_to_unknown_without_disabling_direct_calls() {
    let provider: &dyn DelimiterListingProvider = &LegacyDelimiter;
    assert!(!provider.supports_delimiter("/"));
    let result = provider
        .list_delimited(DelimiterListRequest {
            prefix: None,
            delimiter: "/".into(),
            continuation_token: None,
            max_keys: Some(1),
        })
        .await
        .unwrap();
    assert!(!result.is_truncated);
}

#[tokio::test]
async fn external_constructors_dyn_traits_and_owned_write_future_compile_and_run() {
    fn assert_send<T: Send>(value: T) -> T {
        value
    }
    let control = OperationControl::new(
        std::time::Instant::now() + std::time::Duration::from_secs(1),
        Cancellation::new(),
    );
    let future = {
        let provider = ExternalExtensions;
        let provider_handle: &dyn StorageProvider = &provider;
        let reader = provider_handle.read_responses().unwrap();
        let response = assert_send(
            reader.read_response(ReadRequest::Current("empty.bin".into()), control.clone()),
        )
        .await
        .unwrap();
        assert_eq!(response.receipt().total_size, Some(0));
        let writer = provider_handle.admitted_puts().unwrap();
        let prepared = writer
            .admit_put("empty.bin", PutOptions::default(), WriteMode::SinglePart)
            .unwrap();
        let (body, _, _, _) = response.into_parts();
        // Return the static owned future beyond the provider's lexical lifetime.
        assert_send(prepared.execute(body, control))
    };
    assert_eq!(future.await.unwrap().path, "empty.bin");
    fn error_bounds<T: std::error::Error + Send + Sync + 'static>() {}
    error_bounds::<ExtensionError>();
    error_bounds::<PutFailure>();
}
