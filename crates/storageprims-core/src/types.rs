//! Shared request and result types for the canonical storage contract.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Capability, CredentialSource, ProviderKind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ListOptions {
    pub prefix: Option<String>,
    pub continuation_token: Option<String>,
    pub max_keys: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectSummary {
    pub path: String,
    pub size: u64,
    pub etag: Option<String>,
    pub content_type: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListResult {
    pub objects: Vec<ObjectSummary>,
    pub continuation_token: Option<String>,
    pub is_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectMetadata {
    pub path: String,
    pub size: u64,
    pub etag: Option<String>,
    pub content_type: Option<String>,
    pub last_modified: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetRangeRequest {
    pub key: String,
    pub offset: u64,
    pub length: u64,
}

/// A source selector that a provider must enforce while reading.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceSelector {
    NativeVersion { token: String },
    ValidatorMatch { token: String },
}

impl SourceSelector {
    pub fn validate(&self) -> crate::Result<()> {
        match self {
            Self::NativeVersion { token } => validate_native_version(token),
            Self::ValidatorMatch { token } => validate_strong_etag(token),
        }
    }

    pub fn token(&self) -> &str {
        match self {
            Self::NativeVersion { token } | Self::ValidatorMatch { token } => token,
        }
    }

    pub fn argument_name(&self) -> &'static str {
        match self {
            Self::NativeVersion { .. } => "selector.native_version",
            Self::ValidatorMatch { .. } => "selector.validator",
        }
    }
}

impl std::fmt::Debug for SourceSelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NativeVersion { .. } => f
                .debug_struct("NativeVersion")
                .field("token", &"<redacted>")
                .finish(),
            Self::ValidatorMatch { .. } => f
                .debug_struct("ValidatorMatch")
                .field("token", &"<redacted>")
                .finish(),
        }
    }
}

impl std::fmt::Display for SourceSelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NativeVersion { .. } => f.write_str("native_version(<redacted>)"),
            Self::ValidatorMatch { .. } => f.write_str("validator_match(<redacted>)"),
        }
    }
}

/// A source selector bound by a provider to one configured target and key.
#[derive(Clone, PartialEq, Eq)]
pub struct GuardedReadSelection {
    target_identity: String,
    key: String,
    selector: SourceSelector,
}

impl GuardedReadSelection {
    pub(crate) fn bind(
        target_identity: String,
        key: impl Into<String>,
        selector: SourceSelector,
    ) -> crate::Result<Self> {
        selector.validate()?;
        let key = key.into();
        if key.is_empty() {
            return Err(crate::StorageError::InvalidArgument {
                operation: None,
                argument: "key".to_string(),
                reason: "object key must not be empty".to_string(),
            });
        }
        Ok(Self {
            target_identity,
            key,
            selector,
        })
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn selector(&self) -> &SourceSelector {
        &self.selector
    }

    pub fn validate_for(
        &self,
        target_identity: &str,
        key: &str,
        operation: crate::StorageOperation,
    ) -> crate::Result<()> {
        if self.target_identity != target_identity || self.key != key {
            return Err(crate::StorageError::InvalidArgument {
                operation: Some(operation),
                argument: "selection".to_string(),
                reason: "selection is bound to a different configured target or key".to_string(),
            });
        }
        self.selector.validate()
    }
}

impl std::fmt::Debug for GuardedReadSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuardedReadSelection")
            .field("key", &self.key)
            .field("selector", &self.selector)
            .finish_non_exhaustive()
    }
}

/// Inclusive byte window returned from a guarded range read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteWindow {
    pub start: u64,
    pub end: u64,
}

impl ByteWindow {
    pub fn checked_len(self) -> Option<u64> {
        self.end.checked_sub(self.start)?.checked_add(1)
    }
}

/// Source evidence returned from the same response that supplied metadata or bytes.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceReceipt {
    pub path: String,
    pub native_version: Option<String>,
    pub validator: Option<String>,
    pub total_size: Option<u64>,
    pub requested_window: Option<ByteWindow>,
    pub returned_window: Option<ByteWindow>,
}

impl std::fmt::Debug for SourceReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceReceipt")
            .field("path", &self.path)
            .field(
                "native_version",
                &self.native_version.as_ref().map(|_| "<redacted>"),
            )
            .field("validator", &self.validator.as_ref().map(|_| "<redacted>"))
            .field("total_size", &self.total_size)
            .field("requested_window", &self.requested_window)
            .field("returned_window", &self.returned_window)
            .finish()
    }
}

/// Unconditional source observation. It never authorizes a later unguarded read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceObservation {
    pub receipt: SourceReceipt,
    pub content_type: Option<String>,
    pub last_modified: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

/// A guarded byte response whose receipt describes the same provider response.
pub struct GuardedReadResponse {
    pub receipt: SourceReceipt,
    pub reader: crate::BoxedByteStream,
}

