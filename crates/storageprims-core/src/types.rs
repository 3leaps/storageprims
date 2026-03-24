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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PutOptions {
    pub content_length: Option<u64>,
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
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
