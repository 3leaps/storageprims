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
    Profile { name: String },
    CredentialsFile { path: String },
    InlineStatic,
    Env { variables: Vec<String> },
    InlineEnvMap,
    None,
}

impl CredentialSourceKind {
    pub fn from_config(credentials: &CredentialSource) -> Self {
        match credentials {
            CredentialSource::DefaultChain => Self::DefaultChain,
            CredentialSource::Profile { name } => Self::Profile { name: name.clone() },
            CredentialSource::CredentialsFile { path } => {
                Self::CredentialsFile { path: path.clone() }
            }
            CredentialSource::InlineStatic { .. } => Self::InlineStatic,
            CredentialSource::Env { variables } => Self::Env {
                variables: variables.clone(),
            },
            CredentialSource::InlineEnvMap { .. } => Self::InlineEnvMap,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeResult {
    pub provider: ProviderKind,
    pub endpoint: Option<String>,
    pub credential_source: CredentialSourceKind,
    pub probe_method: String,
    pub latency_ms: u64,
    pub capabilities: Vec<Capability>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
