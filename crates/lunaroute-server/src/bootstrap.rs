//! Bootstrap configuration for LunaRoute server
//!
//! This module provides minimal bootstrap configuration that determines:
//! - Whether to use file-based or database-backed configuration
//! - Database connection details for database-backed mode
//! - Configuration file path for file-based mode
//!
//! The bootstrap config is always loaded from a file (default: ~/.lunaroute/bootstrap.yaml)
//! and is intentionally minimal to reduce dependencies during server startup.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Configuration source type
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ConfigSource {
    /// Load configuration from a file (single-tenant mode)
    #[default]
    File,
    /// Load configuration from PostgreSQL database (multi-tenant mode)
    Database,
}

/// Bootstrap configuration
///
/// This is a minimal configuration file that determines how the server
/// loads its full configuration. It supports two modes:
///
/// # File-based mode (single-tenant)
/// ```yaml
/// source: file
/// file_path: ~/.lunaroute/config.yaml
/// ```
///
/// # Database-backed mode (multi-tenant)
/// ```yaml
/// source: database
/// database_url: postgres://localhost/lunaroute
/// # Optional: tenant_id for single-tenant database mode
/// tenant_id: 550e8400-e29b-41d4-a716-446655440000
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapConfig {
    /// Configuration source (file or database)
    #[serde(default)]
    pub source: ConfigSource,

    /// Path to configuration file (for file-based mode)
    /// Default: ~/.lunaroute/config.yaml
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<PathBuf>,

    /// PostgreSQL connection string (for database-backed mode)
    /// Example: postgres://user:pass@localhost/lunaroute
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database_url: Option<String>,

    /// Tenant ID (for database-backed mode)
    /// If None in multi-tenant database mode, the server will use tenant extraction
    /// middleware to determine the tenant from the request.
    /// If Some, the server runs in single-tenant mode with database-backed config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<uuid::Uuid>,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            source: ConfigSource::File,
            file_path: Some(PathBuf::from("~/.lunaroute/config.yaml")),
            database_url: None,
            tenant_id: None,
        }
    }
}

impl BootstrapConfig {
    /// Load bootstrap configuration from file
    ///
    /// # Arguments
    /// * `path` - Path to bootstrap config file (YAML or TOML)
    ///
    /// # Returns
    /// Parsed bootstrap configuration
    ///
    /// # Errors
    /// - File not found
    /// - Invalid YAML/TOML syntax
    /// - Missing required fields for the selected source
    pub fn from_file(path: &str) -> Result<Self, BootstrapError> {
        // Expand tilde in path
        let expanded_path = shellexpand::tilde(path);
        let path = PathBuf::from(expanded_path.as_ref());

        // Read file contents
        let contents = std::fs::read_to_string(&path).map_err(|e| {
            BootstrapError::FileRead(format!("Failed to read {}: {}", path.display(), e))
        })?;

        // Determine format from extension
        let config = if path.extension().and_then(|s| s.to_str()) == Some("toml") {
            // Parse TOML
            toml::from_str(&contents)
                .map_err(|e| BootstrapError::ParseError(format!("TOML parse error: {}", e)))?
        } else {
            // Parse YAML (default)
            serde_yaml::from_str(&contents)
                .map_err(|e| BootstrapError::ParseError(format!("YAML parse error: {}", e)))?
        };

        // Validate configuration
        Self::validate(&config)?;

        Ok(config)
    }

    /// Validate bootstrap configuration
    fn validate(config: &BootstrapConfig) -> Result<(), BootstrapError> {
        match config.source {
            ConfigSource::File => {
                // File mode requires file_path
                if config.file_path.is_none() {
                    return Err(BootstrapError::ValidationError(
                        "file_path is required when source is 'file'".to_string(),
                    ));
                }
            }
            ConfigSource::Database => {
                // Database mode requires database_url
                if config.database_url.is_none() {
                    return Err(BootstrapError::ValidationError(
                        "database_url is required when source is 'database'".to_string(),
                    ));
                }
            }
        }

        Ok(())
    }

