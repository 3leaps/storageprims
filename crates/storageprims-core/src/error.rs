//! Canonical error and provider types for storageprims.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::provider::Capability;

/// Canonical cloud storage provider identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    S3,
    Gcs,
    AzureBlob,
    Local,
}

impl std::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::S3 => write!(f, "s3"),
            Self::Gcs => write!(f, "gcs"),
            Self::AzureBlob => write!(f, "azure_blob"),
            Self::Local => write!(f, "local"),
        }
    }
}

/// Canonical storage operation identifiers.
/// Stable reason for a conditional-write conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageOperation {
    List,
    Head,
    HeadLines,
    TailLines,
    MidLines,
    CountLines,
    Probe,
    Get,
    GetRange,
    PreviewBytes,
    ObserveSource,
    GuardedHead,
    GuardedGet,
    GuardedGetRange,
    Put,
    Delete,
    Copy,
    ConfigureProvider,
    ParseUri,
}

impl std::fmt::Display for StorageOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::List => write!(f, "list"),
            Self::Head => write!(f, "head"),
            Self::HeadLines => write!(f, "head_lines"),
            Self::TailLines => write!(f, "tail_lines"),
            Self::MidLines => write!(f, "mid_lines"),
            Self::CountLines => write!(f, "count_lines"),
            Self::Probe => write!(f, "probe"),
            Self::Get => write!(f, "get"),
            Self::GetRange => write!(f, "get_range"),
            Self::PreviewBytes => write!(f, "preview_bytes"),
            Self::ObserveSource => write!(f, "observe_source"),
            Self::GuardedHead => write!(f, "guarded_head"),
            Self::GuardedGet => write!(f, "guarded_get"),
            Self::GuardedGetRange => write!(f, "guarded_get_range"),
            Self::Put => write!(f, "put"),
            Self::Delete => write!(f, "delete"),
            Self::Copy => write!(f, "copy"),
            Self::ConfigureProvider => write!(f, "configure_provider"),
            Self::ParseUri => write!(f, "parse_uri"),
        }
    }
}

/// Stable error codes for FFI and bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageErrorCode {
    InvalidUri,
    InvalidArgument,
    NotFound,
    ContainerNotFound,
    AccessDenied,
    InvalidCredentials,
    Throttled,
    ProviderUnavailable,
    UnsupportedCapability,
    Conflict,
    Io,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    AlreadyExists,
    TokenMismatch,
    Other,
}

/// Machine-readable category for bounded inspection failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InspectionKind {
    LimitExceeded,
    EncodingRejected,
}

/// Resource dimension responsible for a bounded inspection failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InspectionLimitDimension {
    PayloadBytes,
    Requests,
    OutputBytes,
    PendingLineBytes,
    LineCount,
    ChunkSize,
    ProbeSize,
    Deadline,
}

/// Canonical error type for all storageprims operations.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("invalid URI: {uri} - {reason}")]
    InvalidUri { uri: String, reason: String },

    #[error("invalid argument for {operation:?}: {argument} - {reason}")]
    InvalidArgument {
        operation: Option<StorageOperation>,
        argument: String,
        reason: String,
    },

    #[error("{provider} {operation}: object not found: {path}")]
    NotFound {
        provider: ProviderKind,
        operation: StorageOperation,
        path: String,
    },

    #[error("{provider} {operation}: container not found: {container}")]
    ContainerNotFound {
        provider: ProviderKind,
        operation: StorageOperation,
        container: String,
    },

    #[error("{provider} {operation}: access denied: {detail}")]
    AccessDenied {
        provider: ProviderKind,
        operation: StorageOperation,
        target: Option<String>,
        detail: String,
    },

    #[error("{provider}: invalid credentials - {detail}")]
    InvalidCredentials {
        provider: ProviderKind,
        detail: String,
    },

    #[error("{provider} {operation}: throttled (retry after {retry_after:?})")]
    Throttled {
        provider: ProviderKind,
        operation: StorageOperation,
        retry_after: Option<Duration>,
    },

    #[error("{provider} {operation}: provider unavailable - {detail}")]
    ProviderUnavailable {
        provider: ProviderKind,
        operation: StorageOperation,
        detail: String,
    },

    #[error("{provider} {operation}: unsupported capability: {capability:?}")]
    UnsupportedCapability {
        provider: ProviderKind,
        operation: StorageOperation,
        capability: Capability,
    },

    #[error("{provider} {operation}: conflict: {detail}")]
    Conflict {
        provider: ProviderKind,
        operation: StorageOperation,
        target: Option<String>,
        /// Machine-readable reason; callers must not parse `detail`.
        kind: ConflictKind,
        detail: String,
    },

    #[error("{operation}: bounded inspection {kind:?} ({limit_dimension:?})")]
    Inspection {
        provider: Option<ProviderKind>,
        operation: StorageOperation,
        kind: InspectionKind,
        limit_dimension: Option<InspectionLimitDimension>,
        configured_limit: u64,
        consumed: u64,
    },

    #[error("I/O error during {operation:?}: {source}")]
    Io {
        operation: Option<StorageOperation>,
        #[source]
        source: std::io::Error,
    },

    #[error("{detail}")]
    Other {
        provider: Option<ProviderKind>,
        operation: Option<StorageOperation>,
        detail: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
}

impl StorageError {
    pub fn code(&self) -> StorageErrorCode {
        StorageErrorCode::from(self)
    }
}

impl From<&StorageError> for StorageErrorCode {
    fn from(value: &StorageError) -> Self {
        match value {
            StorageError::InvalidUri { .. } => Self::InvalidUri,
            StorageError::InvalidArgument { .. } => Self::InvalidArgument,
            StorageError::NotFound { .. } => Self::NotFound,
            StorageError::ContainerNotFound { .. } => Self::ContainerNotFound,
            StorageError::AccessDenied { .. } => Self::AccessDenied,
            StorageError::InvalidCredentials { .. } => Self::InvalidCredentials,
            StorageError::Throttled { .. } => Self::Throttled,
            StorageError::ProviderUnavailable { .. } => Self::ProviderUnavailable,
            StorageError::UnsupportedCapability { .. } => Self::UnsupportedCapability,
            StorageError::Conflict { .. } => Self::Conflict,
            // The Unix ABI keeps its established numeric Other=99 code while
            // projecting this variant through bounded structured error JSON.
            StorageError::Inspection { .. } => Self::Other,
            StorageError::Io { .. } => Self::Io,
            StorageError::Other { .. } => Self::Other,
        }
    }
}

impl From<std::io::Error> for StorageError {
    fn from(source: std::io::Error) -> Self {
        Self::Io {
            operation: None,
            source,
        }
    }
}

/// Result type alias for storageprims operations.
pub type Result<T> = std::result::Result<T, StorageError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_error_code_maps_from_error() {
        let error = StorageError::ContainerNotFound {
            provider: ProviderKind::S3,
            operation: StorageOperation::Head,
            container: "missing-bucket".to_string(),
        };

        assert_eq!(error.code(), StorageErrorCode::ContainerNotFound);
    }
}
