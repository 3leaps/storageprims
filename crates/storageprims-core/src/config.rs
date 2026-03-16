//! Provider configuration and credential-source types.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::ProviderKind;

/// How a provider should resolve credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum CredentialSource {
    DefaultChain,
    Profile { name: String },
    CredentialsFile { path: String },
    InlineStatic { values: BTreeMap<String, String> },
    Env { variables: Vec<String> },
    InlineEnvMap { values: BTreeMap<String, String> },
}

/// Non-secret provider and target configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TargetConfig {
    pub authority: Option<String>,
    pub container: Option<String>,
    pub root_prefix: Option<String>,
    pub region: Option<String>,
    pub endpoint: Option<String>,
    pub force_path_style: Option<bool>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, String>,
}

/// Canonical provider configuration passed into provider construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider: ProviderKind,
    pub target: TargetConfig,
    pub credentials: CredentialSource,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn provider_config_round_trips_through_json() {
        let mut values = BTreeMap::new();
        values.insert("AWS_ACCESS_KEY_ID".to_string(), "masked".to_string());

        let config = ProviderConfig {
            provider: ProviderKind::S3,
            target: TargetConfig {
                container: Some("example-bucket".to_string()),
                region: Some("us-east-1".to_string()),
                endpoint: Some("https://s3.us-east-1.amazonaws.com".to_string()),
                force_path_style: Some(false),
                ..TargetConfig::default()
            },
            credentials: CredentialSource::InlineEnvMap { values },
        };

        let json = serde_json::to_string(&config).unwrap();
        let decoded: ProviderConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.provider, ProviderKind::S3);
        assert_eq!(decoded.target.container.as_deref(), Some("example-bucket"));
        assert!(matches!(
            decoded.credentials,
            CredentialSource::InlineEnvMap { .. }
        ));
    }
}
