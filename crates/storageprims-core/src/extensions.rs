//! Rust-only receipt-bearing reads and provider-bound admitted writes.
//!
//! Discovery is declarative, not authorization. Admission performs no I/O and
//! retains exact options; execution must enforce the accepted predicate. Neither
//! cancellation nor an error establishes destination absence or rollback.

use std::collections::BTreeMap;
use std::fmt;

use crate::{
    BoxedByteStream, FutureResult, GuardedReadSelection, OperationControl, PutOptions, PutResult,
    SourceReceipt, StorageError,
};

#[non_exhaustive]
pub enum ReadRequest {
    Current(String),
    Guarded(GuardedReadSelection),
}

impl fmt::Debug for ReadRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Current(_) => "ReadRequest::Current([redacted])",
            Self::Guarded(_) => "ReadRequest::Guarded([redacted])",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReadEnforcement {
    Unguarded,
    NativeVersion,
    ValidatorMatch,
}

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ReadSupport {
    current: bool,
    native_version: bool,
    validator_match: bool,
}

impl ReadSupport {
    pub fn new(current: bool, native_version: bool, validator_match: bool) -> Self {
        Self {
            current,
            native_version,
            validator_match,
        }
    }
    pub fn current(&self) -> bool {
        self.current
    }
    pub fn native_version(&self) -> bool {
        self.native_version
    }
    pub fn validator_match(&self) -> bool {
        self.validator_match
    }
}

/// Observed absence is distinct from a read path that cannot report a field.
#[non_exhaustive]
pub enum Evidence<T> {
    Unavailable,
    Available(T),
}

impl<T> fmt::Debug for Evidence<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "Unavailable",
            Self::Available(_) => "Available([redacted])",
        })
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub struct ReadMetadata {
    content_type: Evidence<Option<String>>,
    user_metadata: Evidence<BTreeMap<String, String>>,
}

impl ReadMetadata {
    pub fn new(
        content_type: Evidence<Option<String>>,
        user_metadata: Evidence<BTreeMap<String, String>>,
    ) -> Self {
        Self {
            content_type,
            user_metadata,
        }
    }
    pub fn content_type(&self) -> &Evidence<Option<String>> {
        &self.content_type
    }
    pub fn user_metadata(&self) -> &Evidence<BTreeMap<String, String>> {
        &self.user_metadata
    }
}

/// Evidence and metadata must come from the selected response, never an extra HEAD.
#[non_exhaustive]
pub struct ReadResponse {
    reader: BoxedByteStream,
    receipt: SourceReceipt,
    enforcement: ReadEnforcement,
    metadata: ReadMetadata,
}

impl ReadResponse {
    pub fn new(
        reader: BoxedByteStream,
        receipt: SourceReceipt,
        enforcement: ReadEnforcement,
        metadata: ReadMetadata,
    ) -> Self {
        Self {
            reader,
            receipt,
            enforcement,
            metadata,
        }
    }
    pub fn receipt(&self) -> &SourceReceipt {
        &self.receipt
    }
    pub fn enforcement(&self) -> ReadEnforcement {
        self.enforcement
    }
    pub fn metadata(&self) -> &ReadMetadata {
        &self.metadata
    }
    pub fn into_parts(
        self,
    ) -> (
        BoxedByteStream,
        SourceReceipt,
        ReadEnforcement,
        ReadMetadata,
    ) {
        (self.reader, self.receipt, self.enforcement, self.metadata)
    }
}

impl fmt::Debug for ReadResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadResponse")
            .field("enforcement", &self.enforcement)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExtensionErrorKind {
    UnsupportedReadSelection,
    UnsupportedWriteMode,
    UnsupportedPredicate,
    UnsupportedMetadata,
    LengthRequired,
    LengthLimit,
    InvalidRequest,
    Cancelled,
    Deadline,
    Provider,
}

/// Providers must redact paths/credentials in the canonical cause before wrapping
/// it: callers can inspect the typed cause and traverse `Error::source` directly.
/// This wrapper's formatting never emits cause text or request values.
#[non_exhaustive]
pub struct ExtensionError {
    kind: ExtensionErrorKind,
    cause: Option<StorageError>,
}

