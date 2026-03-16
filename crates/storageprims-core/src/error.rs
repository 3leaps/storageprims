//! Canonical error type for storageprims.
//!
//! All provider implementations map their errors into `StorageError` variants.
//! Consumers handle uniform error types regardless of provider.

use std::time::Duration;

use serde::Serialize;

/// Cloud storage provider type identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    S3,
    Gcs,
    Azure,
    Local,
}

impl std::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::S3 => write!(f, "s3"),
            Self::Gcs => write!(f, "gcs"),
            Self::Azure => write!(f, "azure"),
            Self::Local => write!(f, "local"),
        }
    }
}

/// Canonical error type for all storageprims operations.
///
/// Maps provider-specific errors into uniform variants that consumers
/// can handle without knowledge of the underlying provider.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("{provider}: object not found: {key}")]
    NotFound { key: String, provider: ProviderKind },

    #[error("{provider}: access denied: {key} — {detail}")]
    AccessDenied {
        key: String,
        provider: ProviderKind,
        detail: String,
    },

    #[error("{provider}: bucket not found: {bucket}")]
    BucketNotFound {
        bucket: String,
        provider: ProviderKind,
    },

    #[error("{provider}: invalid credentials — {detail}")]
    InvalidCredentials {
        provider: ProviderKind,
        detail: String,
    },

    #[error("{provider}: throttled (retry after {retry_after:?})")]
    Throttled {
        provider: ProviderKind,
        retry_after: Option<Duration>,
    },

    #[error("{provider}: provider unavailable — {detail}")]
    ProviderUnavailable {
        provider: ProviderKind,
        detail: String,
    },

    #[error("invalid URI: {uri} — {reason}")]
    InvalidUri { uri: String, reason: String },

    #[error("I/O error: {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },

    #[error("{provider}: {detail}")]
    Other {
        provider: ProviderKind,
        detail: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
}

/// Result type alias for storageprims operations.
pub type Result<T> = std::result::Result<T, StorageError>;
