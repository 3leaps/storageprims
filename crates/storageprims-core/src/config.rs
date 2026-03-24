//! Provider configuration and credential-source types.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::ProviderKind;

/// How a provider should resolve credentials.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum CredentialSource {
    DefaultChain,
    Profile { name: String },
    CredentialsFile { path: String },
    InlineStatic { values: BTreeMap<String, String> },
    Env { variables: Vec<String> },
    InlineEnvMap { values: BTreeMap<String, String> },
}

impl fmt::Debug for CredentialSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DefaultChain => f.write_str("DefaultChain"),
            Self::Profile { name } => f.debug_struct("Profile").field("name", name).finish(),
            Self::CredentialsFile { path } => f
                .debug_struct("CredentialsFile")
                .field("path", path)
                .finish(),
            Self::InlineStatic { values } => f
                .debug_struct("InlineStatic")
                .field("keys", &values.keys().collect::<Vec<_>>())
                .field("values", &"<redacted>")
                .finish(),
            Self::Env { variables } => f.debug_struct("Env").field("variables", variables).finish(),
            Self::InlineEnvMap { values } => f
                .debug_struct("InlineEnvMap")
                .field("keys", &values.keys().collect::<Vec<_>>())
                .field("values", &"<redacted>")
                .finish(),
        }
    }
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
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider: ProviderKind,
    pub target: TargetConfig,
    pub credentials: CredentialSource,
}

impl fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("provider", &self.provider)
            .field("target", &self.target)
            .field("credentials", &self.credentials)
            .finish()
    }
}

pub fn sanitize_endpoint(endpoint: &str) -> String {
    let Some(scheme_offset) = endpoint.find("://") else {
        return endpoint.to_string();
    };

    let authority_start = scheme_offset + 3;
    let authority_end = endpoint[authority_start..]
        .find(['/', '?', '#'])
        .map(|offset| authority_start + offset)
        .unwrap_or(endpoint.len());
    let authority = &endpoint[authority_start..authority_end];

    let Some(at_offset) = authority.rfind('@') else {
        return endpoint.to_string();
    };

    let mut sanitized = String::with_capacity(endpoint.len());
    sanitized.push_str(&endpoint[..authority_start]);
    sanitized.push_str("***@");
    sanitized.push_str(&authority[at_offset + 1..]);
    sanitized.push_str(&endpoint[authority_end..]);
    sanitized
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

    #[test]
    fn debug_redacts_inline_credential_values() {
        let mut values = BTreeMap::new();
        values.insert("AWS_ACCESS_KEY_ID".to_string(), "AKIASECRET".to_string());
        values.insert(
            "AWS_SECRET_ACCESS_KEY".to_string(),
            "super-secret-value".to_string(),
        );

        let config = ProviderConfig {
            provider: ProviderKind::S3,
            target: TargetConfig::default(),
            credentials: CredentialSource::InlineStatic { values },
        };

        let debug = format!("{config:?}");
        assert!(!debug.contains("AKIASECRET"));
        assert!(!debug.contains("super-secret-value"));
        assert!(debug.contains("AWS_ACCESS_KEY_ID"));
        assert!(debug.contains("AWS_SECRET_ACCESS_KEY"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn debug_redacts_inline_env_map_values() {
        let mut values = BTreeMap::new();
        values.insert("ACCESS_TOKEN".to_string(), "very-secret-token".to_string());

        let credentials = CredentialSource::InlineEnvMap { values };
        let debug = format!("{credentials:?}");

        assert!(!debug.contains("very-secret-token"));
        assert!(debug.contains("ACCESS_TOKEN"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn sanitize_endpoint_strips_userinfo() {
        assert_eq!(
            sanitize_endpoint("https://user:pass@example.com:9000/path?x=1"),
            "https://***@example.com:9000/path?x=1"
        );
    }

    #[test]
    fn sanitize_endpoint_leaves_plain_urls_unchanged() {
        assert_eq!(
            sanitize_endpoint("https://example.com:9000/path?x=1"),
            "https://example.com:9000/path?x=1"
        );
    }
}
