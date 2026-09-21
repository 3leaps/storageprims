//! Storage provider traits and capability discovery.

use std::future::Future;
use std::pin::Pin;

use tokio::io::AsyncRead;

use crate::error::{ProviderKind, Result};
use crate::types::{
    CopyRequest, CopyResult, DelimiterListRequest, DelimiterListResult, GetRangeRequest,
    GuardedRangeRequest, GuardedReadResponse, GuardedReadSelection, ListOptions, ListResult,
    ObjectMetadata, ProbeResult, PutOptions, PutResult, SourceObservation,
};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;
pub type BoxedByteStream = Box<dyn AsyncRead + Send + Unpin>;

/// Optional provider capabilities beyond the universal contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    DelimiterListing,
    MultipartUpload,
    CredentialProbe,
    /// Atomic provider-side put preconditions.
    ConditionalPut,
    /// Source-enforced reads with same-response source receipts.
    GuardedRead,
}

/// Optional native delimiter/common-prefix listing operation.
pub trait DelimiterListingProvider: Send + Sync {
    fn list_delimited(&self, request: DelimiterListRequest) -> BoxFuture<'_, DelimiterListResult>;
}

/// Return native delimiter listing or the canonical refusal.
pub fn require_delimiter_listing(
    provider: &dyn StorageProvider,
) -> Result<&dyn DelimiterListingProvider> {
    provider
        .delimiter_lists()
        .ok_or(crate::error::StorageError::UnsupportedCapability {
            provider: provider.provider_kind(),
            operation: crate::error::StorageOperation::List,
            capability: Capability::DelimiterListing,
        })
}

/// Optional source-enforced read operations.
pub trait GuardedReadProvider: Send + Sync {
    /// A stable, credential-free identity for this configured target.
    fn guarded_read_target_identity(&self) -> String;

    /// Bind an externally supplied selector to this provider target and key.
    fn bind_guarded_read(
        &self,
        key: &str,
        selector: crate::types::SourceSelector,
    ) -> Result<GuardedReadSelection> {
        GuardedReadSelection::bind(self.guarded_read_target_identity(), key, selector)
    }

    fn observe_source(&self, key: &str) -> BoxFuture<'_, SourceObservation>;

    fn guarded_head(
        &self,
        selection: GuardedReadSelection,
    ) -> BoxFuture<'_, crate::types::SourceReceipt>;

    fn guarded_get(&self, selection: GuardedReadSelection) -> BoxFuture<'_, GuardedReadResponse>;

    fn guarded_get_range(&self, request: GuardedRangeRequest)
        -> BoxFuture<'_, GuardedReadResponse>;
}

/// Return the source-enforced-read extension or the canonical refusal.
pub fn require_guarded_reads(
    provider: &dyn StorageProvider,
    operation: crate::error::StorageOperation,
) -> Result<&dyn GuardedReadProvider> {
    provider
        .guarded_reads()
        .ok_or(crate::error::StorageError::UnsupportedCapability {
            provider: provider.provider_kind(),
            operation,
            capability: Capability::GuardedRead,
        })
}

/// Canonical storage provider trait for v0.1.
pub trait StorageProvider: Send + Sync {
    fn provider_kind(&self) -> ProviderKind;

    fn capabilities(&self) -> Vec<Capability>;

    fn has_capability(&self, capability: Capability) -> bool {
        self.capabilities().contains(&capability)
    }

    /// Return source-enforced reads when callable from this Rust surface.
    fn guarded_reads(&self) -> Option<&dyn GuardedReadProvider> {
        None
    }

    /// Return native delimiter listing when callable from this Rust surface.
    fn delimiter_lists(&self) -> Option<&dyn DelimiterListingProvider> {
        None
    }

    fn list(&self, options: ListOptions) -> BoxFuture<'_, ListResult>;

    fn head(&self, key: &str) -> BoxFuture<'_, ObjectMetadata>;

    fn get(&self, key: &str) -> BoxFuture<'_, BoxedByteStream>;

    fn get_range(&self, request: GetRangeRequest) -> BoxFuture<'_, BoxedByteStream>;

    fn put(
        &self,
        key: &str,
        body: BoxedByteStream,
        options: PutOptions,
    ) -> BoxFuture<'_, PutResult>;

    fn delete(&self, key: &str) -> BoxFuture<'_, ()>;

    fn copy(&self, request: CopyRequest) -> BoxFuture<'_, CopyResult>;

