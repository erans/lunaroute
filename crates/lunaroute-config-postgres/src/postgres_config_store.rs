//! PostgresConfigStore - ConfigStore trait implementation for PostgreSQL multi-tenant storage

use async_trait::async_trait;
use futures::StreamExt;
use sqlx::{PgPool, Row, postgres::PgListener};
use std::sync::Arc;

use lunaroute_core::{
    Error, Result,
    config_store::{ConfigChange, ConfigChangeStream, ConfigStore},
    tenant::TenantId,
};

/// PostgreSQL-backed configuration store for multi-tenant mode
///
/// Stores tenant configurations in a PostgreSQL database with:
/// - JSONB column for flexible config schema
/// - Version tracking for optimistic concurrency
/// - Audit history for all changes
/// - LISTEN/NOTIFY for real-time updates
#[derive(Clone)]
pub struct PostgresConfigStore {
    /// PostgreSQL connection pool
    pool: Arc<PgPool>,
}

impl PostgresConfigStore {
    /// Create a new PostgreSQL configuration store
    ///
    /// # Arguments
    /// * `database_url` - PostgreSQL connection string
    ///
    /// # Errors
    /// - `Error::Database` if connection fails or schema migration fails
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = PgPool::connect(database_url)
            .await
            .map_err(|e| Error::Database(format!("Failed to connect to PostgreSQL: {}", e)))?;

        let store = Self {
            pool: Arc::new(pool),
        };

        // Run schema migrations
        store.run_migrations().await?;

        Ok(store)
    }

    /// Create from an existing pool (useful for testing)
    pub fn from_pool(pool: PgPool) -> Self {
        Self {
            pool: Arc::new(pool),
        }
    }

    /// Run database schema migrations
    async fn run_migrations(&self) -> Result<()> {
        // Create tenant_configs table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS tenant_configs (
                tenant_id UUID PRIMARY KEY,
                config JSONB NOT NULL,
                created_at TIMESTAMPTZ DEFAULT NOW(),
                updated_at TIMESTAMPTZ DEFAULT NOW(),
                version INT NOT NULL DEFAULT 1,
                CONSTRAINT valid_config CHECK (jsonb_typeof(config) = 'object')
            )
            "#,
        )
        .execute(&*self.pool)
        .await
        .map_err(|e| Error::Database(format!("Failed to create tenant_configs table: {}", e)))?;

        // Create index for updated_at
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_tenant_configs_updated
            ON tenant_configs(updated_at DESC)
            "#,
        )
        .execute(&*self.pool)
        .await
        .map_err(|e| Error::Database(format!("Failed to create index: {}", e)))?;

        // Create audit history table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS tenant_config_history (
                id BIGSERIAL PRIMARY KEY,
                tenant_id UUID NOT NULL,
                config JSONB NOT NULL,
                version INT NOT NULL,
                changed_by TEXT,
                changed_at TIMESTAMPTZ DEFAULT NOW(),
                FOREIGN KEY (tenant_id) REFERENCES tenant_configs(tenant_id) ON DELETE CASCADE
            )
            "#,
        )
        .execute(&*self.pool)
        .await
        .map_err(|e| {
            Error::Database(format!(
                "Failed to create tenant_config_history table: {}",
                e
            ))
        })?;

        // Create index for history lookups
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_tenant_config_history_tenant
            ON tenant_config_history(tenant_id, changed_at DESC)
            "#,
        )
        .execute(&*self.pool)
        .await
        .map_err(|e| Error::Database(format!("Failed to create history index: {}", e)))?;

        Ok(())
    }

    /// Get the underlying connection pool
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[async_trait]
impl ConfigStore for PostgresConfigStore {
    async fn get_config(&self, tenant_id: Option<TenantId>) -> Result<serde_json::Value> {
        // Multi-tenant mode only - require tenant_id
        let tenant_id = tenant_id.ok_or_else(|| {
            Error::TenantRequired(
                "PostgresConfigStore requires a tenant_id (multi-tenant mode only)".to_string(),
            )
        })?;

        // Query config from database
        let row = sqlx::query("SELECT config FROM tenant_configs WHERE tenant_id = $1")
            .bind(tenant_id.as_uuid())
            .fetch_optional(&*self.pool)
            .await
            .map_err(|e| Error::Database(format!("Failed to query config: {}", e)))?;

        match row {
            Some(row) => {
                let config: serde_json::Value = row
                    .try_get("config")
                    .map_err(|e| Error::Database(format!("Failed to extract config: {}", e)))?;
                Ok(config)
            }
            None => Err(Error::ConfigNotFound),
        }
    }

    async fn update_config(
        &self,
        tenant_id: Option<TenantId>,
        config: serde_json::Value,
    ) -> Result<()> {
        // Multi-tenant mode only - require tenant_id
        let tenant_id = tenant_id.ok_or_else(|| {
            Error::TenantRequired(
                "PostgresConfigStore requires a tenant_id (multi-tenant mode only)".to_string(),
            )
        })?;

        // Validate config first
        self.validate_config(&config).await?;

        // Start transaction
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| Error::Database(format!("Failed to start transaction: {}", e)))?;