impl ExtensionError {
    pub fn new(kind: ExtensionErrorKind, cause: Option<StorageError>) -> Self {
        Self { kind, cause }
    }
    pub fn kind(&self) -> ExtensionErrorKind {
        self.kind
    }
    pub fn cause(&self) -> Option<&StorageError> {
        self.cause.as_ref()
    }
}

impl fmt::Debug for ExtensionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtensionError")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}
impl fmt::Display for ExtensionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "storage extension: {:?}", self.kind)
    }
}
impl std::error::Error for ExtensionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause
            .as_ref()
            .map(|cause| cause as &(dyn std::error::Error + 'static))
    }
}

pub trait ReadResponseProvider: Send + Sync {
    fn read_support(&self) -> ReadSupport;
    fn read_response(
        &self,
        request: ReadRequest,
        control: OperationControl,
    ) -> FutureResult<'_, ReadResponse, ExtensionError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WriteMode {
    SinglePart,
}

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct PutSupport {
    mode: WriteMode,
    unconditional: bool,
    create_only: bool,
    validator_match: bool,
    content_type: bool,
    user_metadata: bool,
    max_content_length: u64,
}

impl PutSupport {
    pub fn new(
        mode: WriteMode,
        unconditional: bool,
        create_only: bool,
        validator_match: bool,
        content_type: bool,
        user_metadata: bool,
        max_content_length: u64,
    ) -> Self {
        Self {
            mode,
            unconditional,
            create_only,
            validator_match,
            content_type,
            user_metadata,
            max_content_length,
        }
    }
    pub fn mode(&self) -> WriteMode {
        self.mode
    }
    pub fn unconditional(&self) -> bool {
        self.unconditional
    }
    pub fn create_only(&self) -> bool {
        self.create_only
    }
    pub fn validator_match(&self) -> bool {
        self.validator_match
    }
    pub fn content_type(&self) -> bool {
        self.content_type
    }
    pub fn user_metadata(&self) -> bool {
        self.user_metadata
    }
    pub fn max_content_length(&self) -> u64 {
        self.max_content_length
    }
}

/// Knowledge about this attempt, not about whether the target exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommitKnowledge {
    NotStarted,
    NotCommitted,
    Committed,
    Unknown,
}

#[derive(Debug)]
#[non_exhaustive]
pub struct PutFailure {
    commit: CommitKnowledge,
    error: ExtensionError,
}

impl PutFailure {
    pub fn new(commit: CommitKnowledge, error: ExtensionError) -> Self {
        Self { commit, error }
    }
    pub fn commit(&self) -> CommitKnowledge {
        self.commit
    }
    pub fn error(&self) -> &ExtensionError {
        &self.error
    }
    /// Move the original commit evidence and canonical cause into composition.
    pub fn into_parts(self) -> (CommitKnowledge, ExtensionError) {
        (self.commit, self.error)
    }
}
impl fmt::Display for PutFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "admitted write ({:?}): {}", self.commit, self.error)
    }
}
impl std::error::Error for PutFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// Single-use binding owning its client/root handle, destination and exact options.
pub trait PreparedPut: Send {
    fn execute(
        self: Box<Self>,
        body: BoxedByteStream,
        control: OperationControl,
    ) -> FutureResult<'static, PutResult, PutFailure>;
}

