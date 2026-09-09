//! storageprims-core - Core traits, config types, error types, and URI parsing.

pub mod config;
pub mod error;
pub mod provider;
pub mod types;
pub mod uri;

pub use config::{sanitize_endpoint, CredentialSource, ProviderConfig, TargetConfig};
pub use error::{
    ConflictKind, ProviderKind, Result, StorageError, StorageErrorCode, StorageOperation,
};
pub use provider::{BoxFuture, BoxedByteStream, Capability, StorageProvider};
pub use types::{
    CopyRequest, CopyResult, CopyStrategy, CredentialSourceKind, GetRangeRequest, ListOptions,
    ListResult, ObjectMetadata, ObjectSummary, ProbeResult, ProbeScope, PutOptions,
    PutPrecondition, PutResult,
};
pub use uri::StorageUri;
