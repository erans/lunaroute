//! Provider configuration and management
//!
//! Defines provider types, configuration, and environment variable resolution.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

/// Provider type determines API format/dialect
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderType {
    /// OpenAI API format (api.openai.com and compatible providers)
    OpenAI,
    /// Anthropic API format (api.anthropic.com and compatible providers)
    Anthropic,
}

impl ProviderType {
    /// Get default base URL for this provider type
    pub fn default_base_url(&self) -> &'static str {
        match self {
            ProviderType::OpenAI => "https://api.openai.com/v1",
            ProviderType::Anthropic => "https://api.anthropic.com",
        }
    }
}

/// Provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Provider type (determines API format)
    #[serde(rename = "type")]
    pub provider_type: ProviderType,

    /// API key (supports env var syntax: $VAR_NAME or ${VAR_NAME})
    pub api_key: String,

    /// Base URL (optional, defaults based on provider type)
    #[serde(default)]
    pub base_url: Option<String>,

    /// Custom headers to add to requests (supports env vars in values)
    #[serde(default)]
    pub headers: HashMap<String, String>,

    /// Request timeout in seconds (optional)
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

impl ProviderConfig {
    /// Get the effective base URL (configured or default)
    pub fn effective_base_url(&self) -> &str {
        self.base_url
            .as_deref()
            .unwrap_or_else(|| self.provider_type.default_base_url())
    }

    /// Resolve environment variables in configuration
    /// Replaces $VAR_NAME or ${VAR_NAME} with actual env var values
    pub fn resolve_env_vars(&mut self) -> Result<(), ProviderConfigError> {
        // Resolve API key
        self.api_key = resolve_env_var(&self.api_key)?;

        // Resolve headers
        for (key, value) in self.headers.iter_mut() {
            *value = resolve_env_var(value)
                .map_err(|e| ProviderConfigError::EnvVarResolution {
                    header: key.clone(),
                    source: Box::new(e),
                })?;
        }

        Ok(())
    }
}

/// Resolve a single environment variable reference
/// Supports: $VAR_NAME or ${VAR_NAME}
/// If no $ prefix, returns value as-is
fn resolve_env_var(value: &str) -> Result<String, ProviderConfigError> {
    let trimmed = value.trim();

    // Check for $VAR_NAME or ${VAR_NAME}
    if let Some(var_name) = trimmed.strip_prefix('$') {
        let var_name = var_name
            .strip_prefix('{')
            .and_then(|s| s.strip_suffix('}'))
            .unwrap_or(var_name);

        std::env::var(var_name).map_err(|_| ProviderConfigError::EnvVarNotFound {
            var_name: var_name.to_string(),
        })
    } else {
        // No $ prefix, return as-is
        Ok(value.to_string())
    }
}

/// Provider configuration errors
#[derive(Debug, Error)]
pub enum ProviderConfigError {
    #[error("Environment variable not found: {var_name}")]
    EnvVarNotFound { var_name: String },

    #[error("Failed to resolve environment variable in header '{header}'")]
    EnvVarResolution {
        header: String,
        #[source]
        source: Box<ProviderConfigError>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_type_default_urls() {
        assert_eq!(
            ProviderType::OpenAI.default_base_url(),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            ProviderType::Anthropic.default_base_url(),
            "https://api.anthropic.com"
        );
    }

    #[test]
    fn test_effective_base_url_default() {
        let config = ProviderConfig {
            provider_type: ProviderType::OpenAI,
            api_key: "test-key".to_string(),
            base_url: None,
            headers: HashMap::new(),
            timeout_secs: None,
        };

        assert_eq!(config.effective_base_url(), "https://api.openai.com/v1");
    }

    #[test]
    fn test_effective_base_url_custom() {
        let config = ProviderConfig {
            provider_type: ProviderType::OpenAI,
            api_key: "test-key".to_string(),
            base_url: Some("https://custom.com/v1".to_string()),
            headers: HashMap::new(),
            timeout_secs: None,
        };

        assert_eq!(config.effective_base_url(), "https://custom.com/v1");
    }

    #[test]
    fn test_resolve_env_var_literal() {
        assert_eq!(resolve_env_var("literal-value").unwrap(), "literal-value");
        assert_eq!(resolve_env_var("sk-abc123").unwrap(), "sk-abc123");
    }

    #[test]
    #[serial_test::serial]
    fn test_resolve_env_var_with_dollar() {
        // Set test env var (serialized test prevents race conditions)
        unsafe {
            std::env::set_var("TEST_VAR_123", "test-value");
        }

        assert_eq!(resolve_env_var("$TEST_VAR_123").unwrap(), "test-value");
        assert_eq!(resolve_env_var("${TEST_VAR_123}").unwrap(), "test-value");

        // Cleanup
        unsafe {
            std::env::remove_var("TEST_VAR_123");
        }
    }

    #[test]
    fn test_resolve_env_var_not_found() {
        let result = resolve_env_var("$NONEXISTENT_VAR_XYZ");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("NONEXISTENT_VAR_XYZ"));
    }

    #[test]
    #[serial_test::serial]
    fn test_provider_config_resolve_api_key() {
        unsafe {
            std::env::set_var("TEST_API_KEY_456", "secret-key");
        }

        let mut config = ProviderConfig {
            provider_type: ProviderType::OpenAI,
            api_key: "$TEST_API_KEY_456".to_string(),
            base_url: None,
            headers: HashMap::new(),
            timeout_secs: None,
        };

        config.resolve_env_vars().unwrap();
        assert_eq!(config.api_key, "secret-key");

        unsafe {
            std::env::remove_var("TEST_API_KEY_456");
        }
    }

    #[test]
    #[serial_test::serial]
    fn test_provider_config_resolve_headers() {
        unsafe {
            std::env::set_var("TEST_HEADER_VAL", "header-value");
        }

        let mut headers = HashMap::new();
        headers.insert("X-Custom".to_string(), "$TEST_HEADER_VAL".to_string());
        headers.insert("X-Static".to_string(), "static-value".to_string());

        let mut config = ProviderConfig {
            provider_type: ProviderType::OpenAI,
            api_key: "key".to_string(),
            base_url: None,
            headers,
            timeout_secs: None,
        };

        config.resolve_env_vars().unwrap();
        assert_eq!(config.headers.get("X-Custom").unwrap(), "header-value");
        assert_eq!(config.headers.get("X-Static").unwrap(), "static-value");

        unsafe {
            std::env::remove_var("TEST_HEADER_VAL");
        }
    }

    #[test]
    fn test_provider_config_serde() {
        let config = ProviderConfig {
            provider_type: ProviderType::Anthropic,
            api_key: "$ANTHROPIC_KEY".to_string(),
            base_url: Some("https://custom.anthropic.com".to_string()),
            headers: {
                let mut h = HashMap::new();
                h.insert("X-Custom".to_string(), "value".to_string());
                h
            },
            timeout_secs: Some(60),
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        let deserialized: ProviderConfig = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(deserialized.provider_type, ProviderType::Anthropic);
        assert_eq!(deserialized.api_key, "$ANTHROPIC_KEY");
        assert_eq!(deserialized.base_url, Some("https://custom.anthropic.com".to_string()));
    }
}