    /// Get the expanded file path (with tilde expansion)
    #[allow(dead_code)]
    pub fn expanded_file_path(&self) -> Option<PathBuf> {
        self.file_path.as_ref().map(|path| {
            let path_str = path.to_string_lossy();
            let expanded = shellexpand::tilde(&path_str);
            PathBuf::from(expanded.as_ref())
        })
    }

    /// Check if this is multi-tenant database mode
    #[allow(dead_code)]
    pub fn is_multitenant(&self) -> bool {
        matches!(self.source, ConfigSource::Database) && self.tenant_id.is_none()
    }

    /// Check if this is single-tenant mode (file-based or database with tenant_id)
    #[allow(dead_code)]
    pub fn is_single_tenant(&self) -> bool {
        !self.is_multitenant()
    }
}

/// Bootstrap configuration errors
#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    /// File read error
    #[error("Failed to read bootstrap config: {0}")]
    FileRead(String),

    /// Parse error
    #[error("Failed to parse bootstrap config: {0}")]
    ParseError(String),

    /// Validation error
    #[error("Invalid bootstrap config: {0}")]
    ValidationError(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = BootstrapConfig::default();
        assert_eq!(config.source, ConfigSource::File);
        assert!(config.file_path.is_some());
        assert!(config.database_url.is_none());
        assert!(config.tenant_id.is_none());
        assert!(config.is_single_tenant());
    }

    #[test]
    fn test_file_mode_validation() {
        // Valid file mode
        let config = BootstrapConfig {
            source: ConfigSource::File,
            file_path: Some(PathBuf::from("config.yaml")),
            database_url: None,
            tenant_id: None,
        };
        assert!(BootstrapConfig::validate(&config).is_ok());

        // Invalid: missing file_path
        let config = BootstrapConfig {
            source: ConfigSource::File,
            file_path: None,
            database_url: None,
            tenant_id: None,
        };
        assert!(BootstrapConfig::validate(&config).is_err());
    }

    #[test]
    fn test_database_mode_validation() {
        // Valid database mode
        let config = BootstrapConfig {
            source: ConfigSource::Database,
            file_path: None,
            database_url: Some("postgres://localhost/test".to_string()),
            tenant_id: None,
        };
        assert!(BootstrapConfig::validate(&config).is_ok());

        // Invalid: missing database_url
        let config = BootstrapConfig {
            source: ConfigSource::Database,
            file_path: None,
            database_url: None,
            tenant_id: None,
        };
        assert!(BootstrapConfig::validate(&config).is_err());
    }

    #[test]
    fn test_is_multitenant() {
        // Multi-tenant: database mode without tenant_id
        let config = BootstrapConfig {
            source: ConfigSource::Database,
            file_path: None,
            database_url: Some("postgres://localhost/test".to_string()),
            tenant_id: None,
        };
        assert!(config.is_multitenant());
        assert!(!config.is_single_tenant());

        // Single-tenant: database mode with tenant_id
        let config = BootstrapConfig {
            source: ConfigSource::Database,
            file_path: None,
            database_url: Some("postgres://localhost/test".to_string()),
            tenant_id: Some(uuid::Uuid::new_v4()),
        };
        assert!(!config.is_multitenant());
        assert!(config.is_single_tenant());

        // Single-tenant: file mode
        let config = BootstrapConfig {
            source: ConfigSource::File,
            file_path: Some(PathBuf::from("config.yaml")),
            database_url: None,
            tenant_id: None,
        };
        assert!(!config.is_multitenant());
        assert!(config.is_single_tenant());
    }

    #[test]
    fn test_yaml_serialization() {
        let config = BootstrapConfig {
            source: ConfigSource::File,
            file_path: Some(PathBuf::from("~/.lunaroute/config.yaml")),
            database_url: None,
            tenant_id: None,
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        assert!(yaml.contains("source: file"));
        assert!(yaml.contains("file_path:"));
    }

    #[test]
    fn test_yaml_deserialization() {
        let yaml = r#"
source: database
database_url: postgres://localhost/lunaroute
tenant_id: 550e8400-e29b-41d4-a716-446655440000
"#;

        let config: BootstrapConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.source, ConfigSource::Database);
        assert_eq!(
            config.database_url,
            Some("postgres://localhost/lunaroute".to_string())
        );
        assert!(config.tenant_id.is_some());
    }
}
