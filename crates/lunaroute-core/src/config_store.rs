//! Configuration store trait for multi-tenancy support
//!
//! The `ConfigStore` trait provides an abstraction over configuration storage,
//! allowing different implementations for single-tenant (file-based) and
//! multi-tenant (database-backed) deployments.

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::{tenant::TenantId, Error, Result};

/// Type alias for configuration change streams
pub type ConfigChangeStream<'a> = BoxStream<'a, Result<ConfigChange>>;

/// Configuration change notification
#[derive(Debug, Clone)]
pub struct ConfigChange {
    /// Tenant ID (None for single-tenant mode)
    pub tenant_id: Option<TenantId>,

    /// Timestamp of the change
    pub timestamp: chrono::DateTime<chrono::Utc>,

    /// Configuration version
    pub version: u32,
}

/// Configuration store trait
///
/// Implementations:
/// - `FileConfigStore`: File-based configuration (single-tenant)
/// - `PostgresConfigStore`: Database-backed configuration (multi-tenant)
///
/// # Example
/// ```no_run
/// # use lunaroute_core::config_store::ConfigStore;
/// # use lunaroute_core::tenant::TenantId;
/// # async fn example(store: &dyn ConfigStore) -> lunaroute_core::Result<()> {
/// // Single-tenant mode
/// let config = store.get_config(None).await?;
///
/// // Multi-tenant mode
/// let tenant_id = TenantId::new();
/// let config = store.get_config(Some(tenant_id)).await?;
/// # Ok(())
/// # }
/// ```
#[async_trait]
pub trait ConfigStore: Send + Sync {
    /// Get configuration for a tenant
    ///
    /// # Arguments
    /// * `tenant_id` - Optional tenant ID (None for single-tenant mode)
    ///
    /// # Returns
    /// The server configuration as a JSON value.
    /// Implementations should deserialize this into `ServerConfig`.
    ///
    /// # Errors
    /// - `Error::ConfigNotFound` if config doesn't exist
    /// - `Error::TenantRequired` if tenant_id is None in multi-tenant mode
    /// - `Error::Database` for database errors
    async fn get_config(&self, tenant_id: Option<TenantId>) -> Result<serde_json::Value>;

    /// Update configuration for a tenant
    ///
    /// # Arguments
    /// * `tenant_id` - Optional tenant ID (None for single-tenant mode)
    /// * `config` - New configuration as JSON
    ///
    /// # Errors
    /// - `Error::ConfigValidation` if config is invalid
    /// - `Error::TenantRequired` if tenant_id is None in multi-tenant mode
    /// - `Error::Database` for database errors
    async fn update_config(
        &self,
        tenant_id: Option<TenantId>,
        config: serde_json::Value,
    ) -> Result<()>;

    /// Watch for configuration changes
    ///
    /// Returns a stream of configuration change notifications.
    /// The stream should emit whenever the configuration is updated.
    ///
    /// # Arguments
    /// * `tenant_id` - Optional tenant ID (None for single-tenant mode)
    ///
    /// # Implementation Notes
    /// - File-based: Use `notify` crate to watch file changes
    /// - Database-based: Use PostgreSQL LISTEN/NOTIFY
    async fn watch_changes(&self, tenant_id: Option<TenantId>) -> Result<ConfigChangeStream<'_>>;

    /// Validate configuration before saving
    ///
    /// Implementations should perform schema validation and
    /// check for required fields.
    ///
    /// # Arguments
    /// * `config` - Configuration to validate as JSON
    ///
    /// # Errors
    /// - `Error::ConfigValidation` if validation fails
    async fn validate_config(&self, config: &serde_json::Value) -> Result<()>;

    /// List all tenant IDs (multi-tenant only)
    ///
    /// Returns empty vec in single-tenant mode.
    async fn list_tenants(&self) -> Result<Vec<TenantId>> {
        Ok(Vec::new())
    }

    /// Delete configuration for a tenant (multi-tenant only)
    ///
    /// # Arguments
    /// * `tenant_id` - Tenant ID to delete
    ///
    /// # Errors
    /// - `Error::TenantNotFound` if tenant doesn't exist
    /// - `Error::Database` for database errors
    async fn delete_config(&self, _tenant_id: TenantId) -> Result<()> {
        Err(Error::Internal(
            "Delete not supported in single-tenant mode".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_change_creation() {
        let change = ConfigChange {
            tenant_id: Some(TenantId::new()),
            timestamp: chrono::Utc::now(),
            version: 1,
        };

        assert!(change.tenant_id.is_some());
        assert_eq!(change.version, 1);
    }
}
