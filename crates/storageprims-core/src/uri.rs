//! URI parsing for storage targets.

use serde::{Deserialize, Serialize};

use crate::error::{ProviderKind, StorageError, StorageOperation};

/// A provider-neutral parsed storage target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageUri {
    pub provider: ProviderKind,
    pub raw: String,
    pub authority: Option<String>,
    pub container: Option<String>,
    pub path: String,
}

impl StorageUri {
    pub fn parse(uri: &str) -> crate::error::Result<Self> {
        if let Some(rest) = uri.strip_prefix("s3://") {
            Self::parse_container_path(uri, rest, ProviderKind::S3)
        } else if let Some(rest) = uri.strip_prefix("gs://") {
            Self::parse_container_path(uri, rest, ProviderKind::Gcs)
        } else if let Some(rest) = uri.strip_prefix("azb://") {
            Self::parse_azure(uri, rest)
        } else if let Some(rest) = uri.strip_prefix("file://") {
            Self::parse_local(uri, rest)
        } else {
            Err(StorageError::InvalidUri {
                uri: uri.to_string(),
                reason: "unsupported scheme (expected s3://, gs://, azb://, or file://)"
                    .to_string(),
            })
        }
    }

    fn parse_container_path(
        raw: &str,
        rest: &str,
        provider: ProviderKind,
    ) -> crate::error::Result<Self> {
        let (container, path) = match rest.split_once('/') {
            Some((container, path)) => (container, path),
            None => (rest, ""),
        };

        if container.is_empty() {
            return Err(StorageError::InvalidUri {
                uri: raw.to_string(),
                reason: "container or bucket name is empty".to_string(),
            });
        }

        Ok(Self {
            provider,
            raw: raw.to_string(),
            authority: None,
            container: Some(container.to_string()),
            path: path.to_string(),
        })
    }

    fn parse_azure(raw: &str, rest: &str) -> crate::error::Result<Self> {
        let parts: Vec<&str> = rest.splitn(3, '/').collect();

        if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() {
            return Err(StorageError::InvalidUri {
                uri: raw.to_string(),
                reason: "Azure URI requires azb://account/container[/path]".to_string(),
            });
        }

        Ok(Self {
            provider: ProviderKind::AzureBlob,
            raw: raw.to_string(),
            authority: Some(parts[0].to_string()),
            container: Some(parts[1].to_string()),
            path: parts.get(2).copied().unwrap_or_default().to_string(),
        })
    }

    fn parse_local(raw: &str, rest: &str) -> crate::error::Result<Self> {
        if !rest.starts_with('/') {
            return Err(StorageError::InvalidArgument {
                operation: Some(StorageOperation::ParseUri),
                argument: raw.to_string(),
                reason: "local file URI must contain an absolute path".to_string(),
            });
        }

        Ok(Self {
            provider: ProviderKind::Local,
            raw: raw.to_string(),
            authority: None,
            container: None,
            path: rest.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_s3_uri() {
        let uri = StorageUri::parse("s3://my-bucket/path/to/object").unwrap();
        assert_eq!(uri.provider, ProviderKind::S3);
        assert_eq!(uri.authority, None);
        assert_eq!(uri.container.as_deref(), Some("my-bucket"));
        assert_eq!(uri.path, "path/to/object");
    }

    #[test]
    fn parse_gs_uri() {
        let uri = StorageUri::parse("gs://my-bucket/data/file.csv").unwrap();
        assert_eq!(uri.provider, ProviderKind::Gcs);
        assert_eq!(uri.container.as_deref(), Some("my-bucket"));
        assert_eq!(uri.path, "data/file.csv");
    }

    #[test]
    fn parse_azure_uri() {
        let uri = StorageUri::parse("azb://myaccount/mycontainer/path/to/blob").unwrap();
        assert_eq!(uri.provider, ProviderKind::AzureBlob);
        assert_eq!(uri.authority.as_deref(), Some("myaccount"));
        assert_eq!(uri.container.as_deref(), Some("mycontainer"));
        assert_eq!(uri.path, "path/to/blob");
    }

    #[test]
    fn parse_local_uri() {
        let uri = StorageUri::parse("file:///tmp/data/export.csv").unwrap();
        assert_eq!(uri.provider, ProviderKind::Local);
        assert_eq!(uri.container, None);
        assert_eq!(uri.path, "/tmp/data/export.csv");
    }

    #[test]
    fn parse_invalid_scheme() {
        let result = StorageUri::parse("http://example.com/file");
        assert!(matches!(result, Err(StorageError::InvalidUri { .. })));
    }

    #[test]
    fn parse_empty_container() {
        let result = StorageUri::parse("s3:///key");
        assert!(matches!(result, Err(StorageError::InvalidUri { .. })));
    }

    #[test]
    fn parse_non_absolute_file_uri() {
        let result = StorageUri::parse("file://tmp/data.csv");
        assert!(matches!(result, Err(StorageError::InvalidArgument { .. })));
    }
}