    /// Check reachability of the provider's configured container.
    ///
    /// A successful probe does not establish authorization for individual
    /// object operations such as list, get, or put.
    fn probe(&self) -> BoxFuture<'_, ProbeResult> {
        Box::pin(async move {
            Err(crate::error::StorageError::UnsupportedCapability {
                provider: self.provider_kind(),
                operation: crate::error::StorageOperation::Probe,
                capability: Capability::CredentialProbe,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::types::{CopyStrategy, ObjectSummary, PutPrecondition};

    struct DummyProvider;

    impl StorageProvider for DummyProvider {
        fn provider_kind(&self) -> ProviderKind {
            ProviderKind::Local
        }

        fn capabilities(&self) -> Vec<Capability> {
            Vec::new()
        }

        fn list(&self, _options: ListOptions) -> BoxFuture<'_, ListResult> {
            Box::pin(async move {
                Ok(ListResult {
                    objects: vec![ObjectSummary {
                        path: "demo.txt".to_string(),
                        size: 4,
                        etag: None,
                        content_type: Some("text/plain".to_string()),
                        last_modified: None,
                    }],
                    continuation_token: None,
                    is_truncated: false,
                })
            })
        }

        fn head(&self, key: &str) -> BoxFuture<'_, ObjectMetadata> {
            let key = key.to_string();
            Box::pin(async move {
                Ok(ObjectMetadata {
                    path: key,
                    size: 4,
                    etag: None,
                    content_type: Some("text/plain".to_string()),
                    last_modified: None,
                    metadata: BTreeMap::new(),
                })
            })
        }

        fn get(&self, _key: &str) -> BoxFuture<'_, BoxedByteStream> {
            Box::pin(async move { Ok(Box::new(tokio::io::empty()) as BoxedByteStream) })
        }

        fn get_range(&self, _request: GetRangeRequest) -> BoxFuture<'_, BoxedByteStream> {
            Box::pin(async move { Ok(Box::new(tokio::io::empty()) as BoxedByteStream) })
        }

        fn put(
            &self,
            key: &str,
            _body: BoxedByteStream,
            options: PutOptions,
        ) -> BoxFuture<'_, PutResult> {
            let key = key.to_string();
            Box::pin(async move {
                if !options.precondition.is_none() {
                    return Err(crate::error::StorageError::UnsupportedCapability {
                        provider: ProviderKind::Local,
                        operation: crate::error::StorageOperation::Put,
                        capability: Capability::ConditionalPut,
                    });
                }
                Ok(PutResult {
                    path: key,
                    etag: Some("etag-1".to_string()),
                    size: options.content_length,
                })
            })
        }

        fn delete(&self, _key: &str) -> BoxFuture<'_, ()> {
            Box::pin(async move { Ok(()) })
        }

        fn copy(&self, request: CopyRequest) -> BoxFuture<'_, CopyResult> {
            Box::pin(async move {
                Ok(CopyResult {
                    source: request.source,
                    destination: request.destination,
                    strategy: CopyStrategy::Native,
                    bytes_copied: Some(4),
                    destination_etag: Some("etag-2".to_string()),
                })
            })
        }
    }

    #[test]
    fn trait_is_object_safe() {
        let provider: Box<dyn StorageProvider> = Box::new(DummyProvider);
        assert_eq!(provider.provider_kind(), ProviderKind::Local);
        assert!(!provider.has_capability(Capability::DelimiterListing));
        assert!(!provider.has_capability(Capability::MultipartUpload));
    }

    #[test]
    fn default_probe_returns_unsupported_capability() {
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
        let provider = DummyProvider;
        let error = runtime
            .block_on(provider.probe())
            .expect_err("default probe should be unsupported");

        assert!(matches!(
            error,
            crate::error::StorageError::UnsupportedCapability {
                provider: ProviderKind::Local,
                operation: crate::error::StorageOperation::Probe,
                capability: Capability::CredentialProbe,
            }
        ));
    }

    #[test]
    fn external_style_provider_keeps_default_guarded_read_refusal() {
        let provider: Box<dyn StorageProvider> = Box::new(DummyProvider);
        assert!(provider.guarded_reads().is_none());
        let error =
            match require_guarded_reads(provider.as_ref(), crate::StorageOperation::GuardedGet) {
                Ok(_) => panic!("default hook must refuse the optional extension"),
                Err(error) => error,
            };
        assert!(matches!(
            error,
            crate::StorageError::UnsupportedCapability {
                capability: Capability::GuardedRead,
                ..
            }
        ));
    }

    #[test]
    fn provider_without_conditional_put_refuses_precondition() {
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
        let provider = DummyProvider;
        let error = runtime
            .block_on(provider.put(
                "demo.txt",
                Box::new(tokio::io::empty()),
                PutOptions {
                    precondition: PutPrecondition::MustNotExist,
                    ..PutOptions::default()
                },
            ))
            .expect_err("conditional put should be unsupported");

        assert!(matches!(
            error,
            crate::error::StorageError::UnsupportedCapability {
                provider: ProviderKind::Local,
                operation: crate::error::StorageOperation::Put,
                capability: Capability::ConditionalPut,
            }
        ));
    }

    #[test]
    fn external_style_provider_keeps_default_delimiter_listing_refusal() {
        let provider: Box<dyn StorageProvider> = Box::new(DummyProvider);
        assert!(provider.delimiter_lists().is_none());
        let error = match require_delimiter_listing(provider.as_ref()) {
            Ok(_) => panic!("default hook must refuse the optional extension"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            crate::StorageError::UnsupportedCapability {
                capability: Capability::DelimiterListing,
                operation: crate::StorageOperation::List,
                ..
            }
        ));
    }
}
