//! storageprims-core - Core traits, config types, error types, and URI parsing.

pub mod config;
pub mod control;
pub mod error;
pub mod extensions;
pub mod provider;
pub mod types;
pub mod uri;

pub use config::{sanitize_endpoint, CredentialSource, ProviderConfig, TargetConfig};
pub use control::{Cancellation, FutureResult, OperationControl};
pub use error::{
    ConflictKind, InspectionKind, InspectionLimitDimension, ProviderKind, Result, StorageError,
    StorageErrorCode, StorageOperation,
};
pub use extensions::{
    AdmittedPutProvider, CommitKnowledge, Evidence, ExtensionError, ExtensionErrorKind,
    PreparedPut, PutFailure, PutSupport, ReadEnforcement, ReadMetadata, ReadRequest, ReadResponse,
    ReadResponseProvider, ReadSupport, WriteMode,
};
pub use provider::{
    require_delimiter_listing, require_guarded_reads, BoxFuture, BoxedByteStream, Capability,
    DelimiterListingProvider, GuardedReadProvider, StorageProvider,
};
pub use types::{
    ByteWindow, CopyRequest, CopyResult, CopyStrategy, CredentialSourceKind, DelimiterListRequest,
    DelimiterListResult, GetRangeRequest, GuardedRangeRequest, GuardedReadResponse,
    GuardedReadSelection, ListOptions, ListResult, ObjectMetadata, ObjectSummary, ProbeResult,
    ProbeScope, PutOptions, PutPrecondition, PutResult, SourceObservation, SourceReceipt,
    SourceSelector,
};
pub use uri::StorageUri;
