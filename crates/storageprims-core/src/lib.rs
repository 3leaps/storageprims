//! storageprims-core - Core traits, config types, error types, and URI parsing.

pub mod config;
pub mod error;
pub mod provider;
pub mod types;
pub mod uri;

pub use config::{CredentialSource, ProviderConfig, TargetConfig};
pub use error::{ProviderKind, Result, StorageError, StorageErrorCode, StorageOperation};
pub use provider::{BoxFuture, BoxedByteStream, Capability, StorageProvider};
pub use types::{
    CopyRequest, CopyResult, CopyStrategy, GetRangeRequest, ListOptions, ListResult,
    ObjectMetadata, ObjectSummary, PutOptions, PutResult,
};
pub use uri::StorageUri;
