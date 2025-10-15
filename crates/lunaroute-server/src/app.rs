//! Core application state with dependency-injected stores
//!
//! This module contains the `AppState` struct that holds ConfigStore and
//! SessionStore trait implementations. This enables dependency injection,
//! allowing the same business logic to work with both single-tenant
//! (file-based config, SQLite sessions) and multi-tenant (PostgreSQL config,
//! PostgreSQL sessions) deployments.

use std::sync::Arc;

use lunaroute_core::{config_store::ConfigStore, session_store::SessionStore, tenant::TenantId};

/// Application state with dependency-injected stores
///
/// Holds the configuration and session stores, and provides access to them
/// throughout the application via Axum's state management.
///
/// # Example
/// ```no_run
/// # use std::sync::Arc;
/// # use lunaroute_core::{ConfigStore, SessionStore};
/// # use lunaroute_server::app::AppState;
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// # let config_store: Arc<dyn ConfigStore> = todo!();
/// # let session_store: Arc<dyn SessionStore> = todo!();
/// let state = AppState::new(config_store, session_store, None);
///
/// // Get configuration
/// let config = state.get_config().await?;
///
/// // Write session event
/// let event = serde_json::json!({"type": "test"});
/// state.write_event(event).await?;
/// # Ok(())
/// # }
/// ```
#[allow(dead_code)] // Infrastructure for future use
#[derive(Clone)]
pub struct AppState {
    /// Configuration store (file-based or database-backed)
    config_store: Arc<dyn ConfigStore>,

    /// Session store (SQLite or PostgreSQL)
    session_store: Arc<dyn SessionStore>,

    /// Tenant ID (None for single-tenant mode)
    tenant_id: Option<TenantId>,
}

#[allow(dead_code)] // Infrastructure methods for future use
impl AppState {
    /// Create a new application state
    ///
    /// # Arguments
    /// * `config_store` - Configuration store implementation
    /// * `session_store` - Session store implementation
    /// * `tenant_id` - Optional tenant ID (None for single-tenant mode)
    pub fn new(
        config_store: Arc<dyn ConfigStore>,
        session_store: Arc<dyn SessionStore>,
        tenant_id: Option<TenantId>,
    ) -> Self {
        Self {
            config_store,
            session_store,
            tenant_id,
        }
    }

    /// Get the configuration from the store
    pub async fn get_config(&self) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        Ok(self.config_store.get_config(self.tenant_id).await?)
    }

    /// Write a session event to the store
    pub async fn write_event(
        &self,
        event: serde_json::Value,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.session_store
            .write_event(self.tenant_id, event)
            .await?;
        Ok(())
    }

    /// Get references to the stores (for advanced usage)
    pub fn stores(&self) -> (&Arc<dyn ConfigStore>, &Arc<dyn SessionStore>) {
        (&self.config_store, &self.session_store)
    }

    /// Get the tenant ID
    pub fn tenant_id(&self) -> Option<TenantId> {
        self.tenant_id
    }

    /// Get the config store
    pub fn config_store(&self) -> &Arc<dyn ConfigStore> {
        &self.config_store
    }

    /// Get the session store
    pub fn session_store(&self) -> &Arc<dyn SessionStore> {
        &self.session_store
    }
}

#[cfg(test)]
mod tests {
    // Placeholder tests - full tests would require mock implementations
    #[test]
    fn test_app_state_tenant_id() {
        // Test will be implemented when we have mock stores
    }
}
