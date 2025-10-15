//! Session store trait for multi-tenancy support
//!
//! The `SessionStore` trait provides an abstraction over session data storage,
//! allowing different implementations for single-tenant (SQLite) and
//! multi-tenant (TimescaleDB/ClickHouse) deployments.

use async_trait::async_trait;

use crate::{tenant::TenantId, Result};

// Placeholder types - will be properly defined in types.rs
pub type SessionEvent = serde_json::Value;
pub type SearchQuery = serde_json::Value;
pub type SearchResults = serde_json::Value;
pub type Session = serde_json::Value;
pub type RetentionPolicy = serde_json::Value;
pub type CleanupStats = serde_json::Value;
pub type TimeRange = serde_json::Value;
pub type AggregateStats = serde_json::Value;

/// Session store trait
///
/// Implementations:
/// - `SqliteSessionStore`: SQLite + JSONL files (single-tenant)
/// - `TimescaleSessionStore`: PostgreSQL + TimescaleDB (multi-tenant)
/// - `ClickHouseSessionStore`: ClickHouse (multi-tenant, high scale)
///
/// # Example
/// ```no_run
/// # use lunaroute_core::session_store::SessionStore;
/// # use lunaroute_core::tenant::TenantId;
/// # async fn example(store: &dyn SessionStore, event: serde_json::Value) -> lunaroute_core::Result<()> {
/// // Single-tenant mode
/// store.write_event(None, event.clone()).await?;
///
/// // Multi-tenant mode
/// let tenant_id = TenantId::new();
/// store.write_event(Some(tenant_id), event).await?;
/// # Ok(())
/// # }
/// ```
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Write a session event
    ///
    /// Events are buffered and written in batches by the implementation.
    /// This method should return immediately without blocking.
    ///
    /// # Arguments
    /// * `tenant_id` - Optional tenant ID (None for single-tenant mode)
    /// * `event` - Session event to write
    ///
    /// # Errors
    /// - `Error::SessionStore` for write errors
    /// - `Error::TenantRequired` if tenant_id is None in multi-tenant mode
    async fn write_event(&self, tenant_id: Option<TenantId>, event: SessionEvent) -> Result<()>;

    /// Search sessions with filters
    ///
    /// # Arguments
    /// * `tenant_id` - Optional tenant ID (None for single-tenant mode)
    /// * `query` - Search query with filters
    ///
    /// # Returns
    /// Paginated search results with sessions matching the query.
    ///
    /// # Errors
    /// - `Error::SessionStore` for query errors
    /// - `Error::TenantRequired` if tenant_id is None in multi-tenant mode
    async fn search(
        &self,
        tenant_id: Option<TenantId>,
        query: SearchQuery,
    ) -> Result<SearchResults>;

    /// Get a single session by ID
    ///
    /// # Arguments
    /// * `tenant_id` - Optional tenant ID (None for single-tenant mode)
    /// * `session_id` - Session ID to retrieve
    ///
    /// # Errors
    /// - `Error::SessionNotFound` if session doesn't exist
    /// - `Error::TenantRequired` if tenant_id is None in multi-tenant mode
    /// - `Error::SessionStore` for read errors
    async fn get_session(&self, tenant_id: Option<TenantId>, session_id: &str)
        -> Result<Session>;

    /// Apply retention policies
    ///
    /// Deletes or archives sessions based on retention policy.
    /// Should be called periodically by a background task.
    ///
    /// # Arguments
    /// * `tenant_id` - Optional tenant ID (None for single-tenant mode)
    /// * `retention` - Retention policy to apply
    ///
    /// # Returns
    /// Statistics about the cleanup operation (sessions deleted, bytes freed, etc.)
    ///
    /// # Errors
    /// - `Error::SessionStore` for cleanup errors
    async fn cleanup(
        &self,
        tenant_id: Option<TenantId>,
        retention: RetentionPolicy,
    ) -> Result<CleanupStats>;

    /// Get aggregated statistics
    ///
    /// Returns aggregated metrics for dashboards and analytics.
    ///
    /// # Arguments
    /// * `tenant_id` - Optional tenant ID (None for single-tenant mode)
    /// * `time_range` - Time range for aggregation
    ///
    /// # Returns
    /// Aggregated statistics (total requests, tokens, latency percentiles, etc.)
    ///
    /// # Errors
    /// - `Error::SessionStore` for query errors
    /// - `Error::TenantRequired` if tenant_id is None in multi-tenant mode
    async fn get_stats(
        &self,
        tenant_id: Option<TenantId>,
        time_range: TimeRange,
    ) -> Result<AggregateStats>;

    /// Flush any buffered events
    ///
    /// Forces immediate write of all buffered events.
    /// Used during graceful shutdown.
    ///
    /// # Errors
    /// - `Error::SessionStore` for flush errors
    async fn flush(&self) -> Result<()> {
        // Default implementation: no-op
        Ok(())
    }

    /// List all sessions for a tenant (with pagination)
    ///
    /// # Arguments
    /// * `tenant_id` - Optional tenant ID (None for single-tenant mode)
    /// * `limit` - Maximum number of sessions to return
    /// * `offset` - Number of sessions to skip
    ///
    /// # Errors
    /// - `Error::SessionStore` for query errors
    async fn list_sessions(
        &self,
        tenant_id: Option<TenantId>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Session>> {
        // Default implementation returns empty list
        let _ = (tenant_id, limit, offset);
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_placeholder_types() {
        // These are just placeholders until we define proper types
        let event: SessionEvent = serde_json::json!({"type": "test"});
        assert!(event.is_object());
    }
}