        // Insert or update config with version increment
        let result = sqlx::query(
            r#"
            INSERT INTO tenant_configs (tenant_id, config, version)
            VALUES ($1, $2, 1)
            ON CONFLICT (tenant_id) DO UPDATE
            SET config = $2,
                version = tenant_configs.version + 1,
                updated_at = NOW()
            RETURNING version
            "#,
        )
        .bind(tenant_id.as_uuid())
        .bind(&config)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| Error::Database(format!("Failed to update config: {}", e)))?;

        let version: i32 = result
            .try_get("version")
            .map_err(|e| Error::Database(format!("Failed to get version: {}", e)))?;

        // Record in history
        sqlx::query(
            r#"
            INSERT INTO tenant_config_history (tenant_id, config, version)
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(tenant_id.as_uuid())
        .bind(&config)
        .bind(version)
        .execute(&mut *tx)
        .await
        .map_err(|e| Error::Database(format!("Failed to record config history: {}", e)))?;

        // Commit transaction
        tx.commit()
            .await
            .map_err(|e| Error::Database(format!("Failed to commit transaction: {}", e)))?;

        // Notify listeners of config change
        sqlx::query(&format!(
            "NOTIFY config_changes, '{}'",
            serde_json::json!({
                "tenant_id": tenant_id.to_string(),
                "version": version,
                "timestamp": chrono::Utc::now().to_rfc3339()
            })
        ))
        .execute(&*self.pool)
        .await
        .ok(); // Ignore notify errors

        Ok(())
    }

    async fn watch_changes(&self, tenant_id: Option<TenantId>) -> Result<ConfigChangeStream<'_>> {
        // Multi-tenant mode only - require tenant_id
        let tenant_id = tenant_id.ok_or_else(|| {
            Error::TenantRequired(
                "PostgresConfigStore requires a tenant_id (multi-tenant mode only)".to_string(),
            )
        })?;

        // Create a listener for PostgreSQL NOTIFY
        let mut listener = PgListener::connect_with(&self.pool)
            .await
            .map_err(|e| Error::Database(format!("Failed to create listener: {}", e)))?;

        listener
            .listen("config_changes")
            .await
            .map_err(|e| Error::Database(format!("Failed to listen to channel: {}", e)))?;

        // Convert listener into a stream
        let stream = listener.into_stream().filter_map(move |notification| {
            async move {
                match notification {
                    Ok(notif) => {
                        // Parse notification payload
                        if let Ok(payload) =
                            serde_json::from_str::<serde_json::Value>(notif.payload())
                        {
                            // Filter by tenant_id and check if it matches
                            if let Some(notif_tenant_id) =
                                payload.get("tenant_id").and_then(|v| v.as_str())
                                && notif_tenant_id == tenant_id.to_string()
                            {
                                // Extract version and timestamp
                                let version =
                                    payload.get("version").and_then(|v| v.as_u64()).unwrap_or(0)
                                        as u32;
                                let timestamp = payload
                                    .get("timestamp")
                                    .and_then(|v| v.as_str())
                                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                                    .map(|dt| dt.with_timezone(&chrono::Utc))
                                    .unwrap_or_else(chrono::Utc::now);

                                return Some(Ok(ConfigChange {
                                    tenant_id: Some(tenant_id),
                                    timestamp,
                                    version,
                                }));
                            }
                        }
                        None
                    }
                    Err(e) => Some(Err(Error::Database(format!("Listener error: {}", e)))),
                }
            }
        });

        Ok(Box::pin(stream))
    }

    async fn validate_config(&self, config: &serde_json::Value) -> Result<()> {
        // Basic validation: ensure it's an object
        if !config.is_object() {
            return Err(Error::ConfigValidation(
                "Configuration must be a JSON object".to_string(),
            ));
        }

        // Additional validation could be added here:
        // - Schema validation using jsonschema crate
        // - Required field checks
        // - Value range checks
        // For now, we just ensure it's an object

        Ok(())
    }

    async fn list_tenants(&self) -> Result<Vec<TenantId>> {
        let rows = sqlx::query("SELECT tenant_id FROM tenant_configs ORDER BY created_at ASC")
            .fetch_all(&*self.pool)
            .await
            .map_err(|e| Error::Database(format!("Failed to list tenants: {}", e)))?;

        let tenant_ids = rows
            .into_iter()
            .filter_map(|row| {
                row.try_get::<uuid::Uuid, _>("tenant_id")
                    .ok()
                    .map(TenantId::from_uuid)
            })
            .collect();

        Ok(tenant_ids)
    }

    async fn delete_config(&self, tenant_id: TenantId) -> Result<()> {
        let result = sqlx::query("DELETE FROM tenant_configs WHERE tenant_id = $1")
            .bind(tenant_id.as_uuid())
            .execute(&*self.pool)
            .await
            .map_err(|e| Error::Database(format!("Failed to delete config: {}", e)))?;

        if result.rows_affected() == 0 {
            return Err(Error::TenantNotFound(format!(
                "Tenant not found: {}",
                tenant_id
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn create_test_store() -> Result<PostgresConfigStore> {
        // Use a test database URL from environment or default
        let database_url = std::env::var("TEST_DATABASE_URL").unwrap_or_else(|_| {
            "postgres://postgres:postgres@localhost:5432/lunaroute_test".to_string()
        });

        PostgresConfigStore::new(&database_url).await
    }

    #[tokio::test]
    #[ignore] // Requires PostgreSQL instance
    async fn test_create_store() {
        let store = create_test_store().await;
        assert!(store.is_ok());
    }

    #[tokio::test]
    #[ignore] // Requires PostgreSQL instance
    async fn test_get_config_not_found() {
        let store = create_test_store().await.unwrap();
        let tenant_id = TenantId::new();

        let result = store.get_config(Some(tenant_id)).await;
        assert!(matches!(result, Err(Error::ConfigNotFound)));
    }

    #[tokio::test]
    #[ignore] // Requires PostgreSQL instance
    async fn test_update_and_get_config() {
        let store = create_test_store().await.unwrap();
        let tenant_id = TenantId::new();

        let config = serde_json::json!({
            "listeners": [{
                "name": "test-listener",
                "bind": "0.0.0.0:8081"
            }]
        });

        // Update config
        store
            .update_config(Some(tenant_id), config.clone())
            .await
            .unwrap();

        // Get config
        let retrieved = store.get_config(Some(tenant_id)).await.unwrap();
        assert_eq!(retrieved, config);

        // Cleanup
        store.delete_config(tenant_id).await.unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires PostgreSQL instance
    async fn test_list_tenants() {
        let store = create_test_store().await.unwrap();
        let tenant_id1 = TenantId::new();
        let tenant_id2 = TenantId::new();

        let config1 = serde_json::json!({"name": "tenant1"});
        let config2 = serde_json::json!({"name": "tenant2"});

        // Create two tenants
        store
            .update_config(Some(tenant_id1), config1)
            .await
            .unwrap();
        store
            .update_config(Some(tenant_id2), config2)
            .await
            .unwrap();

        // List tenants
        let tenants = store.list_tenants().await.unwrap();
        assert!(tenants.len() >= 2);
        assert!(tenants.contains(&tenant_id1));
        assert!(tenants.contains(&tenant_id2));

        // Cleanup
        store.delete_config(tenant_id1).await.unwrap();
        store.delete_config(tenant_id2).await.unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires PostgreSQL instance
    async fn test_delete_config() {
        let store = create_test_store().await.unwrap();
        let tenant_id = TenantId::new();

        let config = serde_json::json!({"name": "test"});

        // Create config
        store.update_config(Some(tenant_id), config).await.unwrap();

        // Delete config
        store.delete_config(tenant_id).await.unwrap();

        // Verify it's gone
        let result = store.get_config(Some(tenant_id)).await;
        assert!(matches!(result, Err(Error::ConfigNotFound)));
    }

    #[tokio::test]
    #[ignore] // Requires PostgreSQL instance
    async fn test_validate_config() {
        let store = create_test_store().await.unwrap();

        // Valid config (object)
        let valid_config = serde_json::json!({"name": "test"});
        assert!(store.validate_config(&valid_config).await.is_ok());

        // Invalid config (not an object)
        let invalid_config = serde_json::json!("string");
        assert!(matches!(
            store.validate_config(&invalid_config).await,
            Err(Error::ConfigValidation(_))
        ));
    }

    #[tokio::test]
    #[ignore] // Requires PostgreSQL instance
    async fn test_config_versioning() {
        let store = create_test_store().await.unwrap();
        let tenant_id = TenantId::new();

        let config1 = serde_json::json!({"version": 1});
        let config2 = serde_json::json!({"version": 2});

        // First update
        store.update_config(Some(tenant_id), config1).await.unwrap();

        // Second update (should increment version)
        store
            .update_config(Some(tenant_id), config2.clone())
            .await
            .unwrap();

        // Verify latest config
        let retrieved = store.get_config(Some(tenant_id)).await.unwrap();
        assert_eq!(retrieved, config2);

        // Cleanup
        store.delete_config(tenant_id).await.unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires PostgreSQL instance
    async fn test_requires_tenant_id() {
        // This test doesn't require PostgreSQL
        let store = create_test_store().await.unwrap();

        // get_config without tenant_id should fail
        let result = store.get_config(None).await;
        assert!(matches!(result, Err(Error::TenantRequired(_))));

        // update_config without tenant_id should fail
        let config = serde_json::json!({"name": "test"});
        let result = store.update_config(None, config).await;
        assert!(matches!(result, Err(Error::TenantRequired(_))));
    }
}
