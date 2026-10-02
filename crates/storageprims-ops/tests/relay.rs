//! Synthetic whole-dispatch relay acceptance. No production SDK/native claims.
use std::{
    collections::BTreeMap,
    io,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
    time::Duration,
};
use storageprims_core::*;
use storageprims_ops::{
    relay_copy, MetadataPolicy, RelayRequest, TransferBudget, TransferFailureKind,
};
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};

#[derive(Default)]
struct Counts {
    max_read_capacity: AtomicUsize,
    open: AtomicUsize,
    poll: AtomicUsize,
    admit: AtomicUsize,
    execute: AtomicUsize,
    dropped: AtomicUsize,
    started: tokio::sync::Notify,
    read_started: tokio::sync::Notify,
    options: std::sync::Mutex<Option<PutOptions>>,
}
#[derive(Clone, Copy)]
enum Write {
    Exact,
    Early,
    Lost,
    CommittedError,
    Conflict,
    Pending,
    CancelThenAck,
    LateAck,
    PartialError,
    IgnoreBodyError,
}
struct Fake {
    bytes: Vec<u8>,
    length: Option<u64>,
    write: Write,
    counts: Arc<Counts>,
    source_error_at: Option<usize>,
    poll_delay: Duration,
    poll_barrier: Option<Arc<std::sync::Barrier>>,
    metadata: bool,
    match_supported: bool,
    maximum: u64,
    read_hook: bool,
    guarded: bool,
    write_hook: bool,
    open_error: bool,
    admit_error: bool,
    open_delay: Duration,
    admit_delay: Duration,
    cancel_open: bool,
    cancel_admit: Option<Cancellation>,
    pending_read: bool,
    metadata_values: bool,
    metadata_supported: bool,
}
impl Fake {
    fn new(bytes: &[u8], length: u64) -> Self {
        Self {
            bytes: bytes.into(),
            length: Some(length),
            write: Write::Exact,
            counts: Arc::default(),
            source_error_at: None,
            poll_delay: Duration::ZERO,
            poll_barrier: None,
            metadata: true,
            match_supported: false,
            maximum: 100,
            read_hook: true,
            guarded: false,
            write_hook: true,
            open_error: false,
            admit_error: false,
            open_delay: Duration::ZERO,
            admit_delay: Duration::ZERO,
            cancel_open: false,
            cancel_admit: None,
            pending_read: false,
            metadata_values: false,
            metadata_supported: true,
        }
    }
}
impl StorageProvider for Fake {
    fn provider_kind(&self) -> ProviderKind {
        ProviderKind::Local
    }
    fn capabilities(&self) -> Vec<Capability> {
        vec![]
    }
    fn read_responses(&self) -> Option<&dyn ReadResponseProvider> {
        self.read_hook.then_some(self)
    }
    fn admitted_puts(&self) -> Option<&dyn AdmittedPutProvider> {
        self.write_hook.then_some(self)
    }
    fn list(&self, _: ListOptions) -> BoxFuture<'_, ListResult> {
        panic!("unexpected list")
    }
    fn head(&self, _: &str) -> BoxFuture<'_, ObjectMetadata> {
        panic!("unexpected HEAD")
    }
    fn get(&self, _: &str) -> BoxFuture<'_, BoxedByteStream> {
        panic!("plain get downgrade")
    }
    fn get_range(&self, _: GetRangeRequest) -> BoxFuture<'_, BoxedByteStream> {
        panic!("unexpected range")
    }
    fn put(&self, _: &str, _: BoxedByteStream, _: PutOptions) -> BoxFuture<'_, PutResult> {
        panic!("plain put downgrade")
    }
    fn delete(&self, _: &str) -> BoxFuture<'_, ()> {
        panic!("compensating delete")
    }
    fn copy(&self, _: CopyRequest) -> BoxFuture<'_, CopyResult> {
        panic!("native strategy substitution")
    }
}
impl GuardedReadProvider for Fake {
    fn guarded_read_target_identity(&self) -> String {
        "synthetic-source".into()
    }
    fn observe_source(&self, _: &str) -> BoxFuture<'_, SourceObservation> {
        panic!("extra source observation")
    }
    fn guarded_head(&self, _: GuardedReadSelection) -> BoxFuture<'_, SourceReceipt> {
        panic!("extra guarded HEAD")
    }
    fn guarded_get(&self, _: GuardedReadSelection) -> BoxFuture<'_, GuardedReadResponse> {
        panic!("legacy guarded get substitution")
    }
    fn guarded_get_range(&self, _: GuardedRangeRequest) -> BoxFuture<'_, GuardedReadResponse> {
        panic!("range substitution")
    }
}
struct Body {
    pending: bool,
    delay: Duration,
    barrier: Option<Arc<std::sync::Barrier>>,
    bytes: Vec<u8>,
    offset: usize,
    error_at: Option<usize>,
    counts: Arc<Counts>,
}
impl Drop for Body {
    fn drop(&mut self) {
        self.counts.dropped.fetch_add(1, Ordering::SeqCst);
    }
}
impl AsyncRead for Body {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.pending {
            this.counts.read_started.notify_one();
            return Poll::Pending;
        }
        if let Some(barrier) = this.barrier.take() {
            barrier.wait();
        }
        if !this.delay.is_zero() {
            std::thread::sleep(std::mem::take(&mut this.delay));
        }
        this.counts.poll.fetch_add(1, Ordering::SeqCst);
        this.counts
            .max_read_capacity
            .fetch_max(out.remaining(), Ordering::SeqCst);
        if this.error_at.is_some_and(|at| this.offset >= at) {
            return Poll::Ready(Err(io::Error::other("synthetic source body fault")));
        }
        let count = out
            .remaining()
            .min(this.bytes.len() - this.offset)
            .min(this.error_at.map_or(usize::MAX, |at| at - this.offset));
        out.put_slice(&this.bytes[this.offset..this.offset + count]);
        this.offset += count;
        Poll::Ready(Ok(()))
    }
}
impl ReadResponseProvider for Fake {
    fn read_support(&self) -> ReadSupport {
        ReadSupport::new(true, self.guarded, self.guarded)
    }
    fn read_response(
        &self,
        request: ReadRequest,
        control: OperationControl,
    ) -> FutureResult<'_, ReadResponse, ExtensionError> {
        self.counts.open.fetch_add(1, Ordering::SeqCst);
        if self.cancel_open {
            control.cancellation().cancel();
        }
        let (enforcement, native_version, validator) = match request {
            ReadRequest::Current(_) => (ReadEnforcement::Unguarded, None, None),
            ReadRequest::Guarded(selection) => {
                if let Err(error) =
                    selection.validate_for("synthetic-source", "sample.bin", StorageOperation::Get)
                {
                    return Box::pin(async {
                        Err(ExtensionError::new(
                            ExtensionErrorKind::InvalidRequest,
                            Some(error),
                        ))
                    });
                }
                match selection.selector() {
                    SourceSelector::NativeVersion { token } => {
                        (ReadEnforcement::NativeVersion, Some(token.clone()), None)
                    }
                    SourceSelector::ValidatorMatch { token } => {
                        (ReadEnforcement::ValidatorMatch, None, Some(token.clone()))
                    }
                }
            }
            _ => panic!("unknown synthetic selection"),
        };
        std::thread::sleep(self.open_delay);
        if self.open_error {
            return Box::pin(async {
                Err(ExtensionError::new(
                    ExtensionErrorKind::Provider,
                    Some(StorageError::AccessDenied {
                        provider: ProviderKind::Local,
                        operation: StorageOperation::Get,
                        target: None,
                        detail: "contained read denied".into(),
                    }),
                ))
            });
        }
        let metadata = if self.metadata {
            ReadMetadata::new(
                Evidence::Available(
                    self.metadata_values
                        .then(|| "application/octet-stream".into()),
                ),
                Evidence::Available(if self.metadata_values {
                    BTreeMap::from([("synthetic-label".into(), "sample".into())])
                } else {
                    BTreeMap::new()
                }),
            )
        } else {
            ReadMetadata::new(Evidence::Unavailable, Evidence::Unavailable)
        };
        let response = ReadResponse::new(
            Box::new(Body {
                pending: self.pending_read,
                bytes: self.bytes.clone(),
                offset: 0,
                error_at: self.source_error_at,
                delay: self.poll_delay,
                barrier: self.poll_barrier.clone(),
                counts: self.counts.clone(),
            }),
            SourceReceipt {
                path: "sample.bin".into(),
                native_version,
                validator,
                total_size: self.length,
                requested_window: None,
                returned_window: None,
            },
            enforcement,
            metadata,
        );
        Box::pin(async { Ok(response) })
    }
}
struct Prepared {
    write: Write,
    counts: Arc<Counts>,
    length: usize,
    key: String,
}
impl AdmittedPutProvider for Fake {
    fn put_support(&self) -> PutSupport {
        PutSupport::new(
            WriteMode::SinglePart,
            true,
            true,
            self.match_supported,
            self.metadata_supported,
            self.metadata_supported,
            self.maximum,
        )
    }
    fn admit_put(
        &self,
        key: &str,
        options: PutOptions,
        _: WriteMode,
    ) -> std::result::Result<Box<dyn PreparedPut>, ExtensionError> {
        self.counts.admit.fetch_add(1, Ordering::SeqCst);
        if let Some(cancellation) = &self.cancel_admit {
            cancellation.cancel();
        }
        std::thread::sleep(self.admit_delay);
        *self.counts.options.lock().unwrap() = Some(options.clone());
        if self.admit_error {
            return Err(ExtensionError::new(
                ExtensionErrorKind::InvalidRequest,
                None,
            ));
        }
        Ok(Box::new(Prepared {
            write: self.write,
            counts: self.counts.clone(),
            length: options.content_length.unwrap() as usize,
            key: key.into(),
        }))
    }
}
fn safe_failure(commit: CommitKnowledge, conflict: bool) -> PutFailure {
    let canonical = if conflict {
        StorageError::Conflict {
            provider: ProviderKind::Local,
            operation: StorageOperation::Put,
            target: None,
            kind: ConflictKind::AlreadyExists,
            detail: "synthetic existing object".into(),
        }
    } else {
        StorageError::Io {
            operation: Some(StorageOperation::Put),
            source: io::Error::new(io::ErrorKind::PermissionDenied, "contained write denied"),
        }
    };
    PutFailure::new(
        commit,
        ExtensionError::new(ExtensionErrorKind::Provider, Some(canonical)),
    )
}
impl PreparedPut for Prepared {
    fn execute(
        self: Box<Self>,
        mut body: BoxedByteStream,
        control: OperationControl,
    ) -> FutureResult<'static, PutResult, PutFailure> {
        self.counts.execute.fetch_add(1, Ordering::SeqCst);
        self.counts.started.notify_one();
        Box::pin(async move {
            if matches!(self.write, Write::Pending) {
                std::future::pending::<()>().await;
            }
            if matches!(self.write, Write::Conflict) {
                return Err(safe_failure(CommitKnowledge::NotCommitted, true));
            }
            if matches!(self.write, Write::PartialError) {
                let mut byte = [0; 1];
                let _ = body.read_exact(&mut byte).await;
                return Err(safe_failure(CommitKnowledge::NotCommitted, false));
            }
            if !matches!(self.write, Write::Early) {
                // Exact-N writer deliberately NEVER polls EOF. Relay must prove it.
                for _ in 0..self.length {
                    let mut byte = [0u8; 1];
                    if body.read_exact(&mut byte).await.is_err() {
                        if matches!(self.write, Write::IgnoreBodyError) {
                            break;
                        }
                        return Err(safe_failure(CommitKnowledge::Unknown, false));
                    }
                }
            }
            match self.write {
                Write::Lost => Err(safe_failure(CommitKnowledge::Unknown, false)),
                Write::CommittedError => Err(safe_failure(CommitKnowledge::Committed, false)),
                _ => {
                    if matches!(self.write, Write::LateAck) {
                        std::thread::sleep(Duration::from_millis(80));
                    }
                    if matches!(self.write, Write::CancelThenAck) {
                        control.cancellation().cancel();
                    }
                    Ok(PutResult {
                        path: self.key,
                        etag: None,
                        size: Some(self.length as u64),
                    })
                }
            }
        })
    }
}
fn request(length: u64, buffer: usize) -> RelayRequest {
    RelayRequest::new(
        ReadRequest::Current("sample.bin".into()),
        "output.bin".into(),
        PutPrecondition::MustNotExist,
        MetadataPolicy::None,
        TransferBudget::new(length, buffer, Duration::from_secs(1)).unwrap(),
    )
}

