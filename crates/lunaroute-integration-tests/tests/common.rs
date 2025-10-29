//! Common test utilities for integration tests

use async_trait::async_trait;
use lunaroute_core::{error::Error as CoreError, session_store::SessionStore, tenant::TenantId};
use lunaroute_session::SessionEvent;
use std::sync::{Arc, Mutex};

/// In-memory session store for testing
#[derive(Clone, Default)]
#[allow(dead_code)]
pub struct InMemorySessionStore {
    events: Arc<Mutex<Vec<serde_json::Value>>>,
}

#[allow(dead_code)]
impl InMemorySessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get all recorded events as SessionEvent enum
    pub fn get_events(&self) -> Vec<SessionEvent> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|v| serde_json::from_value(v.clone()).ok())
            .collect()
    }
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn write_event(
        &self,
        _tenant_id: Option<TenantId>,
        event: serde_json::Value,
    ) -> Result<(), CoreError> {
        self.events.lock().unwrap().push(event);
        Ok(())
    }

    async fn search(
        &self,
        _tenant_id: Option<TenantId>,
        _query: serde_json::Value,
    ) -> Result<serde_json::Value, CoreError> {
        // Not implemented for testing
        Ok(serde_json::json!({"sessions": []}))
    }

    async fn get_session(
        &self,
        _tenant_id: Option<TenantId>,
        _session_id: &str,
    ) -> Result<serde_json::Value, CoreError> {
        // Not implemented for testing
        Ok(serde_json::json!(null))
    }

    async fn cleanup(
        &self,
        _tenant_id: Option<TenantId>,
        _params: serde_json::Value,
    ) -> Result<serde_json::Value, CoreError> {
        // Not implemented for testing
        Ok(serde_json::json!({"deleted": 0}))
    }

    async fn get_stats(
        &self,
        _tenant_id: Option<TenantId>,
        _params: serde_json::Value,
    ) -> Result<serde_json::Value, CoreError> {
        // Not implemented for testing
        Ok(serde_json::json!({"total_sessions": 0}))
    }
}
