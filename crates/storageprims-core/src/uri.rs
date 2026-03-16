//! URI parsing for cloud storage resources.
//!
//! Parses uniform URI schemes into structured provider-specific components:
//!
//! - `s3://bucket/key/path`          → S3
//! - `gs://bucket/object/path`       → GCS
//! - `azb://account/container/blob`  → Azure Blob
//! - `file:///absolute/path`         → Local filesystem

use crate::error::{ProviderKind, StorageError};

/// Identifies which cloud storage provider a URI targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderType {
    S3,
    Gcs,
    Azure,
    Local,
}

impl From<ProviderType> for ProviderKind {
    fn from(pt: ProviderType) -> Self {
        match pt {
            ProviderType::S3 => ProviderKind::S3,
            ProviderType::Gcs => ProviderKind::Gcs,
            ProviderType::Azure => ProviderKind::Azure,
            ProviderType::Local => ProviderKind::Local,
        }
    }
}

/// A parsed cloud storage URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageUri {
    /// The provider type determined from the URI scheme.
    pub provider: ProviderType,
    /// The raw URI string.
    pub raw: String,
    /// Bucket name (S3, GCS) or account name (Azure). Empty for local.
    pub bucket: String,
    /// Container name (Azure only).
    pub container: Option<String>,
    /// Object key or file path.
    pub key: String,
}

impl StorageUri {
    /// Parse a URI string into a `StorageUri`.
    ///
    /// # Supported schemes
    ///
    /// - `s3://bucket/key`
    /// - `gs://bucket/key`
    /// - `azb://account/container/key`
    /// - `file:///path`
    pub fn parse(uri: &str) -> crate::error::Result<Self> {
        if let Some(rest) = uri.strip_prefix("s3://") {
            Self::parse_bucket_key(uri, rest, ProviderType::S3)
        } else if let Some(rest) = uri.strip_prefix("gs://") {
            Self::parse_bucket_key(uri, rest, ProviderType::Gcs)
        } else if let Some(rest) = uri.strip_prefix("azb://") {
            Self::parse_azure(uri, rest)
        } else if let Some(rest) = uri.strip_prefix("file://") {
            Ok(StorageUri {
                provider: ProviderType::Local,
                raw: uri.to_string(),
                bucket: String::new(),
                container: None,
                key: rest.to_string(),
            })
        } else {
            Err(StorageError::InvalidUri {
                uri: uri.to_string(),
                reason: "unsupported scheme (expected s3://, gs://, azb://, or file://)".into(),
            })
        }
    }

    fn parse_bucket_key(
        raw: &str,
        rest: &str,
        provider: ProviderType,
    ) -> crate::error::Result<Self> {
        let (bucket, key) = match rest.split_once('/') {
            Some((b, k)) => (b.to_string(), k.to_string()),
            None => (rest.to_string(), String::new()),
        };

        if bucket.is_empty() {
            return Err(StorageError::InvalidUri {
                uri: raw.to_string(),
                reason: "bucket name is empty".into(),
            });
        }

        Ok(StorageUri {
            provider,
            raw: raw.to_string(),
            bucket,
            container: None,
            key,
        })
    }

    fn parse_azure(raw: &str, rest: &str) -> crate::error::Result<Self> {
        let parts: Vec<&str> = rest.splitn(3, '/').collect();
        match parts.len() {
            0 | 1 => Err(StorageError::InvalidUri {
                uri: raw.to_string(),
                reason: "Azure URI requires azb://account/container[/key]".into(),
            }),
            2 => Ok(StorageUri {
                provider: ProviderType::Azure,
                raw: raw.to_string(),
                bucket: parts[0].to_string(),
                container: Some(parts[1].to_string()),
                key: String::new(),
            }),
            _ => Ok(StorageUri {
                provider: ProviderType::Azure,
                raw: raw.to_string(),
                bucket: parts[0].to_string(),
                container: Some(parts[1].to_string()),
                key: parts[2].to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_s3_uri() {
        let uri = StorageUri::parse("s3://my-bucket/path/to/object").unwrap();
        assert_eq!(uri.provider, ProviderType::S3);
        assert_eq!(uri.bucket, "my-bucket");
        assert_eq!(uri.key, "path/to/object");
    }

    #[test]
    fn parse_s3_bucket_only() {
        let uri = StorageUri::parse("s3://my-bucket").unwrap();
        assert_eq!(uri.provider, ProviderType::S3);
        assert_eq!(uri.bucket, "my-bucket");
        assert_eq!(uri.key, "");
    }

    #[test]
    fn parse_gs_uri() {
        let uri = StorageUri::parse("gs://my-bucket/data/file.csv").unwrap();
        assert_eq!(uri.provider, ProviderType::Gcs);
        assert_eq!(uri.bucket, "my-bucket");
        assert_eq!(uri.key, "data/file.csv");
    }

    #[test]
    fn parse_azure_uri() {
        let uri = StorageUri::parse("azb://myaccount/mycontainer/path/to/blob").unwrap();
        assert_eq!(uri.provider, ProviderType::Azure);
        assert_eq!(uri.bucket, "myaccount");
        assert_eq!(uri.container, Some("mycontainer".into()));
        assert_eq!(uri.key, "path/to/blob");
    }

    #[test]
    fn parse_local_uri() {
        let uri = StorageUri::parse("file:///tmp/data/export.csv").unwrap();
        assert_eq!(uri.provider, ProviderType::Local);
        assert_eq!(uri.key, "/tmp/data/export.csv");
    }

    #[test]
    fn parse_invalid_scheme() {
        let result = StorageUri::parse("http://example.com/file");
        assert!(result.is_err());
    }

    #[test]
    fn parse_empty_bucket() {
        let result = StorageUri::parse("s3:///key");
        assert!(result.is_err());
    }
}