#[tokio::test]
async fn exact_and_empty_sources_prove_eof_for_exact_n_no_extra_poll_writers() {
    for length in 0..10 {
        for buffer in 1..5 {
            let source = Fake::new(&vec![7; length], length as u64);
            let destination = Fake::new(&[], 0);
            let result = relay_copy(
                &source,
                &destination,
                request(length as u64, buffer),
                Cancellation::new(),
            )
            .await
            .unwrap();
            assert_eq!(result.source_bytes_consumed(), length as u64);
            assert_eq!(result.destination_bytes_handed_off(), length as u64);
            assert_eq!(result.strategy(), CopyStrategy::Relay);
            assert!(source.counts.poll.load(Ordering::SeqCst) >= 1);
            assert_eq!(source.counts.open.load(Ordering::SeqCst), 1);
            assert_eq!(destination.counts.execute.load(Ordering::SeqCst), 1);
        }
    }
}

#[tokio::test]
async fn short_excess_and_source_fault_never_release_unvalidated_final_fragment() {
    for (bytes, declared, fault, expected, consumed, delivered) in [
        (
            b"abc".as_slice(),
            4,
            None,
            TransferFailureKind::LengthMismatch,
            3,
            3,
        ),
        (
            b"abcde".as_slice(),
            4,
            None,
            TransferFailureKind::LengthMismatch,
            5,
            2,
        ),
        (
            b"abcd".as_slice(),
            4,
            Some(3),
            TransferFailureKind::SourceBody,
            3,
            3,
        ),
        (
            b"x".as_slice(),
            0,
            None,
            TransferFailureKind::LengthMismatch,
            1,
            0,
        ),
    ] {
        let mut source = Fake::new(bytes, declared);
        source.source_error_at = fault;
        let destination = Fake::new(&[], 0);
        let error = relay_copy(
            &source,
            &destination,
            request(declared, 2),
            Cancellation::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), expected);
        assert_eq!(error.source_bytes_consumed(), consumed);
        assert_eq!(error.destination_bytes_handed_off(), delivered);
        assert_eq!(
            error.commit(),
            if declared == 0 {
                CommitKnowledge::NotStarted
            } else {
                CommitKnowledge::Unknown
            }
        );
        if declared == 0 {
            assert_eq!(destination.counts.execute.load(Ordering::SeqCst), 0);
        }
    }
}