impl std::fmt::Debug for GuardedReadResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuardedReadResponse")
            .field("receipt", &self.receipt)
            .field("reader", &"<owned stream>")
            .finish()
    }
}

/// An offset-and-length request over a previously bound selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardedRangeRequest {
    pub selection: GuardedReadSelection,
    pub offset: u64,
    pub length: u64,
}

fn invalid_selector(argument: &str, reason: &str) -> crate::StorageError {
    crate::StorageError::InvalidArgument {
        operation: None,
        argument: argument.to_string(),
        reason: reason.to_string(),
    }
}

fn validate_native_version(token: &str) -> crate::Result<()> {
    let bytes = token.as_bytes();
    if !(1..=1024).contains(&bytes.len())
        || !bytes.iter().all(|byte| (0x21..=0x7e).contains(byte))
        || bytes
            .iter()
            .any(|byte| matches!(*byte, b'"' | b',' | b'\\'))
        || token.eq_ignore_ascii_case("null")
        || token.eq_ignore_ascii_case("\"null\"")
    {
        return Err(invalid_selector(
            "selector.native_version",
            "native version selector has an invalid format",
        ));
    }
    Ok(())
}

fn validate_strong_etag(token: &str) -> crate::Result<()> {
    let bytes = token.as_bytes();
    let valid = (3..=128).contains(&bytes.len())
        && bytes.first() == Some(&b'"')
        && bytes.last() == Some(&b'"')
        && bytes[1..bytes.len() - 1]
            .iter()
            .all(|byte| *byte == 0x21 || (0x23..=0x7e).contains(byte));
    if !valid {
        return Err(invalid_selector(
            "selector.validator",
            "validator selector must be one strong quoted entity tag",
        ));
    }
    Ok(())
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PutOptions {
    pub content_length: Option<u64>,
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    /// Atomic condition the provider must enforce while writing.
    #[serde(default, skip_serializing_if = "PutPrecondition::is_none")]
    pub precondition: PutPrecondition,
}

impl std::fmt::Debug for PutOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PutOptions")
            .field("content_length", &self.content_length)
            .field("content_type", &self.content_type)
            .field("metadata", &self.metadata)
            .field("precondition", &self.precondition)
            .finish()
    }
}

/// Provider-neutral atomic put condition.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum PutPrecondition {
    /// Preserve the unconditional overwrite behavior.
    #[default]
    None,
    /// Succeed only when the target does not exist.
    MustNotExist,
    /// Succeed only when the provider's opaque version token matches.
    Match {
        /// Opaque token returned by provider metadata or an earlier put.
        token: String,
    },
}

impl PutPrecondition {
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}

