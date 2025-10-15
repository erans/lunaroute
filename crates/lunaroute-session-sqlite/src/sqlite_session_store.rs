//! SqliteSessionStore - SessionStore trait implementation for SQLite + JSONL storage

use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;

use lunaroute_core::{
    session_store::{SessionStore, Session, SearchQuery, SearchResults, RetentionPolicy, CleanupStats, TimeRange, AggregateStats},
    tenant::TenantId,
    Error, Result,
};

use lunaroute_session::{
    events::SessionEvent,  // Use the SessionEvent from lunaroute-session
    sqlite_writer::SqliteWriter,
    jsonl_writer::JsonlWriter,
    writer::{SessionWriter, MultiWriterRecorder, RecorderConfig},
};

/// SQLite + JSONL session store for single-tenant mode
///
/// Combines SQLite for fast queries with JSONL files for complete event history.
#[derive(Clone)]
pub struct SqliteSessionStore {
    /// Multi-writer recorder that writes to both SQLite and JSONL
    recorder: Arc<MultiWriterRecorder>,
    /// SQLite writer for queries
    sqlite: Arc<SqliteWriter>,
    /// JSONL writer for file access
    jsonl: Arc<JsonlWriter>,
}

impl SqliteSessionStore {
    /// Create a new SQLite + JSONL session store
    ///
    /// # Arguments
    /// * `db_path` - Path to SQLite database file
    /// * `jsonl_dir` - Directory for JSONL files (organized by date)
    ///
    /// # Errors
    /// - `Error::Database` if SQLite connection fails
    /// - `Error::Io` if JSONL directory creation fails
    pub async fn new(db_path: impl Into<PathBuf>, jsonl_dir: impl Into<PathBuf>) -> Result<Self> {
        let db_path = db_path.into();
        let jsonl_dir = jsonl_dir.into();

        // Expand tilde in paths
        let db_path = expand_tilde(db_path)?;
        let jsonl_dir = expand_tilde(jsonl_dir)?;

        // Create writers
        let sqlite = Arc::new(SqliteWriter::new(&db_path).await.map_err(|e| {
            Error::Database(format!("Failed to create SQLite writer: {}", e))
        })?);

        let jsonl = Arc::new(JsonlWriter::new(jsonl_dir));
        // Create multi-writer recorder with both writers
        let recorder = Arc::new(MultiWriterRecorder::with_config(
            vec![
                sqlite.clone() as Arc<dyn SessionWriter>,
                jsonl.clone() as Arc<dyn SessionWriter>,
            ],
            RecorderConfig {
                batch_size: 100,
                batch_timeout_ms: 100,
                channel_buffer_size: 10_000,
            },
        ));

        Ok(Self {
            recorder,
            sqlite,
            jsonl,
        })
    }
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn write_event(
        &self,
        tenant_id: Option<TenantId>,
        event: serde_json::Value,  // SessionEvent is a placeholder type in core
    ) -> Result<()> {
        // Single-tenant mode only
        if tenant_id.is_some() {
            return Err(Error::TenantRequired(
                "SQLite session store does not support multi-tenancy".to_string(),
            ));
        }

        // Convert JSON to SessionEvent
        let event: SessionEvent = serde_json::from_value(event)
            .map_err(|e| Error::SessionStore(format!("Failed to deserialize event: {}", e)))?;

        // Record event (non-blocking)
        self.recorder.record_event(event);
        Ok(())
    }

    async fn search(
        &self,
        tenant_id: Option<TenantId>,
        _query: SearchQuery,
    ) -> Result<SearchResults> {
        // Single-tenant mode only
        if tenant_id.is_some() {
            return Err(Error::TenantRequired(
                "SQLite session store does not support multi-tenancy".to_string(),
            ));
        }

        // TODO: Implement search using SQLite queries
        todo!("Implement search")
    }

    async fn get_session(&self, tenant_id: Option<TenantId>, _session_id: &str) -> Result<Session> {
        // Single-tenant mode only
        if tenant_id.is_some() {
            return Err(Error::TenantRequired(
                "SQLite session store does not support multi-tenancy".to_string(),
            ));
        }

        // TODO: Implement get_session
        todo!("Implement get_session")
    }

    async fn cleanup(
        &self,
        tenant_id: Option<TenantId>,
        _retention: RetentionPolicy,
    ) -> Result<CleanupStats> {
        // Single-tenant mode only
        if tenant_id.is_some() {
            return Err(Error::TenantRequired(
                "SQLite session store does not support multi-tenancy".to_string(),
            ));
        }

        // TODO: Implement cleanup
        todo!("Implement cleanup")
    }

    async fn get_stats(
        &self,
        tenant_id: Option<TenantId>,
        _time_range: TimeRange,
    ) -> Result<AggregateStats> {
        // Single-tenant mode only
        if tenant_id.is_some() {
            return Err(Error::TenantRequired(
                "SQLite session store does not support multi-tenancy".to_string(),
            ));
        }

        // TODO: Implement get_stats
        todo!("Implement get_stats")
    }

    async fn flush(&self) -> Result<()> {
        // MultiWriterRecorder uses shutdown instead of flush
        // For now, we'll just return Ok since events are batched automatically
        // In production, you might want to call shutdown on drop or explicitly
        Ok(())
    }

    async fn list_sessions(
        &self,
        tenant_id: Option<TenantId>,
        _limit: usize,
        _offset: usize,
    ) -> Result<Vec<Session>> {
        // Single-tenant mode only
        if tenant_id.is_some() {
            return Err(Error::TenantRequired(
                "SQLite session store does not support multi-tenancy".to_string(),
            ));
        }

        // TODO: Implement list_sessions
        todo!("Implement list_sessions")
    }
}

/// Expand tilde (~) in path
fn expand_tilde(path: PathBuf) -> Result<PathBuf> {
    if path.starts_with("~") {
        let home = dirs::home_dir()
            .ok_or_else(|| Error::Internal("Could not determine home directory".to_string()))?;
        Ok(home.join(path.strip_prefix("~").unwrap()))
    } else {
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_create_store() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let jsonl_dir = temp_dir.path().join("sessions");

        let store = SqliteSessionStore::new(db_path, jsonl_dir).await;
        assert!(store.is_ok());
    }

    #[tokio::test]
    async fn test_multi_tenant_rejected() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let jsonl_dir = temp_dir.path().join("sessions");

        let store = SqliteSessionStore::new(db_path, jsonl_dir).await.unwrap();
        let tenant_id = Some(TenantId::new());

        // write_event with tenant should fail
        let event = serde_json::json!({"type": "test"});
        assert!(store.write_event(tenant_id, event).await.is_err());
    }
}