#[tokio::test]
async fn early_acknowledgement_is_committed_failure_not_success() {
    let source = Fake::new(b"abcd", 4);
    let mut destination = Fake::new(&[], 0);
    destination.write = Write::Early;
    let error = relay_copy(&source, &destination, request(4, 2), Cancellation::new())
        .await
        .unwrap_err();
    assert_eq!(error.kind(), TransferFailureKind::LengthMismatch);
    assert_eq!(error.commit(), CommitKnowledge::Committed);
    assert_eq!(error.source_bytes_consumed(), 0);
}

#[tokio::test]
async fn actual_terminal_mapping_preserves_original_commit_canonical_code_and_nested_chain() {
    for (write, commit) in [
        (Write::Lost, CommitKnowledge::Unknown),
        (Write::CommittedError, CommitKnowledge::Committed),
        (Write::Conflict, CommitKnowledge::NotCommitted),
    ] {
        let source = Fake::new(b"abcd", 4);
        let mut destination = Fake::new(&[], 0);
        destination.write = write;
        let error = relay_copy(&source, &destination, request(4, 2), Cancellation::new())
            .await
            .unwrap_err();
        assert_eq!(error.commit(), commit);
        let canonical = error.cause().unwrap().cause().unwrap();
        if matches!(write, Write::Conflict) {
            assert!(matches!(
                canonical,
                StorageError::Conflict {
                    kind: ConflictKind::AlreadyExists,
                    ..
                }
            ));
            assert_eq!(canonical.code(), StorageErrorCode::Conflict);
        } else {
            assert!(
                matches!(canonical, StorageError::Io { source, .. } if source.kind() == io::ErrorKind::PermissionDenied)
            );
            assert_eq!(canonical.code(), StorageErrorCode::Io);
        }
        let mut cause: &(dyn std::error::Error + 'static) = &error;
        let mut chain = 0;
        loop {
            let text = format!("{cause} {cause:?}");
            assert!(!text.contains("/invented-private-root"));
            chain += 1;
            match cause.source() {
                Some(next) => cause = next,
                None => break,
            }
        }
        assert_eq!(
            chain,
            if matches!(write, Write::Conflict) {
                3
            } else {
                4
            }
        );
        assert_eq!(destination.counts.execute.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn unsupported_predicate_precancel_and_expired_clock_do_not_open_or_dispatch() {
    for mode in 0..3 {
        let source = Fake::new(b"abcd", 4);
        let destination = Fake::new(&[], 0);
        let cancellation = Cancellation::new();
        let request = match mode {
            0 => RelayRequest::new(
                ReadRequest::Current("sample.bin".into()),
                "output.bin".into(),
                PutPrecondition::Match {
                    token: "synthetic-token".into(),
                },
                MetadataPolicy::None,
                TransferBudget::new(4, 2, Duration::from_secs(1)).unwrap(),
            ),
            1 => {
                cancellation.cancel();
                request(4, 2)
            }
            _ => RelayRequest::new(
                ReadRequest::Current("sample.bin".into()),
                "output.bin".into(),
                PutPrecondition::None,
                MetadataPolicy::None,
                TransferBudget::new(4, 2, Duration::from_nanos(1)).unwrap(),
            ),
        };
        let error = relay_copy(&source, &destination, request, cancellation)
            .await
            .unwrap_err();
        assert_eq!(error.commit(), CommitKnowledge::NotStarted);
        assert_eq!(source.counts.open.load(Ordering::SeqCst), 0);
        assert_eq!(source.counts.poll.load(Ordering::SeqCst), 0);
        assert_eq!(destination.counts.execute.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn unknown_or_oversize_length_and_unavailable_metadata_refuse_before_body_poll() {
    for mode in 0..3 {
        let mut source = Fake::new(b"abcd", 4);
        let mut destination = Fake::new(&[], 0);
        match mode {
            0 => source.length = None,
            1 => destination.maximum = 3,
            _ => source.metadata = false,
        }
        let request = RelayRequest::new(
            ReadRequest::Current("sample.bin".into()),
            "output.bin".into(),
            PutPrecondition::None,
            MetadataPolicy::ContentTypeAndUserMetadata,
            TransferBudget::new(4, 2, Duration::from_secs(1)).unwrap(),
        );
        let error = relay_copy(&source, &destination, request, Cancellation::new())
            .await
            .unwrap_err();
        assert_eq!(error.kind(), TransferFailureKind::Admission);
        assert_eq!(error.commit(), CommitKnowledge::NotStarted);
        assert_eq!(source.counts.poll.load(Ordering::SeqCst), 0);
        assert_eq!(destination.counts.execute.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn deadline_after_dispatch_is_unknown_and_not_retried() {
    let source = Fake::new(b"abcd", 4);
    let mut destination = Fake::new(&[], 0);
    destination.write = Write::Pending;
    let request = RelayRequest::new(
        ReadRequest::Current("sample.bin".into()),
        "output.bin".into(),
        PutPrecondition::None,
        MetadataPolicy::None,
        TransferBudget::new(4, 2, Duration::from_millis(20)).unwrap(),
    );
    let error = relay_copy(&source, &destination, request, Cancellation::new())
        .await
        .unwrap_err();
    assert_eq!(error.kind(), TransferFailureKind::Deadline);
    assert_eq!(error.commit(), CommitKnowledge::Unknown);
    assert_eq!(destination.counts.execute.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn source_open_and_exact_admission_failure_keep_cause_without_body_poll() {
    for open_failure in [true, false] {
        let mut source = Fake::new(b"abcd", 4);
        source.open_error = open_failure;
        let mut destination = Fake::new(&[], 0);
        destination.admit_error = !open_failure;
        let error = relay_copy(&source, &destination, request(4, 2), Cancellation::new())
            .await
            .unwrap_err();
        assert_eq!(
            error.kind(),
            if open_failure {
                TransferFailureKind::SourceOpen
            } else {
                TransferFailureKind::Admission
            }
        );
        assert_eq!(error.commit(), CommitKnowledge::NotStarted);
        if open_failure {
            assert_eq!(
                error.cause().unwrap().cause().unwrap().code(),
                StorageErrorCode::AccessDenied
            );
        }
        assert_eq!(source.counts.poll.load(Ordering::SeqCst), 0);
        assert_eq!(destination.counts.execute.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn metadata_policy_is_explicit_and_mapped_from_selected_response() {
    for policy in [
        MetadataPolicy::None,
        MetadataPolicy::ContentTypeOnly,
        MetadataPolicy::ContentTypeAndUserMetadata,
    ] {
        let mut source = Fake::new(b"abcd", 4);
        source.metadata_values = true;
        let destination = Fake::new(&[], 0);
        let mut req = request(4, 2);
        req = RelayRequest::new(
            ReadRequest::Current("sample.bin".into()),
            "output.bin".into(),
            req.precondition().clone(),
            policy,
            *req.budget(),
        );
        relay_copy(&source, &destination, req, Cancellation::new())
            .await
            .unwrap();
        let options = destination.counts.options.lock().unwrap();
        let options = options.as_ref().unwrap();
        assert_eq!(options.content_length, Some(4));
        assert_eq!(
            options.content_type.is_some(),
            policy != MetadataPolicy::None
        );
        assert_eq!(
            !options.metadata.is_empty(),
            policy == MetadataPolicy::ContentTypeAndUserMetadata
        );
        assert!(matches!(
            options.precondition,
            PutPrecondition::MustNotExist
        ));
    }
    let source = Fake::new(b"abcd", 4);
    let mut destination = Fake::new(&[], 0);
    destination.metadata_supported = false;
    let req = RelayRequest::new(
        ReadRequest::Current("sample.bin".into()),
        "output.bin".into(),
        PutPrecondition::None,
        MetadataPolicy::ContentTypeOnly,
        *request(4, 2).budget(),
    );
    let error = relay_copy(&source, &destination, req, Cancellation::new())
        .await
        .unwrap_err();
    assert_eq!(
        error.cause().unwrap().kind(),
        ExtensionErrorKind::UnsupportedMetadata
    );
    assert_eq!(source.counts.open.load(Ordering::SeqCst), 0);
    // Explicit None works even when preserving known-present metadata is unsupported.
    relay_copy(&source, &destination, request(4, 2), Cancellation::new())
        .await
        .unwrap();
}

#[tokio::test]
async fn missing_hooks_never_downgrade_to_legacy_get_put() {
    for source_missing in [true, false] {
        let mut source = Fake::new(b"abcd", 4);
        source.read_hook = !source_missing;
        let mut destination = Fake::new(&[], 0);
        destination.write_hook = source_missing;
        let error = relay_copy(&source, &destination, request(4, 2), Cancellation::new())
            .await
            .unwrap_err();
        assert_eq!(
            error.kind(),
            if source_missing {
                TransferFailureKind::SourceExtensionUnavailable
            } else {
                TransferFailureKind::DestinationExtensionUnavailable
            }
        );
        assert_eq!(source.counts.open.load(Ordering::SeqCst), 0);
        assert_eq!(destination.counts.execute.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn cancellation_after_dispatch_returns_unknown_and_drops_without_retry_or_delete() {
    let source = Fake::new(b"abcd", 4);
    let mut destination = Fake::new(&[], 0);
    destination.write = Write::Pending;
    let counts = destination.counts.clone();
    let source_counts = source.counts.clone();
    let cancellation = Cancellation::new();
    let other = cancellation.clone();
    let task = tokio::spawn(async move {
        relay_copy(&source, &destination, request(4, 2), cancellation).await
    });
    tokio::time::timeout(Duration::from_secs(1), counts.started.notified())
        .await
        .unwrap();
    other.cancel();
    let error = task.await.unwrap().unwrap_err();
    assert_eq!(error.kind(), TransferFailureKind::Cancelled);
    assert_eq!(error.commit(), CommitKnowledge::Unknown);
    assert_eq!(counts.execute.load(Ordering::SeqCst), 1);
    assert_eq!(source_counts.dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn dropping_relay_returns_no_receipt_and_releases_body_without_retry() {
    let source = Fake::new(b"abcd", 4);
    let mut destination = Fake::new(&[], 0);
    destination.write = Write::Pending;
    let counts = destination.counts.clone();
    let source_counts = source.counts.clone();
    let task = tokio::spawn(async move {
        relay_copy(&source, &destination, request(4, 2), Cancellation::new()).await
    });
    tokio::time::timeout(Duration::from_secs(1), counts.started.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(counts.execute.load(Ordering::SeqCst), 1);
    assert_eq!(source_counts.dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancellation_inside_writer_before_acknowledgement_is_committed_failure() {
    let source = Fake::new(b"abcd", 4);
    let mut destination = Fake::new(&[], 0);
    destination.write = Write::CancelThenAck;
    let cancellation = Cancellation::new();
    let error = relay_copy(&source, &destination, request(4, 2), cancellation.clone())
        .await
        .unwrap_err();
    assert!(cancellation.is_cancelled());
    assert_eq!(error.kind(), TransferFailureKind::Cancelled);
    assert_eq!(error.commit(), CommitKnowledge::Committed);
    assert_eq!(error.destination_bytes_handed_off(), 4);
}

#[tokio::test]
async fn cancellation_after_observed_timely_completion_does_not_overturn_receipt() {
    let source = Fake::new(b"a", 1);
    let destination = Fake::new(&[], 0);
    let cancellation = Cancellation::new();
    let receipt = relay_copy(&source, &destination, request(1, 1), cancellation.clone())
        .await
        .unwrap();
    cancellation.cancel();
    assert_eq!(receipt.destination_bytes_handed_off(), 1);
}

#[tokio::test]
async fn blocking_source_poll_charges_consumed_but_withholds_bytes_after_deadline() {
    let mut source = Fake::new(b"a", 1);
    source.poll_delay = Duration::from_millis(80);
    let destination = Fake::new(&[], 0);
    let req = RelayRequest::new(
        ReadRequest::Current("sample.bin".into()),
        "output.bin".into(),
        PutPrecondition::None,
        MetadataPolicy::None,
        TransferBudget::new(1, 1, Duration::from_millis(20)).unwrap(),
    );
    let error = relay_copy(&source, &destination, req, Cancellation::new())
        .await
        .unwrap_err();
    assert_eq!(error.kind(), TransferFailureKind::Deadline);
    assert_eq!(error.commit(), CommitKnowledge::Unknown);
    assert_eq!(error.source_bytes_consumed(), 1);
    assert_eq!(error.destination_bytes_handed_off(), 0);
    assert_eq!(
        error.cause().unwrap().cause().unwrap().code(),
        StorageErrorCode::Io
    );
}

#[tokio::test]
async fn cross_thread_cancellation_during_source_poll_withholds_returned_bytes() {
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let mut source = Fake::new(b"a", 1);
    source.poll_barrier = Some(barrier.clone());
    source.poll_delay = Duration::from_millis(40);
    let destination = Fake::new(&[], 0);
    let cancellation = Cancellation::new();
    let other = cancellation.clone();
    let cancel = std::thread::spawn(move || {
        barrier.wait();
        other.cancel();
    });
    let error = relay_copy(&source, &destination, request(1, 1), cancellation)
        .await
        .unwrap_err();
    cancel.join().unwrap();
    assert_eq!(error.kind(), TransferFailureKind::Cancelled);
    assert_eq!(error.commit(), CommitKnowledge::Unknown);
    assert_eq!(error.source_bytes_consumed(), 1);
    assert_eq!(error.destination_bytes_handed_off(), 0);
}

#[tokio::test]
async fn late_destination_ok_preserves_committed_as_deadline_failure() {
    let source = Fake::new(b"a", 1);
    let mut destination = Fake::new(&[], 0);
    destination.write = Write::LateAck;
    let req = RelayRequest::new(
        ReadRequest::Current("sample.bin".into()),
        "output.bin".into(),
        PutPrecondition::None,
        MetadataPolicy::None,
        TransferBudget::new(1, 1, Duration::from_millis(20)).unwrap(),
    );
    let error = relay_copy(&source, &destination, req, Cancellation::new())
        .await
        .unwrap_err();
    assert_eq!(error.kind(), TransferFailureKind::Deadline);
    assert_eq!(error.commit(), CommitKnowledge::Committed);
    assert_eq!(error.source_bytes_consumed(), 1);
    assert_eq!(error.destination_bytes_handed_off(), 1);
}

#[tokio::test]
async fn late_open_or_admission_never_polls_body_or_executes_destination() {
    for slow_open in [true, false] {
        let mut source = Fake::new(b"a", 1);
        let mut destination = Fake::new(&[], 0);
        if slow_open {
            source.open_delay = Duration::from_millis(80);
        } else {
            destination.admit_delay = Duration::from_millis(80);
        }
        let req = RelayRequest::new(
            ReadRequest::Current("sample.bin".into()),
            "output.bin".into(),
            PutPrecondition::None,
            MetadataPolicy::None,
            TransferBudget::new(1, 1, Duration::from_millis(20)).unwrap(),
        );
        let error = relay_copy(&source, &destination, req, Cancellation::new())
            .await
            .unwrap_err();
        assert_eq!(error.kind(), TransferFailureKind::Deadline);
        assert_eq!(error.commit(), CommitKnowledge::NotStarted);
        assert_eq!(source.counts.poll.load(Ordering::SeqCst), 0);
        assert_eq!(destination.counts.execute.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn guarded_selection_and_destination_predicate_remain_independently_bound() {
    for selector in [
        SourceSelector::NativeVersion {
            token: "synthetic-version".into(),
        },
        SourceSelector::ValidatorMatch {
            token: "\"synthetic-validator\"".into(),
        },
    ] {
        let mut source = Fake::new(b"abcd", 4);
        source.guarded = true;
        let mut destination = Fake::new(&[], 0);
        destination.match_supported = true;
        let expected = match &selector {
            SourceSelector::NativeVersion { .. } => ReadEnforcement::NativeVersion,
            SourceSelector::ValidatorMatch { .. } => ReadEnforcement::ValidatorMatch,
        };
        let selection = source.bind_guarded_read("sample.bin", selector).unwrap();
        let req = RelayRequest::new(
            ReadRequest::Guarded(selection),
            "output.bin".into(),
            PutPrecondition::Match {
                token: "synthetic-destination-validator".into(),
            },
            MetadataPolicy::None,
            *request(4, 2).budget(),
        );
        let receipt = relay_copy(&source, &destination, req, Cancellation::new())
            .await
            .unwrap();
        assert_eq!(receipt.enforcement(), expected);
        let options = destination.counts.options.lock().unwrap();
        assert!(
            matches!(&options.as_ref().unwrap().precondition, PutPrecondition::Match { token }
            if token == "synthetic-destination-validator")
        );
        assert_eq!(source.counts.open.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn unsupported_guard_refuses_before_open_and_wrong_binding_is_not_downgraded() {
    for unsupported in [true, false] {
        let mut source = Fake::new(b"abcd", 4);
        source.guarded = !unsupported;
        let destination = Fake::new(&[], 0);
        let selection = source
            .bind_guarded_read(
                "other.bin",
                SourceSelector::NativeVersion {
                    token: "synthetic-version".into(),
                },
            )
            .unwrap();
        let req = RelayRequest::new(
            ReadRequest::Guarded(selection),
            "output.bin".into(),
            PutPrecondition::None,
            MetadataPolicy::None,
            *request(4, 2).budget(),
        );
        let error = relay_copy(&source, &destination, req, Cancellation::new())
            .await
            .unwrap_err();
        assert_eq!(error.commit(), CommitKnowledge::NotStarted);
        assert_eq!(
            source.counts.open.load(Ordering::SeqCst),
            usize::from(!unsupported)
        );
        assert_eq!(source.counts.poll.load(Ordering::SeqCst), 0);
        assert_eq!(destination.counts.execute.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn partial_destination_failure_does_not_drain_source_or_conflate_counters() {
    let source = Fake::new(b"abcdef", 6);
    let mut destination = Fake::new(&[], 0);
    destination.write = Write::PartialError;
    let error = relay_copy(&source, &destination, request(6, 2), Cancellation::new())
        .await
        .unwrap_err();
    assert_eq!(error.kind(), TransferFailureKind::Destination);
    assert_eq!(error.commit(), CommitKnowledge::NotCommitted);
    assert_eq!(error.source_bytes_consumed(), 2);
    assert_eq!(error.destination_bytes_handed_off(), 1);
    assert_eq!(source.counts.poll.load(Ordering::SeqCst), 1);
    assert_eq!(
        error.cause().unwrap().cause().unwrap().code(),
        StorageErrorCode::Io
    );
}

#[tokio::test]
async fn caller_payload_limit_refuses_before_body_poll_or_destination_admission() {
    let source = Fake::new(b"abcd", 4);
    let destination = Fake::new(&[], 0);
    let error = relay_copy(&source, &destination, request(3, 2), Cancellation::new())
        .await
        .unwrap_err();
    assert_eq!(
        error.cause().unwrap().kind(),
        ExtensionErrorKind::LengthLimit
    );
    assert_eq!(error.commit(), CommitKnowledge::NotStarted);
    assert_eq!(source.counts.poll.load(Ordering::SeqCst), 0);
    assert_eq!(destination.counts.admit.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn empty_source_io_error_is_not_dispatched_and_not_success() {
    let mut source = Fake::new(b"", 0);
    source.source_error_at = Some(0);
    let destination = Fake::new(&[], 0);
    let error = relay_copy(&source, &destination, request(0, 1), Cancellation::new())
        .await
        .unwrap_err();
    assert_eq!(error.kind(), TransferFailureKind::SourceBody);
    assert_eq!(error.commit(), CommitKnowledge::NotStarted);
    assert_eq!(error.source_bytes_consumed(), 0);
    assert_eq!(destination.counts.execute.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn writer_ignoring_source_failure_returns_committed_failure_not_receipt() {
    let source = Fake::new(b"abcde", 4);
    let mut destination = Fake::new(&[], 0);
    destination.write = Write::IgnoreBodyError;
    let error = relay_copy(&source, &destination, request(4, 2), Cancellation::new())
        .await
        .unwrap_err();
    assert_eq!(error.kind(), TransferFailureKind::LengthMismatch);
    assert_eq!(error.commit(), CommitKnowledge::Committed);
    assert_eq!(error.source_bytes_consumed(), 5);
    assert_eq!(error.destination_bytes_handed_off(), 2);
}

#[tokio::test]
async fn source_read_windows_are_bounded_by_caller_buffer_not_payload_size() {
    for buffer in [1, 2, 7, 32] {
        let source = Fake::new(&[3; 100], 100);
        let destination = Fake::new(&[], 0);
        relay_copy(
            &source,
            &destination,
            request(100, buffer),
            Cancellation::new(),
        )
        .await
        .unwrap();
        assert_eq!(
            source.counts.max_read_capacity.load(Ordering::SeqCst),
            buffer
        );
    }
    // This measures requested read windows, not allocator/RSS or SDK buffering.
}

#[tokio::test]
async fn cancellation_during_open_or_synchronous_admission_never_dispatches_write() {
    for at_open in [true, false] {
        let cancellation = Cancellation::new();
        let mut source = Fake::new(b"a", 1);
        source.cancel_open = at_open;
        let mut destination = Fake::new(&[], 0);
        if !at_open {
            destination.cancel_admit = Some(cancellation.clone());
        }
        let error = relay_copy(&source, &destination, request(1, 1), cancellation)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), TransferFailureKind::Cancelled);
        assert_eq!(error.commit(), CommitKnowledge::NotStarted);
        assert_eq!(source.counts.poll.load(Ordering::SeqCst), 0);
        assert_eq!(destination.counts.execute.load(Ordering::SeqCst), 0);
        assert_eq!(
            destination.counts.admit.load(Ordering::SeqCst),
            usize::from(!at_open)
        );
    }
}

#[tokio::test]
async fn cancellation_during_nonterminal_source_read_preserves_unknown_and_releases_body() {
    let mut source = Fake::new(b"a", 1);
    source.pending_read = true;
    let source_counts = source.counts.clone();
    let destination = Fake::new(&[], 0);
    let destination_counts = destination.counts.clone();
    let cancellation = Cancellation::new();
    let other = cancellation.clone();
    let task = tokio::spawn(async move {
        relay_copy(&source, &destination, request(1, 1), cancellation).await
    });
    tokio::time::timeout(
        Duration::from_secs(1),
        source_counts.read_started.notified(),
    )
    .await
    .unwrap();
    other.cancel();
    let error = task.await.unwrap().unwrap_err();
    assert_eq!(error.kind(), TransferFailureKind::Cancelled);
    assert_eq!(error.commit(), CommitKnowledge::Unknown);
    assert_eq!(
        (
            error.source_bytes_consumed(),
            error.destination_bytes_handed_off()
        ),
        (0, 0)
    );
    assert_eq!(source_counts.dropped.load(Ordering::SeqCst), 1);
    assert_eq!(destination_counts.execute.load(Ordering::SeqCst), 1);
}