impl std::fmt::Debug for PutPrecondition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("None"),
            Self::MustNotExist => f.write_str("MustNotExist"),
            Self::Match { .. } => f
                .debug_struct("Match")
                .field("token", &"<redacted>")
                .finish(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutResult {
    pub path: String,
    pub etag: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyRequest {
    pub source: String,
    pub destination: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyStrategy {
    Native,
    Relay,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyResult {
    pub source: String,
    pub destination: String,
    pub strategy: CopyStrategy,
    pub bytes_copied: Option<u64>,
    pub destination_etag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum CredentialSourceKind {
    DefaultChain,
    Profile,
    CredentialsFile,
    InlineStatic,
    Env,
    InlineEnvMap,
    None,
}

impl CredentialSourceKind {
    pub fn from_config(credentials: &CredentialSource) -> Self {
        match credentials {
            CredentialSource::DefaultChain => Self::DefaultChain,
            CredentialSource::Profile { .. } => Self::Profile,
            CredentialSource::CredentialsFile { .. } => Self::CredentialsFile,
            CredentialSource::InlineStatic { .. } => Self::InlineStatic,
            CredentialSource::Env { .. } => Self::Env,
            CredentialSource::InlineEnvMap { .. } => Self::InlineEnvMap,
        }
    }
}

/// Scope whose reachability was checked by a provider probe.
///
/// A configured-container probe establishes only that the provider can reach
/// the configured container. It does not prove authorization for list, get,
/// put, or any other object operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeScope {
    ConfiguredContainer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeResult {
    pub provider: ProviderKind,
    pub endpoint: Option<String>,
    pub credential_source: CredentialSourceKind,
    pub scope: ProbeScope,
    pub probe_method: String,
    pub latency_ms: u64,
    pub capabilities: Vec<Capability>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_source_kind_serializes_as_class_only() {
        for (credentials, expected_mode) in [
            (
                CredentialSource::Profile {
                    name: "profile-sentinel".to_string(),
                },
                "profile",
            ),
            (
                CredentialSource::CredentialsFile {
                    path: "/credential-file-sentinel".to_string(),
                },
                "credentials_file",
            ),
            (
                CredentialSource::Env {
                    variables: vec!["ENV_NAME_SENTINEL".to_string()],
                },
                "env",
            ),
            (
                CredentialSource::InlineStatic {
                    values: Default::default(),
                },
                "inline_static",
            ),
            (
                CredentialSource::InlineEnvMap {
                    values: Default::default(),
                },
                "inline_env_map",
            ),
        ] {
            assert_eq!(
                serde_json::to_value(CredentialSourceKind::from_config(&credentials))
                    .expect("source kind serializes"),
                serde_json::json!({"mode": expected_mode})
            );
        }
    }

    #[test]
    fn put_options_preserve_unconditional_json_compatibility() {
        let options = PutOptions {
            content_length: Some(4),
            content_type: Some("text/plain".to_string()),
            ..PutOptions::default()
        };

        let value = serde_json::to_value(&options).expect("put options serialize");
        assert_eq!(
            value,
            serde_json::json!({
                "content_length": 4,
                "content_type": "text/plain"
            })
        );
        assert_eq!(
            serde_json::from_value::<PutOptions>(value).expect("put options deserialize"),
            options
        );
    }

    #[test]
    fn put_preconditions_round_trip_through_json() {
        for precondition in [
            PutPrecondition::None,
            PutPrecondition::MustNotExist,
            PutPrecondition::Match {
                token: "W/\"opaque\"".to_string(),
            },
        ] {
            let options = PutOptions {
                precondition,
                ..PutOptions::default()
            };
            let json = serde_json::to_string(&options).expect("put options serialize");
            let decoded =
                serde_json::from_str::<PutOptions>(&json).expect("put options deserialize");
            assert_eq!(decoded, options);
        }
    }

    #[test]
    fn put_precondition_debug_redacts_match_token() {
        let sentinel = "sensitive-token-sentinel";
        let rendered = format!(
            "{:?}",
            PutOptions {
                precondition: PutPrecondition::Match {
                    token: sentinel.to_string(),
                },
                ..PutOptions::default()
            }
        );

        assert!(!rendered.contains(sentinel));
        assert!(rendered.contains("<redacted>"));
        assert!(!format!(
            "{}",
            SourceSelector::NativeVersion {
                token: sentinel.to_string(),
            }
        )
        .contains(sentinel));
    }

    #[test]
    fn guarded_selector_validation_is_strong_and_redacted() {
        let valid_native = SourceSelector::NativeVersion {
            token: "version-1".to_string(),
        };
        valid_native.validate().expect("valid native version");
        let valid_validator = SourceSelector::ValidatorMatch {
            token: "\"etag-1\"".to_string(),
        };
        valid_validator.validate().expect("valid strong entity tag");

        for selector in [
            SourceSelector::NativeVersion {
                token: "NULL".to_string(),
            },
            SourceSelector::NativeVersion {
                token: "version,one".to_string(),
            },
            SourceSelector::ValidatorMatch {
                token: "W/\"weak\"".to_string(),
            },
            SourceSelector::ValidatorMatch {
                token: "*".to_string(),
            },
            SourceSelector::ValidatorMatch {
                token: "unquoted".to_string(),
            },
        ] {
            assert!(matches!(
                selector.validate(),
                Err(crate::StorageError::InvalidArgument { .. })
            ));
        }

        let sentinel = "selector-secret-sentinel";
        let rendered = format!(
            "{:?}",
            SourceSelector::NativeVersion {
                token: sentinel.to_string(),
            }
        );
        assert!(!rendered.contains(sentinel));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn selection_rejects_cross_target_or_key_without_disclosing_token() {
        let sentinel = "selection-secret-sentinel";
        let selection = GuardedReadSelection::bind(
            "target-a".to_string(),
            "object-a",
            SourceSelector::NativeVersion {
                token: sentinel.to_string(),
            },
        )
        .expect("selection binds");
        let error = selection
            .validate_for("target-b", "object-a", crate::StorageOperation::GuardedGet)
            .expect_err("cross target must fail");
        assert!(matches!(error, crate::StorageError::InvalidArgument { .. }));
        assert!(!error.to_string().contains(sentinel));
        assert!(!format!("{selection:?}").contains(sentinel));
    }

    #[test]
    fn source_receipt_debug_redacts_observed_identity() {
        let native = "native-version-sentinel";
        let validator = "\"validator-sentinel\"";
        let receipt = SourceReceipt {
            path: "object".to_string(),
            native_version: Some(native.to_string()),
            validator: Some(validator.to_string()),
            total_size: Some(1),
            requested_window: None,
            returned_window: None,
        };
        let rendered = format!("{receipt:?}");
        assert!(!rendered.contains(native));
        assert!(!rendered.contains(validator));
        assert!(rendered.contains("<redacted>"));
    }
}
