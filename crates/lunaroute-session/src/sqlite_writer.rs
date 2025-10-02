//! SQLite session writer implementation

#[cfg(feature = "sqlite-writer")]
use crate::events::SessionEvent;
#[cfg(feature = "sqlite-writer")]
use crate::writer::{SessionWriter, WriterError, WriterResult};
#[cfg(feature = "sqlite-writer")]
use async_trait::async_trait;
#[cfg(feature = "sqlite-writer")]
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous};
#[cfg(feature = "sqlite-writer")]
use std::path::Path;

#[cfg(feature = "sqlite-writer")]
pub struct SqliteWriter {
    pool: SqlitePool,
}

#[cfg(feature = "sqlite-writer")]
impl SqliteWriter {
    pub async fn new(db_path: &Path) -> WriterResult<Self> {
        // Create directory if needed
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(WriterError::Io)?;
        }

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(db_path)
                    .create_if_missing(true)
                    .journal_mode(SqliteJournalMode::Wal)
                    .synchronous(SqliteSynchronous::Normal),
            )
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        // Initialize schema
        Self::initialize_schema(&pool).await?;

        // Verify schema version
        let version: i32 = sqlx::query_scalar("SELECT version FROM schema_version")
            .fetch_one(&pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        if version != 1 {
            return Err(WriterError::Database(format!(
                "Unsupported schema version: {}",
                version
            )));
        }

        Ok(Self { pool })
    }

    async fn initialize_schema(pool: &SqlitePool) -> WriterResult<()> {
        // Schema version table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY
            )
            "#,
        )
        .execute(pool)
        .await
        .map_err(|e| WriterError::Database(e.to_string()))?;

        sqlx::query("INSERT OR IGNORE INTO schema_version (version) VALUES (1)")
            .execute(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        // Sessions table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                session_id TEXT PRIMARY KEY,
                request_id TEXT,
                started_at TIMESTAMP NOT NULL,
                completed_at TIMESTAMP,
                provider TEXT NOT NULL,
                listener TEXT NOT NULL,
                model_requested TEXT NOT NULL,
                model_used TEXT,
                success BOOLEAN,
                error_message TEXT,
                finish_reason TEXT,
                total_duration_ms INTEGER,
                provider_latency_ms INTEGER,
                input_tokens INTEGER DEFAULT 0,
                output_tokens INTEGER DEFAULT 0,
                thinking_tokens INTEGER DEFAULT 0,
                total_tokens INTEGER GENERATED ALWAYS AS (
                    input_tokens + output_tokens + COALESCE(thinking_tokens, 0)
                ) STORED,
                request_text TEXT,
                response_text TEXT,
                client_ip TEXT,
                user_agent TEXT,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(pool)
        .await
        .map_err(|e| WriterError::Database(e.to_string()))?;

        // Indexes
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_created ON sessions(created_at DESC)")
            .execute(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_provider ON sessions(provider, created_at DESC)")
            .execute(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_model ON sessions(model_used, created_at DESC)")
            .execute(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_request_id ON sessions(request_id)")
            .execute(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        // Composite index for filtering by provider and model together
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_provider_model ON sessions(provider, model_used, created_at DESC)")
            .execute(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        // Session stats table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS session_stats (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                request_id TEXT,
                model_name TEXT NOT NULL,
                pre_processing_ms REAL,
                post_processing_ms REAL,
                proxy_overhead_ms REAL,
                input_tokens INTEGER DEFAULT 0,
                output_tokens INTEGER DEFAULT 0,
                thinking_tokens INTEGER DEFAULT 0,
                cache_read_tokens INTEGER DEFAULT 0,
                cache_write_tokens INTEGER DEFAULT 0,
                tokens_per_second REAL,
                thinking_percentage REAL,
                request_size_bytes INTEGER,
                response_size_bytes INTEGER,
                message_count INTEGER,
                content_blocks INTEGER,
                has_tools BOOLEAN DEFAULT 0,
                has_refusal BOOLEAN DEFAULT 0,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
            )
            "#,
        )
        .execute(pool)
        .await
        .map_err(|e| WriterError::Database(e.to_string()))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_session_stats_session ON session_stats(session_id)")
            .execute(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_session_stats_model ON session_stats(model_name, created_at DESC)")
            .execute(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        // Index for time-series queries per session
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_session_stats_session_time ON session_stats(session_id, created_at DESC)")
            .execute(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        // Tool calls table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS tool_calls (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                request_id TEXT,
                model_name TEXT,
                tool_name TEXT NOT NULL,
                call_count INTEGER DEFAULT 1,
                avg_execution_time_ms INTEGER,
                error_count INTEGER DEFAULT 0,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
            )
            "#,
        )
        .execute(pool)
        .await
        .map_err(|e| WriterError::Database(e.to_string()))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_tool_calls_model ON tool_calls(model_name, created_at DESC)")
            .execute(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        // Index for looking up tool calls by session
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_tool_calls_session ON tool_calls(session_id)")
            .execute(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        // Index for tool usage analysis
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_tool_calls_name ON tool_calls(tool_name, created_at DESC)")
            .execute(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        Ok(())
    }
}

#[cfg(feature = "sqlite-writer")]
#[async_trait]
impl SessionWriter for SqliteWriter {
    async fn write_event(&self, event: &SessionEvent) -> WriterResult<()> {
        self.write_batch(std::slice::from_ref(event)).await
    }

    async fn write_batch(&self, events: &[SessionEvent]) -> WriterResult<()> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        for event in events {
            match event {
                SessionEvent::Started {
                    session_id,
                    request_id,
                    timestamp,
                    model_requested,
                    provider,
                    listener,
                    metadata,
                } => {
                    sqlx::query(
                        r#"
                        INSERT INTO sessions (session_id, request_id, started_at, model_requested, provider, listener, client_ip, user_agent)
                        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                        ON CONFLICT(session_id) DO NOTHING
                        "#,
                    )
                    .bind(session_id)
                    .bind(request_id)
                    .bind(timestamp)
                    .bind(model_requested)
                    .bind(provider)
                    .bind(listener)
                    .bind(&metadata.client_ip)
                    .bind(&metadata.user_agent)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| WriterError::Database(e.to_string()))?;
                }

                SessionEvent::RequestRecorded {
                    session_id,
                    request_text,
                    ..
                } => {
                    sqlx::query(
                        r#"
                        UPDATE sessions
                        SET request_text = ?
                        WHERE session_id = ?
                        "#,
                    )
                    .bind(request_text)
                    .bind(session_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| WriterError::Database(e.to_string()))?;

                    // Insert request stats
                    // Note: We don't have model_name here, will be updated on response
                }

                SessionEvent::ResponseRecorded {
                    session_id,
                    request_id,
                    response_text,
                    model_used,
                    stats,
                    ..
                } => {
                    sqlx::query(
                        r#"
                        UPDATE sessions
                        SET response_text = ?,
                            model_used = ?,
                            output_tokens = ?,
                            thinking_tokens = ?,
                            input_tokens = ?,
                            provider_latency_ms = ?
                        WHERE session_id = ?
                        "#,
                    )
                    .bind(response_text)
                    .bind(model_used)
                    .bind(stats.tokens.output_tokens as i64)
                    .bind(stats.tokens.thinking_tokens.map(|t| t as i64))
                    .bind(stats.tokens.input_tokens as i64)
                    .bind(stats.provider_latency_ms as i64)
                    .bind(session_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| WriterError::Database(e.to_string()))?;

                    // Insert session stats
                    sqlx::query(
                        r#"
                        INSERT INTO session_stats (
                            session_id, request_id, model_name,
                            post_processing_ms, proxy_overhead_ms,
                            input_tokens, output_tokens, thinking_tokens,
                            cache_read_tokens, cache_write_tokens,
                            tokens_per_second, thinking_percentage,
                            response_size_bytes, content_blocks, has_refusal
                        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                        "#,
                    )
                    .bind(session_id)
                    .bind(request_id)
                    .bind(model_used)
                    .bind(stats.post_processing_ms)
                    .bind(stats.total_proxy_overhead_ms)
                    .bind(stats.tokens.input_tokens as i64)
                    .bind(stats.tokens.output_tokens as i64)
                    .bind(stats.tokens.thinking_tokens.map(|t| t as i64))
                    .bind(stats.tokens.cache_read_tokens.map(|t| t as i64))
                    .bind(stats.tokens.cache_write_tokens.map(|t| t as i64))
                    .bind(stats.tokens.tokens_per_second.map(|t| t as f64))
                    .bind(stats.tokens.thinking_percentage.map(|t| t as f64))
                    .bind(stats.response_size_bytes as i64)
                    .bind(stats.content_blocks as i64)
                    .bind(stats.has_refusal)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| WriterError::Database(e.to_string()))?;

                    // Insert tool calls
                    for tool in &stats.tool_calls {
                        sqlx::query(
                            r#"
                            INSERT INTO tool_calls (session_id, request_id, model_name, tool_name, call_count, avg_execution_time_ms)
                            VALUES (?, ?, ?, ?, 1, ?)
                            "#,
                        )
                        .bind(session_id)
                        .bind(request_id)
                        .bind(model_used)
                        .bind(&tool.tool_name)
                        .bind(tool.execution_time_ms.map(|t| t as i64))
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| WriterError::Database(e.to_string()))?;
                    }
                }

                SessionEvent::Completed {
                    session_id,
                    success,
                    error,
                    finish_reason,
                    final_stats,
                    ..
                } => {
                    sqlx::query(
                        r#"
                        UPDATE sessions
                        SET completed_at = CURRENT_TIMESTAMP,
                            success = ?,
                            error_message = ?,
                            finish_reason = ?,
                            total_duration_ms = ?
                        WHERE session_id = ?
                        "#,
                    )
                    .bind(success)
                    .bind(error)
                    .bind(finish_reason)
                    .bind(final_stats.total_duration_ms as i64)
                    .bind(session_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| WriterError::Database(e.to_string()))?;
                }

                _ => {
                    // Other event types not stored in SQL
                }
            }
        }

        tx.commit()
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        Ok(())
    }

    fn supports_batching(&self) -> bool {
        true
    }
}

#[cfg(test)]
#[cfg(feature = "sqlite-writer")]
mod tests {
    use super::*;
    use crate::events::*;
    use chrono::Utc;
    use std::collections::HashMap;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_sqlite_writer_schema_creation() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let writer = SqliteWriter::new(&db_path).await.unwrap();

        // Verify schema version
        let version: i32 = sqlx::query_scalar("SELECT version FROM schema_version")
            .fetch_one(&writer.pool)
            .await
            .unwrap();

        assert_eq!(version, 1);
    }

    #[tokio::test]
    async fn test_sqlite_writer_session_flow() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let writer = SqliteWriter::new(&db_path).await.unwrap();

        let events = vec![
            SessionEvent::Started {
                session_id: "test-123".to_string(),
                request_id: "req-456".to_string(),
                timestamp: Utc::now(),
                model_requested: "gpt-4".to_string(),
                provider: "openai".to_string(),
                listener: "openai".to_string(),
                metadata: SessionMetadata {
                    client_ip: Some("127.0.0.1".to_string()),
                    user_agent: Some("test".to_string()),
                    api_version: Some("v1".to_string()),
                    request_headers: HashMap::new(),
                    session_tags: vec![],
                },
            },
            SessionEvent::ResponseRecorded {
                session_id: "test-123".to_string(),
                request_id: "req-456".to_string(),
                timestamp: Utc::now(),
                response_text: "Hello".to_string(),
                response_json: serde_json::json!({}),
                model_used: "gpt-4".to_string(),
                stats: ResponseStats {
                    provider_latency_ms: 100,
                    post_processing_ms: 10.0,
                    total_proxy_overhead_ms: 15.0,
                    tokens: TokenStats {
                        input_tokens: 10,
                        output_tokens: 20,
                        thinking_tokens: None,
                        cache_read_tokens: None,
                        cache_write_tokens: None,
                        total_tokens: 30,
                        thinking_percentage: None,
                        tokens_per_second: Some(200.0),
                    },
                    tool_calls: vec![],
                    response_size_bytes: 100,
                    content_blocks: 1,
                    has_refusal: false,
                },
            },
        ];

        writer.write_batch(&events).await.unwrap();

        // Verify session was created
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE session_id = 'test-123'")
            .fetch_one(&writer.pool)
            .await
            .unwrap();

        assert_eq!(count, 1);

        // Verify stats were recorded
        let stats_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM session_stats WHERE session_id = 'test-123'")
            .fetch_one(&writer.pool)
            .await
            .unwrap();

        assert_eq!(stats_count, 1);
    }
}
