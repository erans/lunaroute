//! Tenant-scoped session store decorator.
//!
//! Wraps a `SessionStore` and resolves an implicit process-wide default tenant
//! for calls that pass `None`. Used for recording, where the caller is the proxy
//! itself (no per-request tenant resolution yet).
//!
//! Resolution rule: `resolved = tenant_id.or(self.default_tenant)`.
//!
//! - When `default_tenant == Some(t)` (Postgres mode): `None` → `Some(t)`,
//!   and an explicit `Some(u)` overrides the default.
//! - When `default_tenant == None` (SQLite/file mode): `None` → `None`
//!   (unchanged behavior — SQLite requires `None`).

use std::sync::Arc;

use async_trait::async_trait;
use lunaroute_core::{
    Result,
    session_store::{
        AggregateStats, CleanupStats, RetentionPolicy, SearchQuery, SearchResults, Session,
        SessionEvent, SessionStore, TimeRange,
    },
    tenant::TenantId,
};

/// Wraps a `SessionStore` and resolves an implicit default tenant.
///
/// See module docs for the resolution rule and the "bridge" rationale.
pub struct TenantScopedStore {
    inner: Arc<dyn SessionStore>,
    default_tenant: Option<TenantId>,
}

impl TenantScopedStore {
    pub fn new(inner: Arc<dyn SessionStore>, default_tenant: Option<TenantId>) -> Self {
        Self {
            inner,
            default_tenant,
        }
    }
}

#[async_trait]
impl SessionStore for TenantScopedStore {
    async fn write_event(&self, tenant_id: Option<TenantId>, event: SessionEvent) -> Result<()> {
        let tid = tenant_id.or(self.default_tenant);
        self.inner.write_event(tid, event).await
    }

    async fn search(
        &self,
        tenant_id: Option<TenantId>,
        query: SearchQuery,
    ) -> Result<SearchResults> {
        let tid = tenant_id.or(self.default_tenant);
        self.inner.search(tid, query).await
    }

    async fn get_session(&self, tenant_id: Option<TenantId>, session_id: &str) -> Result<Session> {
        let tid = tenant_id.or(self.default_tenant);
        self.inner.get_session(tid, session_id).await
    }

    async fn cleanup(
        &self,
        tenant_id: Option<TenantId>,
        retention: RetentionPolicy,
    ) -> Result<CleanupStats> {
        let tid = tenant_id.or(self.default_tenant);
        self.inner.cleanup(tid, retention).await
    }

    async fn get_stats(
        &self,
        tenant_id: Option<TenantId>,
        time_range: TimeRange,
    ) -> Result<AggregateStats> {
        let tid = tenant_id.or(self.default_tenant);
        self.inner.get_stats(tid, time_range).await
    }

    async fn flush(&self) -> Result<()> {
        self.inner.flush().await
    }

    async fn list_sessions(
        &self,
        tenant_id: Option<TenantId>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Session>> {
        let tid = tenant_id.or(self.default_tenant);
        self.inner.list_sessions(tid, limit, offset).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunaroute_core::tenant::TenantId;
    use std::sync::Mutex;
    use uuid::Uuid;

    /// Mock store that records every `tenant_id` it received for `write_event`.
    struct CapturingStore {
        seen: Mutex<Vec<Option<TenantId>>>,
        flush_called: Mutex<bool>,
    }

    impl CapturingStore {
        fn new() -> Self {
            Self {
                seen: Mutex::new(Vec::new()),
                flush_called: Mutex::new(false),
            }
        }
        fn seen(&self) -> Vec<Option<TenantId>> {
            self.seen.lock().unwrap().clone()
        }
        fn was_flushed(&self) -> bool {
            *self.flush_called.lock().unwrap()
        }
    }

    #[async_trait]
    impl SessionStore for CapturingStore {
        async fn write_event(
            &self,
            tenant_id: Option<TenantId>,
            _event: SessionEvent,
        ) -> Result<()> {
            self.seen.lock().unwrap().push(tenant_id);
            Ok(())
        }
        async fn search(&self, _t: Option<TenantId>, _q: SearchQuery) -> Result<SearchResults> {
            Ok(serde_json::json!({"sessions": []}))
        }
        async fn get_session(&self, _t: Option<TenantId>, _id: &str) -> Result<Session> {
            Ok(serde_json::json!(null))
        }
        async fn cleanup(&self, _t: Option<TenantId>, _r: RetentionPolicy) -> Result<CleanupStats> {
            Ok(serde_json::json!({"deleted": 0}))
        }
        async fn get_stats(&self, _t: Option<TenantId>, _tr: TimeRange) -> Result<AggregateStats> {
            Ok(serde_json::json!({}))
        }
        async fn flush(&self) -> Result<()> {
            *self.flush_called.lock().unwrap() = true;
            Ok(())
        }
        async fn list_sessions(
            &self,
            _t: Option<TenantId>,
            _l: usize,
            _o: usize,
        ) -> Result<Vec<Session>> {
            Ok(Vec::new())
        }
    }

    fn tid() -> TenantId {
        TenantId::from_uuid(Uuid::new_v4())
    }

    #[tokio::test]
    async fn none_resolves_to_default_when_default_set() {
        let inner = Arc::new(CapturingStore::new());
        let default = tid();
        let scoped = TenantScopedStore::new(inner.clone(), Some(default));
        scoped
            .write_event(None, serde_json::json!({"type": "Started"}))
            .await
            .unwrap();
        assert_eq!(inner.seen(), vec![Some(default)]);
    }

    #[tokio::test]
    async fn explicit_tenant_overrides_default() {
        let inner = Arc::new(CapturingStore::new());
        let default = tid();
        let request_tenant = tid();
        let scoped = TenantScopedStore::new(inner.clone(), Some(default));
        scoped
            .write_event(Some(request_tenant), serde_json::json!({"type": "Started"}))
            .await
            .unwrap();
        assert_eq!(inner.seen(), vec![Some(request_tenant)]);
    }

    #[tokio::test]
    async fn none_passes_through_when_no_default() {
        let inner = Arc::new(CapturingStore::new());
        let scoped = TenantScopedStore::new(inner.clone(), None);
        scoped
            .write_event(None, serde_json::json!({"type": "Started"}))
            .await
            .unwrap();
        assert_eq!(inner.seen(), vec![None]);
    }

    #[tokio::test]
    async fn flush_delegates_to_inner_store() {
        let inner = Arc::new(CapturingStore::new());
        let scoped = TenantScopedStore::new(inner.clone(), None);
        scoped.flush().await.unwrap();
        assert!(
            inner.was_flushed(),
            "scoped flush must delegate to inner store"
        );
    }
}