pub trait AdmittedPutProvider: Send + Sync {
    fn put_support(&self) -> PutSupport;
    /// Semantic validation only: no filesystem/network/credential I/O or body poll.
    fn admit_put(
        &self,
        key: &str,
        options: PutOptions,
        mode: WriteMode,
    ) -> Result<Box<dyn PreparedPut>, ExtensionError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProviderKind, StorageOperation};

    #[test]
    fn observed_absence_is_not_unavailable_and_response_preserves_parts() {
        let receipt = SourceReceipt {
            path: "sample.bin".into(),
            native_version: None,
            validator: None,
            total_size: Some(0),
            requested_window: None,
            returned_window: None,
        };
        let response = ReadResponse::new(
            Box::new(tokio::io::empty()),
            receipt.clone(),
            ReadEnforcement::Unguarded,
            ReadMetadata::new(
                Evidence::Available(None),
                Evidence::Available(BTreeMap::new()),
            ),
        );
        assert!(matches!(
            response.metadata().content_type(),
            Evidence::Available(None)
        ));
        assert!(
            matches!(response.metadata().user_metadata(), Evidence::Available(m) if m.is_empty())
        );
        assert_eq!(response.receipt(), &receipt);
        let (_, actual, enforcement, _) = response.into_parts();
        assert_eq!(actual, receipt);
        assert_eq!(enforcement, ReadEnforcement::Unguarded);
        let unavailable = ReadMetadata::new(Evidence::Unavailable, Evidence::Unavailable);
        assert!(matches!(unavailable.content_type(), Evidence::Unavailable));
    }

    #[test]
    fn typed_cause_and_commit_are_preserved_without_wrapper_disclosure() {
        let sentinel = "invented-sensitive-value";
        let failure = PutFailure::new(
            CommitKnowledge::Committed,
            ExtensionError::new(
                ExtensionErrorKind::Provider,
                Some(StorageError::AccessDenied {
                    provider: ProviderKind::Local,
                    operation: StorageOperation::Put,
                    target: None,
                    detail: sentinel.into(),
                }),
            ),
        );
        assert_eq!(failure.commit(), CommitKnowledge::Committed);
        assert!(matches!(
            failure.error().cause(),
            Some(StorageError::AccessDenied { .. })
        ));
        assert!(std::error::Error::source(&failure).is_some());
        assert!(std::error::Error::source(failure.error()).is_some());
        assert!(!format!("{failure:?} {failure}").contains(sentinel));
        // Typed cause access intentionally retains the cause; providers must
        // sanitize their causes before exposing a public source chain.
    }

    #[test]
    fn debug_omits_request_metadata_and_receipt_values() {
        let sentinel = "invented-sensitive-value";
        let request = ReadRequest::Current(sentinel.into());
        let response = ReadResponse::new(
            Box::new(tokio::io::empty()),
            SourceReceipt {
                path: sentinel.into(),
                native_version: Some(sentinel.into()),
                validator: Some(sentinel.into()),
                total_size: Some(0),
                requested_window: None,
                returned_window: None,
            },
            ReadEnforcement::Unguarded,
            ReadMetadata::new(
                Evidence::Available(Some(sentinel.into())),
                Evidence::Available(BTreeMap::from([(sentinel.into(), sentinel.into())])),
            ),
        );
        assert!(!format!("{request:?} {response:?} {:?}", response.metadata()).contains(sentinel));
    }

    #[test]
    fn conforming_sanitized_cause_chain_retains_class_and_safe_formatting() {
        let private_root = "/invented-private-volume/fixture";
        // The adapter retains the canonical class but excludes the OS path and
        // credential-bearing detail before publishing the error to core.
        let canonical = StorageError::Io {
            operation: Some(StorageOperation::Put),
            source: std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "contained object write denied",
            ),
        };
        assert_eq!(canonical.code(), crate::StorageErrorCode::Io);
        let failure = PutFailure::new(
            CommitKnowledge::NotCommitted,
            ExtensionError::new(ExtensionErrorKind::Provider, Some(canonical)),
        );
        let mut error: &(dyn std::error::Error + 'static) = &failure;
        let mut visited = 0;
        loop {
            let text = format!("{error} {error:?}");
            assert!(!text.contains(private_root));
            assert!(!text.contains("invented-credential-value"));
            visited += 1;
            match error.source() {
                Some(source) => error = source,
                None => break,
            }
        }
        assert_eq!(visited, 4); // PutFailure -> ExtensionError -> StorageError -> io::Error
        assert!(matches!(
            failure.error().cause(),
            Some(StorageError::Io { .. })
        ));
    }
}
