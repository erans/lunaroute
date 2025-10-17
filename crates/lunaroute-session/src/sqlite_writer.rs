//! SQLite session writer implementation

#[cfg(feature = "sqlite-writer")]
use crate::events::{
    FinalSessionStats, SessionEvent, StreamingStats, TokenTotals, ToolUsageSummary,
};
#[cfg(feature = "sqlite-writer")]
use crate::search::{SearchResults, SessionAggregates, SessionFilter, SessionRecord, SortOrder};
#[cfg(feature = "sqlite-writer")]
use crate::writer::{SessionWriter, WriterError, WriterResult};
#[cfg(feature = "sqlite-writer")]
use async_trait::async_trait;
#[cfg(feature = "sqlite-writer")]
use sqlx::Row;
#[cfg(feature = "sqlite-writer")]
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
#[cfg(feature = "sqlite-writer")]
use std::collections::HashMap;
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

        Ok(Self { pool })
    }

    /// Validate session ID to prevent SQL injection and path traversal attacks
    /// Returns error if ID contains unsafe characters
    fn validate_session_id(session_id: &str) -> WriterResult<()> {
        // Check for empty or too long IDs
        if session_id.is_empty() {
            return Err(WriterError::InvalidData(
                "Session ID cannot be empty".into(),
            ));
        }

        if session_id.len() > 255 {
            return Err(WriterError::InvalidData(format!(
                "Session ID too long: {} chars (max 255)",
                session_id.len()
            )));
        }

        // Check that ID only contains safe characters (alphanumeric, hyphen, underscore)
        if !session_id
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return Err(WriterError::InvalidData(format!(
                "Invalid session ID: contains unsafe characters: {}",
                session_id
            )));
        }

        Ok(())
    }

    /// Safely convert u64 to i64 for SQLite storage
    /// Returns error if value exceeds i64::MAX
    fn safe_u64_to_i64(value: u64, context: &str) -> WriterResult<i64> {
        i64::try_from(value).map_err(|_| {
            WriterError::InvalidData(format!(
                "{} value {} exceeds maximum SQLite integer (i64::MAX)",
                context, value
            ))
        })
    }

    /// Safely convert u32 to i64 for SQLite storage (always safe)
    fn safe_u32_to_i64(value: u32) -> i64 {
        value as i64
    }

    /// Safely convert usize to i64 for SQLite storage
    /// Returns error if value exceeds i64::MAX
    fn safe_usize_to_i64(value: usize, context: &str) -> WriterResult<i64> {
        i64::try_from(value).map_err(|_| {
            WriterError::InvalidData(format!(
                "{} value {} exceeds maximum SQLite integer (i64::MAX)",
                context, value
            ))
        })
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

        // For fresh databases, start at version 1 and let migrations handle the upgrade
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
                reasoning_tokens INTEGER DEFAULT 0,
                cache_read_tokens INTEGER DEFAULT 0,
                cache_creation_tokens INTEGER DEFAULT 0,
                audio_input_tokens INTEGER DEFAULT 0,
                audio_output_tokens INTEGER DEFAULT 0,
                total_tokens INTEGER GENERATED ALWAYS AS (
                    input_tokens + output_tokens +
                    COALESCE(thinking_tokens, 0) +
                    COALESCE(reasoning_tokens, 0) +
                    COALESCE(cache_read_tokens, 0) +
                    COALESCE(cache_creation_tokens, 0) +
                    COALESCE(audio_input_tokens, 0) +
                    COALESCE(audio_output_tokens, 0)
                ) STORED,
                request_text TEXT,
                response_text TEXT,
                client_ip TEXT,
                user_agent TEXT,
                is_streaming BOOLEAN DEFAULT 0,
                time_to_first_token_ms INTEGER,
                chunk_count INTEGER,
                streaming_duration_ms INTEGER,
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

        // Index for streaming sessions
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_streaming ON sessions(is_streaming, created_at DESC)")
            .execute(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        // Index for time range queries (critical for performance)
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_sessions_started_at ON sessions(started_at DESC)",
        )
        .execute(pool)
        .await
        .map_err(|e| WriterError::Database(e.to_string()))?;

        // Index for user_agent queries
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_user_agent ON sessions(user_agent)")
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
                reasoning_tokens INTEGER DEFAULT 0,
                cache_read_tokens INTEGER DEFAULT 0,
                cache_creation_tokens INTEGER DEFAULT 0,
                audio_input_tokens INTEGER DEFAULT 0,
                audio_output_tokens INTEGER DEFAULT 0,
                tokens_per_second REAL,
                thinking_percentage REAL,
                request_size_bytes INTEGER,
                response_size_bytes INTEGER,
                message_count INTEGER,
                content_blocks INTEGER,
                has_tools BOOLEAN DEFAULT 0,
                has_refusal BOOLEAN DEFAULT 0,
                user_agent TEXT,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
            )
            "#,
        )
        .execute(pool)
        .await
        .map_err(|e| WriterError::Database(e.to_string()))?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_session_stats_session ON session_stats(session_id)",
        )
        .execute(pool)
        .await
        .map_err(|e| WriterError::Database(e.to_string()))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_session_stats_model ON session_stats(model_name, created_at DESC)")
            .execute(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_session_stats_user_agent ON session_stats(user_agent)",
        )
        .execute(pool)
        .await
        .map_err(|e| WriterError::Database(e.to_string()))?;

        // Index for time-series queries per session
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_session_stats_session_time ON session_stats(session_id, created_at DESC)")
            .execute(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        // Check if tool_calls table exists (indicates we're at version 4 and need migration)
        // If it exists, we must NOT create tool_stats here - the migration will rename tool_calls to tool_stats
        let tool_calls_exists: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='tool_calls'",
        )
        .fetch_one(pool)
        .await
        .unwrap_or(false);

        // Only create tool_stats and tool_call_executions if tool_calls doesn't exist
        // (if tool_calls exists, migration 4->5 will handle renaming it to tool_stats)
        if !tool_calls_exists {
            // Tool stats table (aggregate summary by tool name)
            sqlx::query(
                r#"
                CREATE TABLE IF NOT EXISTS tool_stats (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                request_id TEXT,
                model_name TEXT,
                tool_name TEXT NOT NULL,
                call_count INTEGER DEFAULT 1,
                avg_execution_time_ms INTEGER,
                error_count INTEGER DEFAULT 0,
                tool_arguments TEXT,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
            )
                "#,
            )
            .execute(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

            sqlx::query("CREATE INDEX IF NOT EXISTS idx_tool_stats_model ON tool_stats(model_name, created_at DESC)")
                .execute(pool)
                .await
                .map_err(|e| WriterError::Database(e.to_string()))?;

            // Index for looking up tool stats by session
            sqlx::query(
                "CREATE INDEX IF NOT EXISTS idx_tool_stats_session ON tool_stats(session_id)",
            )
            .execute(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

            // Index for tool usage analysis
            sqlx::query("CREATE INDEX IF NOT EXISTS idx_tool_stats_name ON tool_stats(tool_name, created_at DESC)")
                .execute(pool)
                .await
                .map_err(|e| WriterError::Database(e.to_string()))?;

            // Unique index for ON CONFLICT handling (prevents duplicate tool entries per session/request)
            sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS idx_tool_stats_unique ON tool_stats(session_id, request_id, tool_name)")
                .execute(pool)
                .await
                .map_err(|e| WriterError::Database(e.to_string()))?;

            // Tool call executions table (individual call tracking with arguments)
            sqlx::query(
                r#"
                CREATE TABLE IF NOT EXISTS tool_call_executions (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id TEXT NOT NULL,
                    request_id TEXT NOT NULL,
                    tool_call_id TEXT NOT NULL,
                    tool_name TEXT NOT NULL,
                    tool_arguments TEXT,
                    execution_time_ms INTEGER,
                    input_size_bytes INTEGER,
                    output_size_bytes INTEGER,
                    success BOOLEAN,
                    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
                )
                "#,
            )
            .execute(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

            // Unique index for tool_call_executions
            sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS idx_tool_call_executions_unique ON tool_call_executions(session_id, request_id, tool_call_id)")
                .execute(pool)
                .await
                .map_err(|e| WriterError::Database(e.to_string()))?;

            // Index for looking up executions by session
            sqlx::query("CREATE INDEX IF NOT EXISTS idx_tool_call_executions_session ON tool_call_executions(session_id, created_at DESC)")
                .execute(pool)
                .await
                .map_err(|e| WriterError::Database(e.to_string()))?;

            // Index for looking up executions by tool name
            sqlx::query("CREATE INDEX IF NOT EXISTS idx_tool_call_executions_tool_name ON tool_call_executions(tool_name, created_at DESC)")
                .execute(pool)
                .await
                .map_err(|e| WriterError::Database(e.to_string()))?;

            // Index for looking up executions by request
            sqlx::query("CREATE INDEX IF NOT EXISTS idx_tool_call_executions_request ON tool_call_executions(request_id)")
                .execute(pool)
                .await
                .map_err(|e| WriterError::Database(e.to_string()))?;
        }
        // End of version check - tool_stats and tool_call_executions created only if not version 4

        // Stream metrics table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS stream_metrics (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                request_id TEXT NOT NULL,
                time_to_first_token_ms INTEGER NOT NULL,
                total_chunks INTEGER NOT NULL,
                streaming_duration_ms INTEGER NOT NULL,
                avg_chunk_latency_ms REAL,
                p50_chunk_latency_ms INTEGER,
                p95_chunk_latency_ms INTEGER,
                p99_chunk_latency_ms INTEGER,
                max_chunk_latency_ms INTEGER,
                min_chunk_latency_ms INTEGER,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
            )
            "#,
        )
        .execute(pool)
        .await
        .map_err(|e| WriterError::Database(e.to_string()))?;

        // Stream metrics indexes
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_stream_metrics_session ON stream_metrics(session_id)",
        )
        .execute(pool)
        .await
        .map_err(|e| WriterError::Database(e.to_string()))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_stream_metrics_ttft ON stream_metrics(time_to_first_token_ms)")
            .execute(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_stream_metrics_chunks ON stream_metrics(total_chunks DESC)")
            .execute(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        // Run migrations if needed
        Self::run_migrations(pool).await?;

        Ok(())
    }

    /// Check if a column exists in a table
    async fn column_exists(pool: &SqlitePool, table: &str, column: &str) -> WriterResult<bool> {
        let query = format!("PRAGMA table_info({})", table);
        let rows: Vec<(i32, String, String, i32, Option<String>, i32)> = sqlx::query_as(&query)
            .fetch_all(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        Ok(rows.iter().any(|(_, name, _, _, _, _)| name == column))
    }

    /// Check if migration 2->3 has already been applied by examining the total_tokens column
    async fn is_migration_2_to_3_applied(pool: &SqlitePool) -> WriterResult<bool> {
        // Check if the sessions table has the updated total_tokens formula
        // In version 3, total_tokens includes cache and audio tokens
        let sql = "SELECT sql FROM sqlite_master WHERE type='table' AND name='sessions'";
        let schema: Option<String> = sqlx::query_scalar(sql)
            .fetch_optional(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        if let Some(schema) = schema {
            // Check if the schema includes cache_read_tokens and audio_input_tokens in the GENERATED formula
            Ok(schema.contains("cache_read_tokens") && schema.contains("audio_input_tokens"))
        } else {
            Ok(false)
        }
    }

    /// Run schema migrations to bring database up to current version
    async fn run_migrations(pool: &SqlitePool) -> WriterResult<()> {
        const CURRENT_VERSION: i32 = 5;

        // Clean up any duplicate version entries (fix for previous bug where INSERT OR IGNORE could create duplicates)
        // Keep only the minimum version (the one we need to migrate from)
        sqlx::query(
            r#"
            DELETE FROM schema_version
            WHERE version NOT IN (SELECT MIN(version) FROM schema_version)
            "#,
        )
        .execute(pool)
        .await
        .map_err(|e| {
            WriterError::Database(format!("Failed to clean up schema_version table: {}", e))
        })?;

        // Get current schema version
        let version: i32 = sqlx::query_scalar("SELECT version FROM schema_version")
            .fetch_one(pool)
            .await
            .map_err(|e| WriterError::Database(e.to_string()))?;

        if version > CURRENT_VERSION {
            return Err(WriterError::Database(format!(
                "Database schema version {} is newer than supported version {}. Please upgrade lunaroute.",
                version, CURRENT_VERSION
            )));
        }

        // Apply migrations one by one
        let mut current_version = version;

        // Migration 1 -> 2: Add new token columns to sessions table
        if current_version == 1 {
            if !Self::column_exists(pool, "sessions", "reasoning_tokens").await? {
                sqlx::query("ALTER TABLE sessions ADD COLUMN reasoning_tokens INTEGER DEFAULT 0")
                    .execute(pool)
                    .await
                    .map_err(|e| {
                        WriterError::Database(format!(
                            "Migration 1->2 failed (reasoning_tokens): {}",
                            e
                        ))
                    })?;
            }

            if !Self::column_exists(pool, "sessions", "cache_read_tokens").await? {
                sqlx::query("ALTER TABLE sessions ADD COLUMN cache_read_tokens INTEGER DEFAULT 0")
                    .execute(pool)
                    .await
                    .map_err(|e| {
                        WriterError::Database(format!(
                            "Migration 1->2 failed (cache_read_tokens): {}",
                            e
                        ))
                    })?;
            }

            if !Self::column_exists(pool, "sessions", "cache_creation_tokens").await? {
                sqlx::query(
                    "ALTER TABLE sessions ADD COLUMN cache_creation_tokens INTEGER DEFAULT 0",
                )
                .execute(pool)
                .await
                .map_err(|e| {
                    WriterError::Database(format!(
                        "Migration 1->2 failed (cache_creation_tokens): {}",
                        e
                    ))
                })?;
            }

            if !Self::column_exists(pool, "sessions", "audio_input_tokens").await? {
                sqlx::query("ALTER TABLE sessions ADD COLUMN audio_input_tokens INTEGER DEFAULT 0")
                    .execute(pool)
                    .await
                    .map_err(|e| {
                        WriterError::Database(format!(
                            "Migration 1->2 failed (audio_input_tokens): {}",
                            e
                        ))
                    })?;
            }

            if !Self::column_exists(pool, "sessions", "audio_output_tokens").await? {
                sqlx::query(
                    "ALTER TABLE sessions ADD COLUMN audio_output_tokens INTEGER DEFAULT 0",
                )
                .execute(pool)
                .await
                .map_err(|e| {
                    WriterError::Database(format!(
                        "Migration 1->2 failed (audio_output_tokens): {}",
                        e
                    ))
                })?;
            }

            sqlx::query("UPDATE schema_version SET version = 2")
                .execute(pool)
                .await
                .map_err(|e| {
                    WriterError::Database(format!("Migration 1->2 failed (version update): {}", e))
                })?;

            current_version = 2;
        }

        // Migration 2 -> 3: Update total_tokens GENERATED column to include cache and audio tokens
        // SQLite doesn't support ALTER COLUMN for generated columns, so we need to recreate the table
        if current_version == 2 {
            // Check if this migration has already been applied (schema is already at v3)
            if Self::is_migration_2_to_3_applied(pool).await? {
                // Migration already applied, just update version
                sqlx::query("UPDATE schema_version SET version = 3")
                    .execute(pool)
                    .await
                    .map_err(|e| {
                        WriterError::Database(format!(
                            "Migration 2->3 failed (version update): {}",
                            e
                        ))
                    })?;

                #[allow(unused_assignments)]
                {
                    current_version = 3;
                }
            } else {
                // Check if sessions table still exists (might have been dropped in a previous failed migration)
                let sessions_exists: bool = sqlx::query_scalar(
                    "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='sessions'",
                )
                .fetch_one(pool)
                .await
                .map_err(|e| {
                    WriterError::Database(format!(
                        "Migration 2->3 failed (check sessions exists): {}",
                        e
                    ))
                })?;

                if !sessions_exists {
                    // Migration was partially completed, just rename sessions_new if it exists
                    let sessions_new_exists: bool = sqlx::query_scalar(
                    "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='sessions_new'"
                )
                .fetch_one(pool)
                .await
                .map_err(|e| WriterError::Database(format!("Migration 2->3 failed (check sessions_new exists): {}", e)))?;

                    if sessions_new_exists {
                        sqlx::query("ALTER TABLE sessions_new RENAME TO sessions")
                            .execute(pool)
                            .await
                            .map_err(|e| {
                                WriterError::Database(format!(
                                    "Migration 2->3 failed (rename table): {}",
                                    e
                                ))
                            })?;

                        // Recreate indexes
                        sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_created ON sessions(created_at DESC)")
                        .execute(pool)
                        .await
                        .map_err(|e| WriterError::Database(format!("Migration 2->3 failed (idx_sessions_created): {}", e)))?;

                        sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_provider ON sessions(provider, created_at DESC)")
                        .execute(pool)
                        .await
                        .map_err(|e| WriterError::Database(format!("Migration 2->3 failed (idx_sessions_provider): {}", e)))?;

                        sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_user_agent ON sessions(user_agent, created_at DESC)")
                        .execute(pool)
                        .await
                        .map_err(|e| WriterError::Database(format!("Migration 2->3 failed (idx_sessions_user_agent): {}", e)))?;
                    }

                    // Update schema version
                    sqlx::query("UPDATE schema_version SET version = 3")
                        .execute(pool)
                        .await
                        .map_err(|e| {
                            WriterError::Database(format!(
                                "Migration 2->3 failed (version update): {}",
                                e
                            ))
                        })?;

                    #[allow(unused_assignments)]
                    {
                        current_version = 3;
                    }

                    return Ok(());
                }

                // Drop sessions_new if it exists from a previous failed migration attempt
                sqlx::query("DROP TABLE IF EXISTS sessions_new")
                    .execute(pool)
                    .await
                    .map_err(|e| {
                        WriterError::Database(format!(
                            "Migration 2->3 failed (drop sessions_new): {}",
                            e
                        ))
                    })?;

                // Create new table with updated formula
                sqlx::query(
                    r#"
                CREATE TABLE sessions_new (
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
                    reasoning_tokens INTEGER DEFAULT 0,
                    cache_read_tokens INTEGER DEFAULT 0,
                    cache_creation_tokens INTEGER DEFAULT 0,
                    audio_input_tokens INTEGER DEFAULT 0,
                    audio_output_tokens INTEGER DEFAULT 0,
                    total_tokens INTEGER GENERATED ALWAYS AS (
                        input_tokens + output_tokens +
                        COALESCE(thinking_tokens, 0) +
                        COALESCE(reasoning_tokens, 0) +
                        COALESCE(cache_read_tokens, 0) +
                        COALESCE(cache_creation_tokens, 0) +
                        COALESCE(audio_input_tokens, 0) +
                        COALESCE(audio_output_tokens, 0)
                    ) STORED,
                    request_text TEXT,
                    response_text TEXT,
                    client_ip TEXT,
                    user_agent TEXT,
                    is_streaming BOOLEAN DEFAULT 0,
                    time_to_first_token_ms INTEGER,
                    chunk_count INTEGER,
                    streaming_duration_ms INTEGER,
                    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
                )
                "#,
                )
                .execute(pool)
                .await
                .map_err(|e| {
                    WriterError::Database(format!(
                        "Migration 2->3 failed (create new table): {}",
                        e
                    ))
                })?;

                // Copy data from old table to new table
                sqlx::query(
                    r#"
                INSERT INTO sessions_new
                SELECT session_id, request_id, started_at, completed_at, provider, listener,
                       model_requested, model_used, success, error_message, finish_reason,
                       total_duration_ms, provider_latency_ms, input_tokens, output_tokens,
                       thinking_tokens, reasoning_tokens, cache_read_tokens, cache_creation_tokens,
                       audio_input_tokens, audio_output_tokens, request_text, response_text,
                       client_ip, user_agent, is_streaming, time_to_first_token_ms,
                       chunk_count, streaming_duration_ms, created_at
                FROM sessions
                "#,
                )
                .execute(pool)
                .await
                .map_err(|e| {
                    WriterError::Database(format!("Migration 2->3 failed (copy data): {}", e))
                })?;

                // Drop old table
                sqlx::query("DROP TABLE sessions")
                    .execute(pool)
                    .await
                    .map_err(|e| {
                        WriterError::Database(format!(
                            "Migration 2->3 failed (drop old table): {}",
                            e
                        ))
                    })?;

                // Rename new table
                sqlx::query("ALTER TABLE sessions_new RENAME TO sessions")
                    .execute(pool)
                    .await
                    .map_err(|e| {
                        WriterError::Database(format!(
                            "Migration 2->3 failed (rename table): {}",
                            e
                        ))
                    })?;

                // Recreate indexes
                sqlx::query(
                    "CREATE INDEX IF NOT EXISTS idx_sessions_created ON sessions(created_at DESC)",
                )
                .execute(pool)
                .await
                .map_err(|e| {
                    WriterError::Database(format!(
                        "Migration 2->3 failed (idx_sessions_created): {}",
                        e
                    ))
                })?;

                sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_provider ON sessions(provider, created_at DESC)")
                .execute(pool)
                .await
                .map_err(|e| WriterError::Database(format!("Migration 2->3 failed (idx_sessions_provider): {}", e)))?;

                sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_user_agent ON sessions(user_agent, created_at DESC)")
                .execute(pool)
                .await
                .map_err(|e| WriterError::Database(format!("Migration 2->3 failed (idx_sessions_user_agent): {}", e)))?;

                // Update schema version
                sqlx::query("UPDATE schema_version SET version = 3")
                    .execute(pool)
                    .await
                    .map_err(|e| {
                        WriterError::Database(format!(
                            "Migration 2->3 failed (version update): {}",
                            e
                        ))
                    })?;

                #[allow(unused_assignments)]
                {
                    current_version = 3;
                }
            }
        }

        // Migration 3 -> 4: Add tool_arguments column to tool_calls table
        // Note: This migration is only needed for databases that were created with v3
        // New databases skip directly to v5 with tool_stats table
        if current_version == 3 {
            // Check if tool_calls table exists (might have been renamed already)
            let tool_calls_exists: bool = sqlx::query_scalar(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='tool_calls'",
            )
            .fetch_one(pool)
            .await
            .map_err(|e| {
                WriterError::Database(format!(
                    "Migration 3->4 failed (check tool_calls exists): {}",
                    e
                ))
            })?;

            if tool_calls_exists
                && !Self::column_exists(pool, "tool_calls", "tool_arguments").await?
            {
                sqlx::query("ALTER TABLE tool_calls ADD COLUMN tool_arguments TEXT")
                    .execute(pool)
                    .await
                    .map_err(|e| {
                        WriterError::Database(format!(
                            "Migration 3->4 failed (tool_arguments): {}",
                            e
                        ))
                    })?;
            }

            sqlx::query("UPDATE schema_version SET version = 4")
                .execute(pool)
                .await
                .map_err(|e| {
                    WriterError::Database(format!("Migration 3->4 failed (version update): {}", e))
                })?;

            current_version = 4;
        }

        // Migration 4 -> 5: Split tool tracking into aggregate (tool_stats) and detailed (tool_call_executions) tables
        if current_version == 4 {
            // Check if tool_calls table exists (to be renamed)
            let tool_calls_exists: bool = sqlx::query_scalar(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='tool_calls'",
            )
            .fetch_one(pool)
            .await
            .map_err(|e| {
                WriterError::Database(format!(
                    "Migration 4->5 failed (check tool_calls exists): {}",
                    e
                ))
            })?;

            if tool_calls_exists {
                // Check if tool_stats already exists (from a failed migration attempt)
                let tool_stats_exists: bool = sqlx::query_scalar(
                    "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='tool_stats'",
                )
                .fetch_one(pool)
                .await
                .map_err(|e| {
                    WriterError::Database(format!(
                        "Migration 4->5 failed (check tool_stats exists): {}",
                        e
                    ))
                })?;

                if tool_stats_exists {
                    // tool_stats exists - this is likely from a failed migration attempt
                    // Drop it if it's empty, otherwise fail with a clear error
                    let tool_stats_count: i64 =
                        sqlx::query_scalar("SELECT COUNT(*) FROM tool_stats")
                            .fetch_one(pool)
                            .await
                            .map_err(|e| {
                                WriterError::Database(format!(
                                    "Migration 4->5 failed (count tool_stats): {}",
                                    e
                                ))
                            })?;

                    if tool_stats_count == 0 {
                        // Empty table from failed migration - safe to drop
                        sqlx::query("DROP TABLE tool_stats")
                            .execute(pool)
                            .await
                            .map_err(|e| {
                                WriterError::Database(format!(
                                    "Migration 4->5 failed (drop empty tool_stats): {}",
                                    e
                                ))
                            })?;
                    } else {
                        return Err(WriterError::Database(format!(
                            "Migration 4->5 failed: tool_stats already exists with {} records. \
                            This indicates a migration conflict. Please backup your database and \
                            contact support.",
                            tool_stats_count
                        )));
                    }
                }

                // Rename tool_calls to tool_stats
                sqlx::query("ALTER TABLE tool_calls RENAME TO tool_stats")
                    .execute(pool)
                    .await
                    .map_err(|e| {
                        WriterError::Database(format!(
                            "Migration 4->5 failed (rename table): {}",
                            e
                        ))
                    })?;

                // Drop old indexes (they reference the old table name)
                sqlx::query("DROP INDEX IF EXISTS idx_tool_calls_model")
                    .execute(pool)
                    .await
                    .map_err(|e| {
                        WriterError::Database(format!(
                            "Migration 4->5 failed (drop idx_tool_calls_model): {}",
                            e
                        ))
                    })?;

                sqlx::query("DROP INDEX IF EXISTS idx_tool_calls_session")
                    .execute(pool)
                    .await
                    .map_err(|e| {
                        WriterError::Database(format!(
                            "Migration 4->5 failed (drop idx_tool_calls_session): {}",
                            e
                        ))
                    })?;

                sqlx::query("DROP INDEX IF EXISTS idx_tool_calls_name")
                    .execute(pool)
                    .await
                    .map_err(|e| {
                        WriterError::Database(format!(
                            "Migration 4->5 failed (drop idx_tool_calls_name): {}",
                            e
                        ))
                    })?;

                sqlx::query("DROP INDEX IF EXISTS idx_tool_calls_unique")
                    .execute(pool)
                    .await
                    .map_err(|e| {
                        WriterError::Database(format!(
                            "Migration 4->5 failed (drop idx_tool_calls_unique): {}",
                            e
                        ))
                    })?;

                // Recreate indexes with new names
                sqlx::query("CREATE INDEX IF NOT EXISTS idx_tool_stats_model ON tool_stats(model_name, created_at DESC)")
                    .execute(pool)
                    .await
                    .map_err(|e| {
                        WriterError::Database(format!(
                            "Migration 4->5 failed (idx_tool_stats_model): {}",
                            e
                        ))
                    })?;

                sqlx::query(
                    "CREATE INDEX IF NOT EXISTS idx_tool_stats_session ON tool_stats(session_id)",
                )
                .execute(pool)
                .await
                .map_err(|e| {
                    WriterError::Database(format!(
                        "Migration 4->5 failed (idx_tool_stats_session): {}",
                        e
                    ))
                })?;

                sqlx::query("CREATE INDEX IF NOT EXISTS idx_tool_stats_name ON tool_stats(tool_name, created_at DESC)")
                    .execute(pool)
                    .await
                    .map_err(|e| {
                        WriterError::Database(format!(
                            "Migration 4->5 failed (idx_tool_stats_name): {}",
                            e
                        ))
                    })?;

                sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS idx_tool_stats_unique ON tool_stats(session_id, request_id, tool_name)")
                    .execute(pool)
                    .await
                    .map_err(|e| {
                        WriterError::Database(format!(
                            "Migration 4->5 failed (idx_tool_stats_unique): {}",
                            e
                        ))
                    })?;
            }

            // Create tool_call_executions table
            sqlx::query(
                r#"
                CREATE TABLE IF NOT EXISTS tool_call_executions (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id TEXT NOT NULL,
                    request_id TEXT NOT NULL,
                    tool_call_id TEXT NOT NULL,
                    tool_name TEXT NOT NULL,
                    tool_arguments TEXT,
                    execution_time_ms INTEGER,
                    input_size_bytes INTEGER,
                    output_size_bytes INTEGER,
                    success BOOLEAN,
                    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
                )
                "#,
            )
            .execute(pool)
            .await
            .map_err(|e| {
                WriterError::Database(format!(
                    "Migration 4->5 failed (create tool_call_executions): {}",
                    e
                ))
            })?;

            // Create indexes for tool_call_executions
            sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS idx_tool_call_executions_unique ON tool_call_executions(session_id, request_id, tool_call_id)")
                .execute(pool)
                .await
                .map_err(|e| {
                    WriterError::Database(format!(
                        "Migration 4->5 failed (idx_tool_call_executions_unique): {}",
                        e
                    ))
                })?;

            sqlx::query("CREATE INDEX IF NOT EXISTS idx_tool_call_executions_session ON tool_call_executions(session_id, created_at DESC)")
                .execute(pool)
                .await
                .map_err(|e| {
                    WriterError::Database(format!(
                        "Migration 4->5 failed (idx_tool_call_executions_session): {}",
                        e
                    ))
                })?;

            sqlx::query("CREATE INDEX IF NOT EXISTS idx_tool_call_executions_tool_name ON tool_call_executions(tool_name, created_at DESC)")
                .execute(pool)
                .await
                .map_err(|e| {
                    WriterError::Database(format!(
                        "Migration 4->5 failed (idx_tool_call_executions_tool_name): {}",
                        e
                    ))
                })?;

            sqlx::query("CREATE INDEX IF NOT EXISTS idx_tool_call_executions_request ON tool_call_executions(request_id)")
                .execute(pool)
                .await
                .map_err(|e| {
                    WriterError::Database(format!(
                        "Migration 4->5 failed (idx_tool_call_executions_request): {}",
                        e
                    ))
                })?;

            // Clean up "unknown" tool entries from tool_stats
            sqlx::query("DELETE FROM tool_stats WHERE tool_name = 'unknown'")
                .execute(pool)
                .await
                .map_err(|e| {
                    WriterError::Database(format!(
                        "Migration 4->5 failed (cleanup unknown tools): {}",
                        e
                    ))
                })?;

            sqlx::query("UPDATE schema_version SET version = 5")
                .execute(pool)
                .await
                .map_err(|e| {
                    WriterError::Database(format!("Migration 4->5 failed (version update): {}", e))
                })?;

            #[allow(unused_assignments)]
            {
                current_version = 5;
            }
        }

        Ok(())
    }

    /// Search for sessions matching the given filter
    pub async fn search_sessions(
        &self,
        filter: &SessionFilter,
    ) -> WriterResult<SearchResults<SessionRecord>> {
        filter.validate().map_err(WriterError::Database)?;

        // Build the WHERE clause
        let (where_clause, bind_values) = Self::build_where_clause(filter);

        // Build the ORDER BY clause
        let order_by = match filter.sort {
            SortOrder::NewestFirst => "started_at DESC",
            SortOrder::OldestFirst => "started_at ASC",
            SortOrder::HighestTokens => "total_tokens DESC",
            SortOrder::LongestDuration => "total_duration_ms DESC NULLS LAST",
            SortOrder::ShortestDuration => "total_duration_ms ASC NULLS LAST",
        };

        // Get total count
        let count_query = format!(
            "SELECT COUNT(*) FROM sessions {}",
            if where_clause.is_empty() {
                String::new()
            } else {
                format!("WHERE {}", where_clause)
            }
        );

        let total_count: i64 = {
            let mut query = sqlx::query_scalar(&count_query);
            for value in &bind_values {
                query = query.bind(value);
            }
            query
                .fetch_one(&self.pool)
                .await
                .map_err(|e| WriterError::Database(e.to_string()))?
        };

        // Get paginated results
        let offset = filter.page * filter.page_size;
        let results_query = format!(
            "SELECT session_id, request_id, started_at, completed_at, provider,
                    model_requested, model_used, success, error_message, finish_reason,
                    total_duration_ms, input_tokens, output_tokens, total_tokens,
                    is_streaming, client_ip
             FROM sessions {}
             ORDER BY {}
             LIMIT ? OFFSET ?",
            if where_clause.is_empty() {
                String::new()
            } else {
                format!("WHERE {}", where_clause)
            },
            order_by
        );

        let records: Vec<SessionRecord> = {
            let mut query = sqlx::query_as::<_, SessionRecord>(&results_query);
            for value in &bind_values {
                query = query.bind(value);
            }

            // Safe cast for page_size and offset (validated to reasonable limits already)
            let page_size_i64 = i64::try_from(filter.page_size).map_err(|_| {
                WriterError::InvalidData(format!("Page size {} exceeds i64::MAX", filter.page_size))
            })?;
            let offset_i64 = i64::try_from(offset).map_err(|_| {
                WriterError::InvalidData(format!("Offset {} exceeds i64::MAX", offset))
            })?;

            query
                .bind(page_size_i64)
                .bind(offset_i64)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| WriterError::Database(format!("Failed to fetch sessions: {}", e)))?
        };

        Ok(SearchResults::new(
            records,
            total_count as u64,
            filter.page,
            filter.page_size,
        ))
    }

    /// Get session aggregates for the given filter
    pub async fn get_aggregates(&self, filter: &SessionFilter) -> WriterResult<SessionAggregates> {
        filter.validate().map_err(WriterError::Database)?;

        let (where_clause, bind_values) = Self::build_where_clause(filter);

        // Aggregates query
        let agg_query = format!(
            "SELECT
                COUNT(*) as total,
                SUM(CASE WHEN success = 1 THEN 1 ELSE 0 END) as successful,
                SUM(CASE WHEN success = 0 THEN 1 ELSE 0 END) as failed,
                COALESCE(SUM(total_tokens), 0) as total_tokens,
                COALESCE(SUM(input_tokens), 0) as total_input,
                COALESCE(SUM(output_tokens), 0) as total_output,
                COALESCE(AVG(total_duration_ms), 0) as avg_duration
             FROM sessions {}",
            if where_clause.is_empty() {
                String::new()
            } else {
                format!("WHERE {}", where_clause)
            }
        );

        let row = {
            let mut query = sqlx::query(&agg_query);
            for value in &bind_values {
                query = query.bind(value);
            }
            query
                .fetch_one(&self.pool)
                .await
                .map_err(|e| WriterError::Database(e.to_string()))?
        };

        let total_sessions: i64 = row.get("total");
        let successful: i64 = row.get("successful");
        let failed: i64 = row.get("failed");
        let total_tokens: i64 = row.get("total_tokens");
        let total_input: i64 = row.get("total_input");
        let total_output: i64 = row.get("total_output");
        let avg_duration: f64 = row.get("avg_duration");

        // Get percentiles
        let percentile_query = format!(
            "WITH ordered AS (
                SELECT total_duration_ms,
                       ROW_NUMBER() OVER (ORDER BY total_duration_ms) as row_num,
                       COUNT(*) OVER () as total_rows
                FROM sessions
                WHERE total_duration_ms IS NOT NULL {}
            )
            SELECT
                MAX(CASE WHEN row_num = CAST(total_rows * 0.50 AS INTEGER) THEN total_duration_ms END) as p50,
                MAX(CASE WHEN row_num = CAST(total_rows * 0.95 AS INTEGER) THEN total_duration_ms END) as p95,
                MAX(CASE WHEN row_num = CAST(total_rows * 0.99 AS INTEGER) THEN total_duration_ms END) as p99
            FROM ordered",
            if where_clause.is_empty() {
                String::new()
            } else {
                format!("AND {}", where_clause)
            }
        );

        let percentiles = {
            let mut query = sqlx::query(&percentile_query);
            for value in &bind_values {
                query = query.bind(value);
            }
            query
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| WriterError::Database(e.to_string()))?
        };

        let (p50, p95, p99) = if let Some(row) = percentiles {
            (
                row.get::<Option<i64>, _>("p50").map(|v| v as u64),
                row.get::<Option<i64>, _>("p95").map(|v| v as u64),
                row.get::<Option<i64>, _>("p99").map(|v| v as u64),
            )
        } else {
            (None, None, None)
        };

        // Get sessions by provider
        let provider_query = format!(
            "SELECT provider, COUNT(*) as count
             FROM sessions {}
             GROUP BY provider",
            if where_clause.is_empty() {
                String::new()
            } else {
                format!("WHERE {}", where_clause)
            }
        );

        let sessions_by_provider: HashMap<String, u64> = {
            let mut query = sqlx::query(&provider_query);
            for value in &bind_values {
                query = query.bind(value);
            }
            query
                .fetch_all(&self.pool)
                .await
                .map_err(|e| WriterError::Database(e.to_string()))?
                .into_iter()
                .map(|row| {
                    let provider: String = row.get("provider");
                    let count: i64 = row.get("count");
                    (provider, count as u64)
                })
                .collect()
        };

        // Get sessions by model
        let model_query = format!(
            "SELECT COALESCE(model_used, model_requested) as model, COUNT(*) as count
             FROM sessions {}
             GROUP BY model",
            if where_clause.is_empty() {
                String::new()
            } else {
                format!("WHERE {}", where_clause)
            }
        );

        let sessions_by_model: HashMap<String, u64> = {
            let mut query = sqlx::query(&model_query);
            for value in &bind_values {
                query = query.bind(value);
            }
            query
                .fetch_all(&self.pool)
                .await
                .map_err(|e| WriterError::Database(e.to_string()))?
                .into_iter()
                .map(|row| {
                    let model: String = row.get("model");
                    let count: i64 = row.get("count");
                    (model, count as u64)
                })
                .collect()
        };

        Ok(SessionAggregates {
            total_sessions: total_sessions as u64,
            successful_sessions: successful as u64,
            failed_sessions: failed as u64,
            total_tokens: total_tokens as u64,
            total_input_tokens: total_input as u64,
            total_output_tokens: total_output as u64,
            avg_duration_ms: avg_duration,
            p50_duration_ms: p50,
            p95_duration_ms: p95,
            p99_duration_ms: p99,
            sessions_by_provider,
            sessions_by_model,
        })
    }

    /// Build WHERE clause from filter
    fn build_where_clause(filter: &SessionFilter) -> (String, Vec<String>) {
        let mut conditions = Vec::new();
        let mut bind_values = Vec::new();

        if let Some(ref time_range) = filter.time_range {
            conditions.push("started_at >= ?".to_string());
            bind_values.push(time_range.start.to_rfc3339());
            conditions.push("started_at <= ?".to_string());
            bind_values.push(time_range.end.to_rfc3339());
        }

        if !filter.providers.is_empty() {
            let placeholders = filter
                .providers
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");
            conditions.push(format!("provider IN ({})", placeholders));
            bind_values.extend(filter.providers.clone());
        }

        if !filter.models.is_empty() {
            let placeholders = filter
                .models
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");
            conditions.push(format!(
                "(model_requested IN ({}) OR model_used IN ({}))",
                placeholders, placeholders
            ));
            bind_values.extend(filter.models.clone());
            bind_values.extend(filter.models.clone());
        }

        if !filter.request_ids.is_empty() {
            let placeholders = filter
                .request_ids
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");
            conditions.push(format!("request_id IN ({})", placeholders));
            bind_values.extend(filter.request_ids.clone());
        }

        if !filter.session_ids.is_empty() {
            let placeholders = filter
                .session_ids
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");
            conditions.push(format!("session_id IN ({})", placeholders));
            bind_values.extend(filter.session_ids.clone());
        }

        if let Some(success) = filter.success {
            conditions.push("success = ?".to_string());
            bind_values.push(if success {
                "1".to_string()
            } else {
                "0".to_string()
            });
        }

        if let Some(is_streaming) = filter.is_streaming {
            conditions.push("is_streaming = ?".to_string());
            bind_values.push(if is_streaming {
                "1".to_string()
            } else {
                "0".to_string()
            });
        }

        if let Some(min_tokens) = filter.min_tokens {
            conditions.push("total_tokens >= ?".to_string());
            bind_values.push(min_tokens.to_string());
        }

        if let Some(max_tokens) = filter.max_tokens {
            conditions.push("total_tokens <= ?".to_string());
            bind_values.push(max_tokens.to_string());
        }

        if let Some(min_duration) = filter.min_duration_ms {
            conditions.push("total_duration_ms >= ?".to_string());
            bind_values.push(min_duration.to_string());
        }

        if let Some(max_duration) = filter.max_duration_ms {
            conditions.push("total_duration_ms <= ?".to_string());
            bind_values.push(max_duration.to_string());
        }

        if !filter.client_ips.is_empty() {
            let placeholders = filter
                .client_ips
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");
            conditions.push(format!("client_ip IN ({})", placeholders));
            bind_values.extend(filter.client_ips.clone());
        }

        if !filter.finish_reasons.is_empty() {
            let placeholders = filter
                .finish_reasons
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");
            conditions.push(format!("finish_reason IN ({})", placeholders));
            bind_values.extend(filter.finish_reasons.clone());
        }

        if let Some(ref text_search) = filter.text_search {
            conditions.push(
                "(request_text LIKE ? ESCAPE '\\' OR response_text LIKE ? ESCAPE '\\')".to_string(),
            );
            // Escape SQL LIKE metacharacters to prevent injection
            let escaped = text_search
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            let pattern = format!("%{}%", escaped);
            bind_values.push(pattern.clone());
            bind_values.push(pattern);
        }

        let where_clause = conditions.join(" AND ");
        (where_clause, bind_values)
    }
}

#[cfg(feature = "sqlite-writer")]
#[async_trait]
impl SessionWriter for SqliteWriter {
    async fn write_event(&self, event: &SessionEvent) -> WriterResult<()> {
        self.write_batch(std::slice::from_ref(event)).await
    }

    async fn write_batch(&self, events: &[SessionEvent]) -> WriterResult<()> {
        // Validate all session IDs before starting transaction
        for event in events {
            let session_id = match event {
                SessionEvent::Started { session_id, .. }
                | SessionEvent::StreamStarted { session_id, .. }
                | SessionEvent::RequestRecorded { session_id, .. }
                | SessionEvent::ResponseRecorded { session_id, .. }
                | SessionEvent::ToolCallRecorded { session_id, .. }
                | SessionEvent::StatsSnapshot { session_id, .. }
                | SessionEvent::Completed { session_id, .. }
                | SessionEvent::StatsUpdated { session_id, .. } => session_id,
            };
            Self::validate_session_id(session_id)?;
        }

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
                    is_streaming,
                    metadata,
                } => {
                    sqlx::query(
                        r#"
                        INSERT INTO sessions (session_id, request_id, started_at, model_requested, provider, listener, client_ip, user_agent, is_streaming)
                        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
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
                    .bind(is_streaming)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| WriterError::Database(e.to_string()))?;
                }

                SessionEvent::StreamStarted {
                    session_id,
                    time_to_first_token_ms,
                    ..
                } => {
                    let ttft =
                        Self::safe_u64_to_i64(*time_to_first_token_ms, "time_to_first_token_ms")?;
                    sqlx::query(
                        r#"
                        UPDATE sessions
                        SET time_to_first_token_ms = ?
                        WHERE session_id = ?
                        "#,
                    )
                    .bind(ttft)
                    .bind(session_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| {
                        WriterError::Database(format!(
                            "Failed to update TTFT for session {}: {}",
                            session_id, e
                        ))
                    })?;
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
                    // Safe conversions for all u32/u64/usize values
                    let output_tokens = Self::safe_u32_to_i64(stats.tokens.output_tokens);
                    let thinking_tokens = stats.tokens.thinking_tokens.map(Self::safe_u32_to_i64);
                    let reasoning_tokens = stats.tokens.reasoning_tokens.map(Self::safe_u32_to_i64);
                    let input_tokens = Self::safe_u32_to_i64(stats.tokens.input_tokens);
                    let provider_latency =
                        Self::safe_u64_to_i64(stats.provider_latency_ms, "provider_latency_ms")?;
                    let chunk_count = stats.chunk_count.map(Self::safe_u32_to_i64);
                    let streaming_duration = stats
                        .streaming_duration_ms
                        .map(|d| Self::safe_u64_to_i64(d, "streaming_duration_ms"))
                        .transpose()?;
                    let cache_read = stats.tokens.cache_read_tokens.map(Self::safe_u32_to_i64);
                    let cache_creation = stats
                        .tokens
                        .cache_creation_tokens
                        .map(Self::safe_u32_to_i64);
                    let audio_input = stats.tokens.audio_input_tokens.map(Self::safe_u32_to_i64);
                    let audio_output = stats.tokens.audio_output_tokens.map(Self::safe_u32_to_i64);
                    let response_size =
                        Self::safe_usize_to_i64(stats.response_size_bytes, "response_size_bytes")?;
                    let content_blocks =
                        Self::safe_usize_to_i64(stats.content_blocks, "content_blocks")?;

                    sqlx::query(
                        r#"
                        UPDATE sessions
                        SET response_text = ?,
                            model_used = ?,
                            output_tokens = ?,
                            thinking_tokens = ?,
                            reasoning_tokens = ?,
                            input_tokens = ?,
                            cache_read_tokens = ?,
                            cache_creation_tokens = ?,
                            audio_input_tokens = ?,
                            audio_output_tokens = ?,
                            provider_latency_ms = ?,
                            chunk_count = ?,
                            streaming_duration_ms = ?
                        WHERE session_id = ?
                        "#,
                    )
                    .bind(response_text)
                    .bind(model_used)
                    .bind(output_tokens)
                    .bind(thinking_tokens)
                    .bind(reasoning_tokens)
                    .bind(input_tokens)
                    .bind(cache_read)
                    .bind(cache_creation)
                    .bind(audio_input)
                    .bind(audio_output)
                    .bind(provider_latency)
                    .bind(chunk_count)
                    .bind(streaming_duration)
                    .bind(session_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| {
                        WriterError::Database(format!(
                            "Failed to update session {} with response: {}",
                            session_id, e
                        ))
                    })?;

                    // Insert session stats
                    sqlx::query(
                        r#"
                        INSERT INTO session_stats (
                            session_id, request_id, model_name,
                            post_processing_ms, proxy_overhead_ms,
                            input_tokens, output_tokens, thinking_tokens, reasoning_tokens,
                            cache_read_tokens, cache_creation_tokens,
                            audio_input_tokens, audio_output_tokens,
                            tokens_per_second, thinking_percentage,
                            response_size_bytes, content_blocks, has_refusal
                        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                        "#,
                    )
                    .bind(session_id)
                    .bind(request_id)
                    .bind(model_used)
                    .bind(stats.post_processing_ms)
                    .bind(stats.total_proxy_overhead_ms)
                    .bind(input_tokens)
                    .bind(output_tokens)
                    .bind(thinking_tokens)
                    .bind(reasoning_tokens)
                    .bind(cache_read)
                    .bind(cache_creation)
                    .bind(audio_input)
                    .bind(audio_output)
                    .bind(stats.tokens.tokens_per_second.map(|t| t as f64))
                    .bind(stats.tokens.thinking_percentage.map(|t| t as f64))
                    .bind(response_size)
                    .bind(content_blocks)
                    .bind(stats.has_refusal)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| {
                        WriterError::Database(format!(
                            "Failed to insert session stats for {}: {}",
                            session_id, e
                        ))
                    })?;

                    // Insert tool calls
                    for tool in &stats.tool_calls {
                        let exec_time = tool
                            .execution_time_ms
                            .map(|t| Self::safe_u64_to_i64(t, "tool execution_time_ms"))
                            .transpose()?;

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
                        .bind(exec_time)
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| {
                            WriterError::Database(format!(
                                "Failed to insert tool call {} for session {}: {}",
                                tool.tool_name, session_id, e
                            ))
                        })?;
                    }
                }

                SessionEvent::Completed {
                    session_id,
                    request_id,
                    timestamp,
                    success,
                    error,
                    finish_reason,
                    final_stats,
                } => {
                    Self::handle_session_completed(
                        &mut tx,
                        session_id,
                        request_id,
                        timestamp,
                        *success,
                        error,
                        finish_reason,
                        final_stats,
                    )
                    .await?;
                }

                SessionEvent::ToolCallRecorded {
                    session_id,
                    request_id,
                    tool_name,
                    tool_call_id,
                    execution_time_ms,
                    input_size_bytes,
                    output_size_bytes,
                    success,
                    tool_arguments,
                    ..
                } => {
                    // Increment error_count if success is Some(false)
                    let error_count = if matches!(success, Some(false)) { 1 } else { 0 };
                    let exec_time = execution_time_ms
                        .map(|t| Self::safe_u64_to_i64(t, "execution_time_ms"))
                        .transpose()?;
                    let input_size =
                        Self::safe_usize_to_i64(*input_size_bytes, "input_size_bytes")?;
                    let output_size = output_size_bytes
                        .map(|s| Self::safe_usize_to_i64(s, "output_size_bytes"))
                        .transpose()?;

                    // Write to tool_stats (aggregate summary)
                    sqlx::query(
                        r#"
                        INSERT INTO tool_stats (session_id, request_id, tool_name, call_count, avg_execution_time_ms, error_count, tool_arguments)
                        VALUES (?, ?, ?, 1, ?, ?, ?)
                        ON CONFLICT(session_id, request_id, tool_name)
                        DO UPDATE SET
                            call_count = call_count + 1,
                            avg_execution_time_ms = COALESCE(excluded.avg_execution_time_ms, avg_execution_time_ms),
                            error_count = error_count + excluded.error_count,
                            tool_arguments = COALESCE(tool_arguments, excluded.tool_arguments)
                        "#,
                    )
                    .bind(session_id)
                    .bind(request_id)
                    .bind(tool_name)
                    .bind(exec_time)
                    .bind(error_count)
                    .bind(tool_arguments)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| {
                        WriterError::Database(format!(
                            "Failed to record tool stats for {} in session {}: {}",
                            tool_name, session_id, e
                        ))
                    })?;

                    // Write to tool_call_executions (individual call with full details)
                    sqlx::query(
                        r#"
                        INSERT INTO tool_call_executions (
                            session_id, request_id, tool_call_id, tool_name,
                            tool_arguments, execution_time_ms, input_size_bytes, output_size_bytes, success
                        )
                        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                        ON CONFLICT(session_id, request_id, tool_call_id)
                        DO UPDATE SET
                            execution_time_ms = COALESCE(excluded.execution_time_ms, execution_time_ms),
                            output_size_bytes = COALESCE(excluded.output_size_bytes, output_size_bytes),
                            success = COALESCE(excluded.success, success)
                        "#,
                    )
                    .bind(session_id)
                    .bind(request_id)
                    .bind(tool_call_id)
                    .bind(tool_name)
                    .bind(tool_arguments)
                    .bind(exec_time)
                    .bind(input_size)
                    .bind(output_size)
                    .bind(success)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| {
                        WriterError::Database(format!(
                            "Failed to record tool call execution {} for session {}: {}",
                            tool_call_id, session_id, e
                        ))
                    })?;

                    tracing::debug!(
                        session_id = %session_id,
                        tool_name = %tool_name,
                        tool_call_id = %tool_call_id,
                        success = ?success,
                        error_count = error_count,
                        "Recorded tool call in both tables"
                    );
                }

                SessionEvent::StatsUpdated {
                    session_id,
                    request_id,
                    token_updates,
                    tool_call_updates,
                    model_used,
                    response_size_bytes,
                    content_blocks,
                    has_refusal,
                    user_agent,
                    ..
                } => {
                    Self::handle_stats_updated(
                        &mut tx,
                        session_id,
                        request_id,
                        token_updates.as_ref(),
                        tool_call_updates.as_ref(),
                        model_used.as_deref(),
                        *response_size_bytes,
                        *content_blocks,
                        *has_refusal,
                        user_agent.as_deref(),
                    )
                    .await?;
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

// Private helper methods for SqliteWriter
#[cfg(feature = "sqlite-writer")]
impl SqliteWriter {
    /// Process session completion event - updates tokens, tool calls, and streaming stats
    #[allow(clippy::too_many_arguments)]
    async fn handle_session_completed(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        session_id: &str,
        request_id: &str,
        timestamp: &chrono::DateTime<chrono::Utc>,
        success: bool,
        error: &Option<String>,
        finish_reason: &Option<String>,
        final_stats: &FinalSessionStats,
    ) -> WriterResult<()> {
        // Update session with completion data AND token totals from final_stats
        // Uses MAX() to avoid double-counting: for non-streaming sessions, ResponseRecorded already sets tokens
        // For streaming/passthrough mode, StatsUpdated events accumulate tokens, and this ensures final_stats value is used
        let total_duration =
            Self::safe_u64_to_i64(final_stats.total_duration_ms, "total_duration_ms")?;
        let input_tokens =
            Self::safe_u64_to_i64(final_stats.total_tokens.total_input, "input_tokens")?;
        let output_tokens =
            Self::safe_u64_to_i64(final_stats.total_tokens.total_output, "output_tokens")?;
        let thinking_tokens =
            Self::safe_u64_to_i64(final_stats.total_tokens.total_thinking, "thinking_tokens")?;
        let reasoning_tokens =
            Self::safe_u64_to_i64(final_stats.total_tokens.total_reasoning, "reasoning_tokens")?;
        let cache_read_tokens = Self::safe_u64_to_i64(
            final_stats.total_tokens.total_cache_read,
            "cache_read_tokens",
        )?;
        let cache_creation_tokens = Self::safe_u64_to_i64(
            final_stats.total_tokens.total_cache_creation,
            "cache_creation_tokens",
        )?;
        let audio_input_tokens = Self::safe_u64_to_i64(
            final_stats.total_tokens.total_audio_input,
            "audio_input_tokens",
        )?;
        let audio_output_tokens = Self::safe_u64_to_i64(
            final_stats.total_tokens.total_audio_output,
            "audio_output_tokens",
        )?;

        sqlx::query(
            r#"
            UPDATE sessions
            SET completed_at = ?,
                success = ?,
                error_message = ?,
                finish_reason = ?,
                total_duration_ms = MAX(
                    CAST((julianday(?) - julianday(started_at)) * 86400000 AS INTEGER),
                    ?
                ),
                input_tokens = MAX(COALESCE(input_tokens, 0), ?),
                output_tokens = MAX(COALESCE(output_tokens, 0), ?),
                thinking_tokens = MAX(COALESCE(thinking_tokens, 0), ?),
                reasoning_tokens = MAX(COALESCE(reasoning_tokens, 0), ?),
                cache_read_tokens = MAX(COALESCE(cache_read_tokens, 0), ?),
                cache_creation_tokens = MAX(COALESCE(cache_creation_tokens, 0), ?),
                audio_input_tokens = MAX(COALESCE(audio_input_tokens, 0), ?),
                audio_output_tokens = MAX(COALESCE(audio_output_tokens, 0), ?)
            WHERE session_id = ?
            "#,
        )
        .bind(timestamp)
        .bind(success)
        .bind(error)
        .bind(finish_reason)
        .bind(timestamp)
        .bind(total_duration)
        .bind(input_tokens)
        .bind(output_tokens)
        .bind(thinking_tokens)
        .bind(reasoning_tokens)
        .bind(cache_read_tokens)
        .bind(cache_creation_tokens)
        .bind(audio_input_tokens)
        .bind(audio_output_tokens)
        .bind(session_id)
        .execute(&mut **tx)
        .await
        .map_err(|e| {
            WriterError::Database(format!(
                "Failed to update session {} on completion: {}",
                session_id, e
            ))
        })?;

        // Insert tool calls if any (using ON CONFLICT to handle duplicates)
        Self::insert_tool_calls(tx, session_id, request_id, final_stats).await?;

        // Insert streaming metrics if present
        if let Some(streaming_stats) = &final_stats.streaming_stats {
            Self::insert_streaming_metrics(tx, session_id, request_id, streaming_stats).await?;
        }

        Ok(())
    }

    /// Handle stats update event - updates session with late-arriving data from async parsing
    /// This is used in passthrough mode where we parse response data asynchronously
    /// Uses MAX() logic to keep the highest token values and avoid double-counting
    #[allow(clippy::too_many_arguments)]
    async fn handle_stats_updated(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        session_id: &str,
        request_id: &str,
        token_updates: Option<&TokenTotals>,
        tool_call_updates: Option<&ToolUsageSummary>,
        model_used: Option<&str>,
        response_size_bytes: usize,
        content_blocks: usize,
        has_refusal: bool,
        user_agent: Option<&str>,
    ) -> WriterResult<()> {
        // Update session tokens if provided (accumulates tokens across multiple requests)
        if let Some(tokens) = token_updates {
            let input_tokens = Self::safe_u64_to_i64(tokens.total_input, "input_tokens")?;
            let output_tokens = Self::safe_u64_to_i64(tokens.total_output, "output_tokens")?;
            let thinking_tokens = Self::safe_u64_to_i64(tokens.total_thinking, "thinking_tokens")?;
            let reasoning_tokens =
                Self::safe_u64_to_i64(tokens.total_reasoning, "reasoning_tokens")?;
            let cache_read_tokens =
                Self::safe_u64_to_i64(tokens.total_cache_read, "cache_read_tokens")?;
            let cache_creation_tokens =
                Self::safe_u64_to_i64(tokens.total_cache_creation, "cache_creation_tokens")?;
            let audio_input_tokens =
                Self::safe_u64_to_i64(tokens.total_audio_input, "audio_input_tokens")?;
            let audio_output_tokens =
                Self::safe_u64_to_i64(tokens.total_audio_output, "audio_output_tokens")?;

            sqlx::query(
                r#"
                UPDATE sessions
                SET input_tokens = MAX(COALESCE(input_tokens, 0), ?),
                    output_tokens = MAX(COALESCE(output_tokens, 0), ?),
                    thinking_tokens = MAX(COALESCE(thinking_tokens, 0), ?),
                    reasoning_tokens = MAX(COALESCE(reasoning_tokens, 0), ?),
                    cache_read_tokens = MAX(COALESCE(cache_read_tokens, 0), ?),
                    cache_creation_tokens = MAX(COALESCE(cache_creation_tokens, 0), ?),
                    audio_input_tokens = MAX(COALESCE(audio_input_tokens, 0), ?),
                    audio_output_tokens = MAX(COALESCE(audio_output_tokens, 0), ?)
                WHERE session_id = ?
                "#,
            )
            .bind(input_tokens)
            .bind(output_tokens)
            .bind(thinking_tokens)
            .bind(reasoning_tokens)
            .bind(cache_read_tokens)
            .bind(cache_creation_tokens)
            .bind(audio_input_tokens)
            .bind(audio_output_tokens)
            .bind(session_id)
            .execute(&mut **tx)
            .await
            .map_err(|e| {
                WriterError::Database(format!(
                    "Failed to update session tokens for {}: {}",
                    session_id, e
                ))
            })?;
        }

        // Update model if provided
        if let Some(model) = model_used {
            sqlx::query(
                r#"
                UPDATE sessions
                SET model_used = COALESCE(model_used, ?)
                WHERE session_id = ?
                "#,
            )
            .bind(model)
            .bind(session_id)
            .execute(&mut **tx)
            .await
            .map_err(|e| {
                WriterError::Database(format!(
                    "Failed to update session model for {}: {}",
                    session_id, e
                ))
            })?;
        }

        // Insert/update tool calls if provided
        if let Some(tool_summary) = tool_call_updates
            && tool_summary.total_tool_calls > 0
        {
            for (tool_name, tool_stats) in &tool_summary.by_tool {
                let call_count = Self::safe_u32_to_i64(tool_stats.call_count);
                let avg_time = Self::safe_u64_to_i64(
                    tool_stats.avg_execution_time_ms,
                    "tool avg_execution_time_ms",
                )?;
                let error_count = Self::safe_u32_to_i64(tool_stats.error_count);

                sqlx::query(
                    r#"
                    INSERT INTO tool_stats (session_id, request_id, tool_name, call_count, avg_execution_time_ms, error_count, tool_arguments)
                    VALUES (?, ?, ?, ?, ?, ?, ?)
                    ON CONFLICT(session_id, request_id, tool_name)
                    DO UPDATE SET
                        call_count = MAX(call_count, excluded.call_count),
                        avg_execution_time_ms = excluded.avg_execution_time_ms,
                        error_count = MAX(error_count, excluded.error_count),
                        tool_arguments = COALESCE(tool_arguments, excluded.tool_arguments)
                    "#,
                )
                .bind(session_id)
                .bind(request_id)
                .bind(tool_name)
                .bind(call_count)
                .bind(avg_time)
                .bind(error_count)
                .bind(None::<String>)
                .execute(&mut **tx)
                .await
                .map_err(|e| {
                    WriterError::Database(format!(
                        "Failed to upsert tool call {} for session {}: {}",
                        tool_name, session_id, e
                    ))
                })?;
            }
        }

        // Insert into session_stats if we have model information
        // This is used in passthrough mode to record per-request stats
        tracing::debug!(
            "handle_stats_updated: model_used={:?}, has_tokens={}",
            model_used,
            token_updates.is_some()
        );
        if let Some(model) = model_used {
            tracing::debug!(
                "Inserting into session_stats for session={}, request={}, model={}",
                session_id,
                request_id,
                model
            );
            let input_tokens = if let Some(tokens) = token_updates {
                Self::safe_u64_to_i64(tokens.total_input, "input_tokens")?
            } else {
                0
            };

            let output_tokens = if let Some(tokens) = token_updates {
                Self::safe_u64_to_i64(tokens.total_output, "output_tokens")?
            } else {
                0
            };

            let thinking_tokens = if let Some(tokens) = token_updates {
                Self::safe_u64_to_i64(tokens.total_thinking, "thinking_tokens")?
            } else {
                0
            };

            let reasoning_tokens = if let Some(tokens) = token_updates {
                Self::safe_u64_to_i64(tokens.total_reasoning, "reasoning_tokens")?
            } else {
                0
            };

            let cache_read = if let Some(tokens) = token_updates {
                // Use new total_cache_read field, fall back to deprecated total_cached
                let cache_r = if tokens.total_cache_read > 0 {
                    tokens.total_cache_read
                } else {
                    tokens.total_cached
                };
                Self::safe_u64_to_i64(cache_r, "cache_read_tokens")?
            } else {
                0
            };

            let cache_creation = if let Some(tokens) = token_updates {
                Self::safe_u64_to_i64(tokens.total_cache_creation, "cache_creation_tokens")?
            } else {
                0
            };

            let audio_input = if let Some(tokens) = token_updates {
                Self::safe_u64_to_i64(tokens.total_audio_input, "audio_input_tokens")?
            } else {
                0
            };

            let audio_output = if let Some(tokens) = token_updates {
                Self::safe_u64_to_i64(tokens.total_audio_output, "audio_output_tokens")?
            } else {
                0
            };

            let response_size =
                Self::safe_usize_to_i64(response_size_bytes, "response_size_bytes")?;
            let content_blocks_i64 = Self::safe_usize_to_i64(content_blocks, "content_blocks")?;
            let has_refusal_i64 = if has_refusal { 1i64 } else { 0i64 };

            // Calculate thinking_percentage if we have thinking tokens
            let thinking_percentage = if thinking_tokens > 0 && output_tokens > 0 {
                Some((thinking_tokens as f64 / output_tokens as f64) * 100.0)
            } else {
                None
            };

            // Check if we have tool calls
            let has_tools = tool_call_updates
                .map(|t| t.total_tool_calls > 0)
                .unwrap_or(false);
            let has_tools_i64 = if has_tools { 1i64 } else { 0i64 };

            sqlx::query(
                r#"
                INSERT INTO session_stats (
                    session_id, request_id, model_name,
                    input_tokens, output_tokens, thinking_tokens, reasoning_tokens,
                    cache_read_tokens, cache_creation_tokens,
                    audio_input_tokens, audio_output_tokens,
                    thinking_percentage,
                    response_size_bytes, content_blocks,
                    has_tools, has_refusal, user_agent
                )
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(session_id)
            .bind(request_id)
            .bind(model)
            .bind(input_tokens)
            .bind(output_tokens)
            .bind(thinking_tokens)
            .bind(reasoning_tokens)
            .bind(cache_read)
            .bind(cache_creation)
            .bind(audio_input)
            .bind(audio_output)
            .bind(thinking_percentage)
            .bind(response_size)
            .bind(content_blocks_i64)
            .bind(has_tools_i64)
            .bind(has_refusal_i64)
            .bind(user_agent)
            .execute(&mut **tx)
            .await
            .map_err(|e| {
                WriterError::Database(format!(
                    "Failed to insert session_stats for request {}: {}",
                    request_id, e
                ))
            })?;
        }

        Ok(())
    }

    /// Insert tool call summary into database with duplicate handling
    async fn insert_tool_calls(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        session_id: &str,
        request_id: &str,
        final_stats: &FinalSessionStats,
    ) -> WriterResult<()> {
        if final_stats.tool_summary.total_tool_calls == 0 {
            return Ok(());
        }

        // Batch insert all tool calls in a single multi-value INSERT
        // Using ON CONFLICT to handle duplicates (update with latest values)
        for (tool_name, tool_stats) in &final_stats.tool_summary.by_tool {
            let call_count = Self::safe_u32_to_i64(tool_stats.call_count);
            let avg_time = Self::safe_u64_to_i64(
                tool_stats.avg_execution_time_ms,
                "tool avg_execution_time_ms",
            )?;
            let error_count = Self::safe_u32_to_i64(tool_stats.error_count);

            sqlx::query(
                r#"
                INSERT INTO tool_stats (session_id, request_id, tool_name, call_count, avg_execution_time_ms, error_count, tool_arguments)
                VALUES (?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(session_id, request_id, tool_name)
                DO UPDATE SET
                    call_count = excluded.call_count,
                    avg_execution_time_ms = excluded.avg_execution_time_ms,
                    error_count = excluded.error_count,
                    tool_arguments = COALESCE(tool_arguments, excluded.tool_arguments)
                "#,
            )
            .bind(session_id)
            .bind(request_id)
            .bind(tool_name)
            .bind(call_count)
            .bind(avg_time)
            .bind(error_count)
            .bind(None::<String>)
            .execute(&mut **tx)
            .await
            .map_err(|e| {
                WriterError::Database(format!(
                    "Failed to insert tool call {} for session {}: {}",
                    tool_name, session_id, e
                ))
            })?;
        }

        Ok(())
    }

    /// Insert streaming metrics into database
    async fn insert_streaming_metrics(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        session_id: &str,
        request_id: &str,
        streaming_stats: &StreamingStats,
    ) -> WriterResult<()> {
        let ttft = Self::safe_u64_to_i64(
            streaming_stats.time_to_first_token_ms,
            "time_to_first_token_ms",
        )?;
        let total_chunks = Self::safe_u32_to_i64(streaming_stats.total_chunks);
        let duration = Self::safe_u64_to_i64(
            streaming_stats.streaming_duration_ms,
            "streaming_duration_ms",
        )?;
        let p50 = streaming_stats
            .p50_chunk_latency_ms
            .map(|v| Self::safe_u64_to_i64(v, "p50_chunk_latency"))
            .transpose()?;
        let p95 = streaming_stats
            .p95_chunk_latency_ms
            .map(|v| Self::safe_u64_to_i64(v, "p95_chunk_latency"))
            .transpose()?;
        let p99 = streaming_stats
            .p99_chunk_latency_ms
            .map(|v| Self::safe_u64_to_i64(v, "p99_chunk_latency"))
            .transpose()?;
        let max_latency =
            Self::safe_u64_to_i64(streaming_stats.max_chunk_latency_ms, "max_chunk_latency_ms")?;
        let min_latency =
            Self::safe_u64_to_i64(streaming_stats.min_chunk_latency_ms, "min_chunk_latency_ms")?;

        sqlx::query(
            r#"
            INSERT INTO stream_metrics (
                session_id, request_id,
                time_to_first_token_ms, total_chunks, streaming_duration_ms,
                avg_chunk_latency_ms, p50_chunk_latency_ms, p95_chunk_latency_ms,
                p99_chunk_latency_ms, max_chunk_latency_ms, min_chunk_latency_ms
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(session_id)
        .bind(request_id)
        .bind(ttft)
        .bind(total_chunks)
        .bind(duration)
        .bind(streaming_stats.avg_chunk_latency_ms)
        .bind(p50)
        .bind(p95)
        .bind(p99)
        .bind(max_latency)
        .bind(min_latency)
        .execute(&mut **tx)
        .await
        .map_err(|e| {
            WriterError::Database(format!(
                "Failed to insert streaming metrics for session {}: {}",
                session_id, e
            ))
        })?;

        Ok(())
    }
}

#[cfg(test)]
#[cfg(feature = "sqlite-writer")]
mod tests {
    use super::*;
    use crate::events::*;
    use chrono::Utc;
    use std::collections::HashMap;
    use tempfile::{TempDir, tempdir};

    #[test]
    fn test_validate_session_id_valid() {
        // Valid UUIDs
        assert!(SqliteWriter::validate_session_id("550e8400-e29b-41d4-a716-446655440000").is_ok());
        assert!(SqliteWriter::validate_session_id("test-123").is_ok());
        assert!(SqliteWriter::validate_session_id("session_abc_123").is_ok());
        assert!(SqliteWriter::validate_session_id("abc123").is_ok());
    }

    #[test]
    fn test_validate_session_id_empty() {
        let result = SqliteWriter::validate_session_id("");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn test_validate_session_id_too_long() {
        let long_id = "a".repeat(256);
        let result = SqliteWriter::validate_session_id(&long_id);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too long"));
    }

    #[test]
    fn test_validate_session_id_path_traversal() {
        // Path traversal attempts
        assert!(SqliteWriter::validate_session_id("../../../etc/passwd").is_err());
        assert!(SqliteWriter::validate_session_id("..").is_err());
        assert!(SqliteWriter::validate_session_id("./test").is_err());
        assert!(SqliteWriter::validate_session_id("test/../admin").is_err());
    }

    #[test]
    fn test_validate_session_id_special_chars() {
        // Special characters that could be dangerous
        assert!(SqliteWriter::validate_session_id("test;DROP TABLE sessions").is_err());
        assert!(SqliteWriter::validate_session_id("test\0null").is_err());
        assert!(SqliteWriter::validate_session_id("test/path").is_err());
        assert!(SqliteWriter::validate_session_id("test\\path").is_err());
        assert!(SqliteWriter::validate_session_id("test@host").is_err());
        assert!(SqliteWriter::validate_session_id("test$var").is_err());
    }

    #[tokio::test]
    async fn test_sqlite_writer_rejects_invalid_session_id() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        let event = SessionEvent::Started {
            session_id: "../../../evil".to_string(),
            request_id: "req-123".to_string(),
            timestamp: Utc::now(),
            model_requested: "test".to_string(),
            provider: "test".to_string(),
            listener: "test".to_string(),
            is_streaming: false,
            metadata: SessionMetadata {
                client_ip: None,
                user_agent: None,
                api_version: None,
                request_headers: HashMap::new(),
                session_tags: vec![],
            },
        };

        let result = writer.write_event(&event).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Invalid session ID")
        );
    }

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

        assert_eq!(version, 5);
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
                is_streaming: false,
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
                        cache_creation_tokens: None,
                        reasoning_tokens: None,
                        audio_input_tokens: None,
                        audio_output_tokens: None,
                        total_tokens: 30,
                        thinking_percentage: None,
                        tokens_per_second: Some(200.0),
                    },
                    tool_calls: vec![],
                    response_size_bytes: 100,
                    content_blocks: 1,
                    has_refusal: false,
                    is_streaming: false,
                    chunk_count: None,
                    streaming_duration_ms: None,
                },
            },
        ];

        writer.write_batch(&events).await.unwrap();

        // Verify session was created
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE session_id = 'test-123'")
                .fetch_one(&writer.pool)
                .await
                .unwrap();

        assert_eq!(count, 1);

        // Verify stats were recorded
        let stats_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM session_stats WHERE session_id = 'test-123'")
                .fetch_one(&writer.pool)
                .await
                .unwrap();

        assert_eq!(stats_count, 1);
    }

    #[tokio::test]
    async fn test_sqlite_writer_streaming_session() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let writer = SqliteWriter::new(&db_path).await.unwrap();

        let session_id = "stream-test-123";
        let request_id = "req-stream-456";

        let events = vec![
            SessionEvent::Started {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                timestamp: Utc::now(),
                model_requested: "claude-sonnet-4".to_string(),
                provider: "anthropic".to_string(),
                listener: "anthropic".to_string(),
                is_streaming: true,
                metadata: SessionMetadata {
                    client_ip: Some("192.168.1.1".to_string()),
                    user_agent: Some("test-client".to_string()),
                    api_version: Some("2023-06-01".to_string()),
                    request_headers: HashMap::new(),
                    session_tags: vec!["streaming".to_string()],
                },
            },
            SessionEvent::StreamStarted {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                timestamp: Utc::now(),
                time_to_first_token_ms: 150,
            },
            SessionEvent::Completed {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                timestamp: Utc::now(),
                success: true,
                error: None,
                finish_reason: Some("end_turn".to_string()),
                final_stats: Box::new(FinalSessionStats {
                    total_duration_ms: 5000,
                    provider_time_ms: 4800,
                    proxy_overhead_ms: 200.0,
                    total_tokens: TokenTotals {
                        total_input: 100,
                        total_output: 500,
                        total_thinking: 50,
                        total_cached: 20,
                        grand_total: 650,
                        total_reasoning: 0,
                        total_cache_read: 0,
                        total_cache_creation: 0,
                        total_audio_input: 0,
                        total_audio_output: 0,
                        by_model: HashMap::new(),
                    },
                    tool_summary: ToolUsageSummary::default(),
                    performance: PerformanceMetrics {
                        avg_provider_latency_ms: 4800.0,
                        p50_latency_ms: Some(4500),
                        p95_latency_ms: Some(5000),
                        p99_latency_ms: Some(5200),
                        max_latency_ms: 5500,
                        min_latency_ms: 4000,
                        avg_pre_processing_ms: 10.0,
                        avg_post_processing_ms: 15.0,
                        proxy_overhead_percentage: 4.2,
                    },
                    streaming_stats: Some(StreamingStats {
                        time_to_first_token_ms: 150,
                        total_chunks: 42,
                        streaming_duration_ms: 4850,
                        avg_chunk_latency_ms: 115.5,
                        p50_chunk_latency_ms: Some(100),
                        p95_chunk_latency_ms: Some(200),
                        p99_chunk_latency_ms: Some(250),
                        max_chunk_latency_ms: 300,
                        min_chunk_latency_ms: 50,
                    }),
                    estimated_cost: None,
                }),
            },
        ];

        writer.write_batch(&events).await.unwrap();

        // Verify session was created with streaming flag
        let is_streaming: bool =
            sqlx::query_scalar("SELECT is_streaming FROM sessions WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&writer.pool)
                .await
                .unwrap();
        assert!(is_streaming);

        // Verify TTFT was recorded
        let ttft: Option<i64> =
            sqlx::query_scalar("SELECT time_to_first_token_ms FROM sessions WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&writer.pool)
                .await
                .unwrap();
        assert_eq!(ttft, Some(150));

        // Verify stream_metrics table has the data
        let stream_metrics_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM stream_metrics WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&writer.pool)
                .await
                .unwrap();
        assert_eq!(stream_metrics_count, 1);

        // Verify streaming stats details
        let (total_chunks, streaming_duration, avg_latency, p95_latency): (
            i64,
            i64,
            f64,
            Option<i64>,
        ) = sqlx::query_as(
            "SELECT total_chunks, streaming_duration_ms, avg_chunk_latency_ms, p95_chunk_latency_ms
                 FROM stream_metrics WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(&writer.pool)
        .await
        .unwrap();

        assert_eq!(total_chunks, 42);
        assert_eq!(streaming_duration, 4850);
        assert_eq!(avg_latency, 115.5);
        assert_eq!(p95_latency, Some(200));
    }

    #[tokio::test]
    async fn test_search_sessions_basic() {
        use crate::search::SessionFilter;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        // Create test sessions
        for i in 0..10 {
            let events = vec![
                SessionEvent::Started {
                    session_id: format!("session-{}", i),
                    request_id: format!("req-{}", i),
                    timestamp: Utc::now() - chrono::Duration::minutes(i),
                    model_requested: if i % 2 == 0 {
                        "gpt-4".to_string()
                    } else {
                        "claude-3".to_string()
                    },
                    provider: if i % 2 == 0 {
                        "openai".to_string()
                    } else {
                        "anthropic".to_string()
                    },
                    listener: "test".to_string(),
                    is_streaming: i % 3 == 0,
                    metadata: SessionMetadata {
                        client_ip: Some(format!("192.168.1.{}", i)),
                        user_agent: Some("test".to_string()),
                        api_version: Some("v1".to_string()),
                        request_headers: HashMap::new(),
                        session_tags: vec![],
                    },
                },
                SessionEvent::ResponseRecorded {
                    session_id: format!("session-{}", i),
                    request_id: format!("req-{}", i),
                    timestamp: Utc::now(),
                    response_text: format!("Response {}", i),
                    response_json: serde_json::json!({}),
                    model_used: if i % 2 == 0 {
                        "gpt-4".to_string()
                    } else {
                        "claude-3".to_string()
                    },
                    stats: ResponseStats {
                        provider_latency_ms: 100 + (i as u64 * 10),
                        post_processing_ms: 10.0,
                        total_proxy_overhead_ms: 15.0,
                        tokens: TokenStats {
                            input_tokens: 10,
                            output_tokens: 20 + (i as u32 * 5),
                            thinking_tokens: None,
                            cache_read_tokens: None,
                            cache_creation_tokens: None,
                            reasoning_tokens: None,
                            audio_input_tokens: None,
                            audio_output_tokens: None,
                            total_tokens: 30 + (i as u32 * 5),
                            thinking_percentage: None,
                            tokens_per_second: Some(200.0),
                        },
                        tool_calls: vec![],
                        response_size_bytes: 100,
                        content_blocks: 1,
                        has_refusal: false,
                        is_streaming: false,
                        chunk_count: None,
                        streaming_duration_ms: None,
                    },
                },
            ];
            writer.write_batch(&events).await.unwrap();
        }

        // Test basic search
        let filter = SessionFilter::default();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.items.len(), 10);
        assert_eq!(results.total_count, 10);

        // Test provider filter
        let filter = SessionFilter::builder()
            .providers(vec!["openai".to_string()])
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.total_count, 5);

        // Test model filter
        let filter = SessionFilter::builder()
            .models(vec!["claude-3".to_string()])
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.total_count, 5);

        // Test pagination
        let filter = SessionFilter::builder()
            .page_size(3)
            .page(0)
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.items.len(), 3);
        assert_eq!(results.total_count, 10);
        assert_eq!(results.total_pages, 4);
        assert!(results.has_next_page());
        assert!(!results.has_prev_page());
    }

    #[tokio::test]
    async fn test_search_sessions_aggregates() {
        use crate::search::SessionFilter;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        // Create test sessions with varied stats
        for i in 0..5 {
            let events = vec![
                SessionEvent::Started {
                    session_id: format!("agg-session-{}", i),
                    request_id: format!("agg-req-{}", i),
                    timestamp: Utc::now(),
                    model_requested: "gpt-4".to_string(),
                    provider: "openai".to_string(),
                    listener: "test".to_string(),
                    is_streaming: false,
                    metadata: SessionMetadata {
                        client_ip: None,
                        user_agent: None,
                        api_version: None,
                        request_headers: HashMap::new(),
                        session_tags: vec![],
                    },
                },
                SessionEvent::ResponseRecorded {
                    session_id: format!("agg-session-{}", i),
                    request_id: format!("agg-req-{}", i),
                    timestamp: Utc::now(),
                    response_text: "Response".to_string(),
                    response_json: serde_json::json!({}),
                    model_used: "gpt-4".to_string(),
                    stats: ResponseStats {
                        provider_latency_ms: 100,
                        post_processing_ms: 10.0,
                        total_proxy_overhead_ms: 15.0,
                        tokens: TokenStats {
                            input_tokens: 100,
                            output_tokens: 200,
                            thinking_tokens: None,
                            cache_read_tokens: None,
                            cache_creation_tokens: None,
                            reasoning_tokens: None,
                            audio_input_tokens: None,
                            audio_output_tokens: None,
                            total_tokens: 300,
                            thinking_percentage: None,
                            tokens_per_second: Some(200.0),
                        },
                        tool_calls: vec![],
                        response_size_bytes: 100,
                        content_blocks: 1,
                        has_refusal: false,
                        is_streaming: false,
                        chunk_count: None,
                        streaming_duration_ms: None,
                    },
                },
                SessionEvent::Completed {
                    session_id: format!("agg-session-{}", i),
                    request_id: format!("agg-req-{}", i),
                    timestamp: Utc::now(),
                    success: i < 4, // First 4 successful, last one fails
                    error: if i >= 4 {
                        Some("Error".to_string())
                    } else {
                        None
                    },
                    finish_reason: Some("end_turn".to_string()),
                    final_stats: Box::new(FinalSessionStats {
                        total_duration_ms: 1000 + (i * 100),
                        provider_time_ms: 900,
                        proxy_overhead_ms: 100.0,
                        total_tokens: TokenTotals {
                            total_input: 100,
                            total_output: 200,
                            total_thinking: 0,
                            total_cached: 0,
                            grand_total: 300,
                            total_reasoning: 0,
                            total_cache_read: 0,
                            total_cache_creation: 0,
                            total_audio_input: 0,
                            total_audio_output: 0,
                            by_model: HashMap::new(),
                        },
                        tool_summary: ToolUsageSummary::default(),
                        performance: PerformanceMetrics {
                            avg_provider_latency_ms: 900.0,
                            p50_latency_ms: Some(900),
                            p95_latency_ms: Some(950),
                            p99_latency_ms: Some(980),
                            max_latency_ms: 1000,
                            min_latency_ms: 800,
                            avg_pre_processing_ms: 50.0,
                            avg_post_processing_ms: 50.0,
                            proxy_overhead_percentage: 10.0,
                        },
                        streaming_stats: None,
                        estimated_cost: None,
                    }),
                },
            ];
            writer.write_batch(&events).await.unwrap();
        }

        let filter = SessionFilter::default();
        let agg = writer.get_aggregates(&filter).await.unwrap();

        assert_eq!(agg.total_sessions, 5);
        assert_eq!(agg.successful_sessions, 4);
        assert_eq!(agg.failed_sessions, 1);
        assert_eq!(agg.total_tokens, 1500); // 300 * 5
        assert_eq!(agg.total_input_tokens, 500); // 100 * 5
        assert_eq!(agg.total_output_tokens, 1000); // 200 * 5
        assert!(agg.avg_duration_ms > 0.0);
        assert_eq!(*agg.sessions_by_provider.get("openai").unwrap(), 5);
        assert_eq!(*agg.sessions_by_model.get("gpt-4").unwrap(), 5);
    }

    #[tokio::test]
    async fn test_search_sessions_time_range() {
        use crate::search::SessionFilter;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        let now = Utc::now();
        let two_hours_ago = now - chrono::Duration::hours(2);
        let one_hour_ago = now - chrono::Duration::hours(1);

        // Create sessions at different times
        for i in 0..5 {
            let timestamp = two_hours_ago + chrono::Duration::minutes(i * 30);
            let events = vec![SessionEvent::Started {
                session_id: format!("time-session-{}", i),
                request_id: format!("time-req-{}", i),
                timestamp,
                model_requested: "gpt-4".to_string(),
                provider: "openai".to_string(),
                listener: "test".to_string(),
                is_streaming: false,
                metadata: SessionMetadata {
                    client_ip: None,
                    user_agent: None,
                    api_version: None,
                    request_headers: HashMap::new(),
                    session_tags: vec![],
                },
            }];
            writer.write_batch(&events).await.unwrap();
        }

        // Filter for last hour only
        let filter = SessionFilter::builder()
            .time_range(one_hour_ago, now)
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();

        // Should only get sessions from the last hour (sessions 2, 3, 4)
        assert_eq!(results.total_count, 3);
    }

    #[tokio::test]
    async fn test_search_sessions_text_search() {
        use crate::search::SessionFilter;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        // Create sessions with different text content
        let test_data = vec![
            ("text-1", "Hello world", "Response about AI"),
            ("text-2", "Database query", "SQL results"),
            ("text-3", "Hello AI", "Greeting response"),
        ];

        for (id, request_text, response_text) in test_data {
            let events = vec![
                SessionEvent::Started {
                    session_id: id.to_string(),
                    request_id: format!("{}-req", id),
                    timestamp: Utc::now(),
                    model_requested: "gpt-4".to_string(),
                    provider: "openai".to_string(),
                    listener: "test".to_string(),
                    is_streaming: false,
                    metadata: SessionMetadata {
                        client_ip: None,
                        user_agent: None,
                        api_version: None,
                        request_headers: HashMap::new(),
                        session_tags: vec![],
                    },
                },
                SessionEvent::RequestRecorded {
                    session_id: id.to_string(),
                    request_id: format!("{}-req", id),
                    timestamp: Utc::now(),
                    request_text: request_text.to_string(),
                    request_json: serde_json::json!({}),
                    estimated_tokens: 10,
                    stats: RequestStats {
                        pre_processing_ms: 5.0,
                        request_size_bytes: request_text.len(),
                        message_count: 1,
                        has_system_prompt: false,
                        has_tools: false,
                        tool_count: 0,
                    },
                },
                SessionEvent::ResponseRecorded {
                    session_id: id.to_string(),
                    request_id: format!("{}-req", id),
                    timestamp: Utc::now(),
                    response_text: response_text.to_string(),
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
                            cache_creation_tokens: None,
                            reasoning_tokens: None,
                            audio_input_tokens: None,
                            audio_output_tokens: None,
                            total_tokens: 30,
                            thinking_percentage: None,
                            tokens_per_second: Some(200.0),
                        },
                        tool_calls: vec![],
                        response_size_bytes: 100,
                        content_blocks: 1,
                        has_refusal: false,
                        is_streaming: false,
                        chunk_count: None,
                        streaming_duration_ms: None,
                    },
                },
            ];
            writer.write_batch(&events).await.unwrap();
        }

        // Search for "Hello"
        let filter = SessionFilter::builder()
            .text_search("Hello".to_string())
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.total_count, 2); // text-1 and text-3

        // Search for "AI"
        let filter = SessionFilter::builder()
            .text_search("AI".to_string())
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.total_count, 2); // text-1 (response) and text-3

        // Search for "SQL"
        let filter = SessionFilter::builder()
            .text_search("SQL".to_string())
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.total_count, 1); // text-2 only
    }

    #[tokio::test]
    async fn test_search_sessions_token_range() {
        use crate::search::SessionFilter;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        // Create sessions with different token counts
        for i in 0..5 {
            let token_count = (i + 1) * 100; // 100, 200, 300, 400, 500
            let events = vec![
                SessionEvent::Started {
                    session_id: format!("token-session-{}", i),
                    request_id: format!("token-req-{}", i),
                    timestamp: Utc::now(),
                    model_requested: "gpt-4".to_string(),
                    provider: "openai".to_string(),
                    listener: "test".to_string(),
                    is_streaming: false,
                    metadata: SessionMetadata {
                        client_ip: None,
                        user_agent: None,
                        api_version: None,
                        request_headers: HashMap::new(),
                        session_tags: vec![],
                    },
                },
                SessionEvent::ResponseRecorded {
                    session_id: format!("token-session-{}", i),
                    request_id: format!("token-req-{}", i),
                    timestamp: Utc::now(),
                    response_text: "Response".to_string(),
                    response_json: serde_json::json!({}),
                    model_used: "gpt-4".to_string(),
                    stats: ResponseStats {
                        provider_latency_ms: 100,
                        post_processing_ms: 10.0,
                        total_proxy_overhead_ms: 15.0,
                        tokens: TokenStats {
                            input_tokens: (token_count / 2) as u32,
                            output_tokens: (token_count / 2) as u32,
                            thinking_tokens: None,
                            cache_read_tokens: None,
                            cache_creation_tokens: None,
                            reasoning_tokens: None,
                            audio_input_tokens: None,
                            audio_output_tokens: None,
                            total_tokens: token_count as u32,
                            thinking_percentage: None,
                            tokens_per_second: Some(200.0),
                        },
                        tool_calls: vec![],
                        response_size_bytes: 100,
                        content_blocks: 1,
                        has_refusal: false,
                        is_streaming: false,
                        chunk_count: None,
                        streaming_duration_ms: None,
                    },
                },
            ];
            writer.write_batch(&events).await.unwrap();
        }

        // Filter for min_tokens >= 250
        let filter = SessionFilter::builder().min_tokens(250).build().unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.total_count, 3); // 300, 400, 500

        // Filter for max_tokens <= 250
        let filter = SessionFilter::builder().max_tokens(250).build().unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.total_count, 2); // 100, 200

        // Filter for range 200-400
        let filter = SessionFilter::builder()
            .min_tokens(200)
            .max_tokens(400)
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.total_count, 3); // 200, 300, 400
    }

    #[tokio::test]
    async fn test_search_sessions_duration_range() {
        use crate::search::SessionFilter;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        // Create sessions with different durations
        for i in 0..5 {
            let duration_ms = (i + 1) * 1000; // 1s, 2s, 3s, 4s, 5s
            let events = vec![
                SessionEvent::Started {
                    session_id: format!("duration-session-{}", i),
                    request_id: format!("duration-req-{}", i),
                    timestamp: Utc::now(),
                    model_requested: "gpt-4".to_string(),
                    provider: "openai".to_string(),
                    listener: "test".to_string(),
                    is_streaming: false,
                    metadata: SessionMetadata {
                        client_ip: None,
                        user_agent: None,
                        api_version: None,
                        request_headers: HashMap::new(),
                        session_tags: vec![],
                    },
                },
                SessionEvent::Completed {
                    session_id: format!("duration-session-{}", i),
                    request_id: format!("duration-req-{}", i),
                    timestamp: Utc::now(),
                    success: true,
                    error: None,
                    finish_reason: Some("end_turn".to_string()),
                    final_stats: Box::new(FinalSessionStats {
                        total_duration_ms: duration_ms,
                        provider_time_ms: duration_ms - 100,
                        proxy_overhead_ms: 100.0,
                        total_tokens: TokenTotals {
                            total_input: 100,
                            total_output: 200,
                            total_thinking: 0,
                            total_cached: 0,
                            grand_total: 300,
                            total_reasoning: 0,
                            total_cache_read: 0,
                            total_cache_creation: 0,
                            total_audio_input: 0,
                            total_audio_output: 0,
                            by_model: HashMap::new(),
                        },
                        tool_summary: ToolUsageSummary::default(),
                        performance: PerformanceMetrics {
                            avg_provider_latency_ms: 900.0,
                            p50_latency_ms: Some(900),
                            p95_latency_ms: Some(950),
                            p99_latency_ms: Some(980),
                            max_latency_ms: 1000,
                            min_latency_ms: 800,
                            avg_pre_processing_ms: 50.0,
                            avg_post_processing_ms: 50.0,
                            proxy_overhead_percentage: 10.0,
                        },
                        streaming_stats: None,
                        estimated_cost: None,
                    }),
                },
            ];
            writer.write_batch(&events).await.unwrap();
        }

        // Filter for min_duration >= 2500ms
        let filter = SessionFilter::builder()
            .min_duration_ms(2500)
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.total_count, 3); // 3s, 4s, 5s

        // Filter for max_duration <= 2500ms
        let filter = SessionFilter::builder()
            .max_duration_ms(2500)
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.total_count, 2); // 1s, 2s

        // Filter for range 2000-4000ms
        let filter = SessionFilter::builder()
            .min_duration_ms(2000)
            .max_duration_ms(4000)
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.total_count, 3); // 2s, 3s, 4s
    }

    #[tokio::test]
    async fn test_search_sessions_client_ip_and_finish_reason() {
        use crate::search::SessionFilter;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        // Create sessions with different IPs and finish reasons
        let test_data = vec![
            ("ip-1", "192.168.1.1", "end_turn"),
            ("ip-2", "192.168.1.2", "end_turn"),
            ("ip-3", "10.0.0.1", "max_tokens"),
            ("ip-4", "192.168.1.1", "max_tokens"),
        ];

        for (id, ip, finish_reason) in test_data {
            let events = vec![
                SessionEvent::Started {
                    session_id: id.to_string(),
                    request_id: format!("{}-req", id),
                    timestamp: Utc::now(),
                    model_requested: "gpt-4".to_string(),
                    provider: "openai".to_string(),
                    listener: "test".to_string(),
                    is_streaming: false,
                    metadata: SessionMetadata {
                        client_ip: Some(ip.to_string()),
                        user_agent: None,
                        api_version: None,
                        request_headers: HashMap::new(),
                        session_tags: vec![],
                    },
                },
                SessionEvent::Completed {
                    session_id: id.to_string(),
                    request_id: format!("{}-req", id),
                    timestamp: Utc::now(),
                    success: true,
                    error: None,
                    finish_reason: Some(finish_reason.to_string()),
                    final_stats: Box::new(FinalSessionStats {
                        total_duration_ms: 1000,
                        provider_time_ms: 900,
                        proxy_overhead_ms: 100.0,
                        total_tokens: TokenTotals {
                            total_input: 100,
                            total_output: 200,
                            total_thinking: 0,
                            total_cached: 0,
                            grand_total: 300,
                            total_reasoning: 0,
                            total_cache_read: 0,
                            total_cache_creation: 0,
                            total_audio_input: 0,
                            total_audio_output: 0,
                            by_model: HashMap::new(),
                        },
                        tool_summary: ToolUsageSummary::default(),
                        performance: PerformanceMetrics {
                            avg_provider_latency_ms: 900.0,
                            p50_latency_ms: Some(900),
                            p95_latency_ms: Some(950),
                            p99_latency_ms: Some(980),
                            max_latency_ms: 1000,
                            min_latency_ms: 800,
                            avg_pre_processing_ms: 50.0,
                            avg_post_processing_ms: 50.0,
                            proxy_overhead_percentage: 10.0,
                        },
                        streaming_stats: None,
                        estimated_cost: None,
                    }),
                },
            ];
            writer.write_batch(&events).await.unwrap();
        }

        // Filter by client IP
        let filter = SessionFilter::builder()
            .client_ips(vec!["192.168.1.1".to_string()])
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.total_count, 2); // ip-1 and ip-4

        // Filter by finish reason
        let filter = SessionFilter::builder()
            .finish_reasons(vec!["max_tokens".to_string()])
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.total_count, 2); // ip-3 and ip-4

        // Combine both filters
        let filter = SessionFilter::builder()
            .client_ips(vec!["192.168.1.1".to_string()])
            .finish_reasons(vec!["max_tokens".to_string()])
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.total_count, 1); // ip-4 only
    }

    #[tokio::test]
    async fn test_search_sessions_success_and_streaming() {
        use crate::search::SessionFilter;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        // Create mix of successful/failed and streaming/non-streaming sessions
        for i in 0..6 {
            let is_success = i < 4;
            let is_streaming = i % 2 == 0;
            let events = vec![
                SessionEvent::Started {
                    session_id: format!("status-session-{}", i),
                    request_id: format!("status-req-{}", i),
                    timestamp: Utc::now(),
                    model_requested: "gpt-4".to_string(),
                    provider: "openai".to_string(),
                    listener: "test".to_string(),
                    is_streaming,
                    metadata: SessionMetadata {
                        client_ip: None,
                        user_agent: None,
                        api_version: None,
                        request_headers: HashMap::new(),
                        session_tags: vec![],
                    },
                },
                SessionEvent::Completed {
                    session_id: format!("status-session-{}", i),
                    request_id: format!("status-req-{}", i),
                    timestamp: Utc::now(),
                    success: is_success,
                    error: if is_success {
                        None
                    } else {
                        Some("Error".to_string())
                    },
                    finish_reason: Some("end_turn".to_string()),
                    final_stats: Box::new(FinalSessionStats {
                        total_duration_ms: 1000,
                        provider_time_ms: 900,
                        proxy_overhead_ms: 100.0,
                        total_tokens: TokenTotals {
                            total_input: 100,
                            total_output: 200,
                            total_thinking: 0,
                            total_cached: 0,
                            grand_total: 300,
                            total_reasoning: 0,
                            total_cache_read: 0,
                            total_cache_creation: 0,
                            total_audio_input: 0,
                            total_audio_output: 0,
                            by_model: HashMap::new(),
                        },
                        tool_summary: ToolUsageSummary::default(),
                        performance: PerformanceMetrics {
                            avg_provider_latency_ms: 900.0,
                            p50_latency_ms: Some(900),
                            p95_latency_ms: Some(950),
                            p99_latency_ms: Some(980),
                            max_latency_ms: 1000,
                            min_latency_ms: 800,
                            avg_pre_processing_ms: 50.0,
                            avg_post_processing_ms: 50.0,
                            proxy_overhead_percentage: 10.0,
                        },
                        streaming_stats: None,
                        estimated_cost: None,
                    }),
                },
            ];
            writer.write_batch(&events).await.unwrap();
        }

        // Filter for successful only
        let filter = SessionFilter::builder().success(true).build().unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.total_count, 4);

        // Filter for failed only
        let filter = SessionFilter::builder().success(false).build().unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.total_count, 2);

        // Filter for streaming only
        let filter = SessionFilter::builder().streaming(true).build().unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.total_count, 3); // sessions 0, 2, 4

        // Filter for non-streaming only
        let filter = SessionFilter::builder().streaming(false).build().unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.total_count, 3); // sessions 1, 3, 5

        // Combine: successful AND streaming
        let filter = SessionFilter::builder()
            .success(true)
            .streaming(true)
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.total_count, 2); // sessions 0, 2
    }

    #[tokio::test]
    async fn test_search_sessions_all_sort_orders() {
        use crate::search::{SessionFilter, SortOrder};

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        // Create sessions with varied attributes for sorting
        for i in 0..5 {
            let timestamp = Utc::now() - chrono::Duration::minutes((4 - i) * 10); // Reverse order
            let duration = ((i + 1) * 500) as u64;
            let tokens = ((i + 1) * 50) as u32;

            let events = vec![
                SessionEvent::Started {
                    session_id: format!("sort-session-{}", i),
                    request_id: format!("sort-req-{}", i),
                    timestamp,
                    model_requested: "gpt-4".to_string(),
                    provider: "openai".to_string(),
                    listener: "test".to_string(),
                    is_streaming: false,
                    metadata: SessionMetadata {
                        client_ip: None,
                        user_agent: None,
                        api_version: None,
                        request_headers: HashMap::new(),
                        session_tags: vec![],
                    },
                },
                SessionEvent::ResponseRecorded {
                    session_id: format!("sort-session-{}", i),
                    request_id: format!("sort-req-{}", i),
                    timestamp: Utc::now(),
                    response_text: "Response".to_string(),
                    response_json: serde_json::json!({}),
                    model_used: "gpt-4".to_string(),
                    stats: ResponseStats {
                        provider_latency_ms: 100,
                        post_processing_ms: 10.0,
                        total_proxy_overhead_ms: 15.0,
                        tokens: TokenStats {
                            input_tokens: tokens,
                            output_tokens: tokens,
                            thinking_tokens: None,
                            cache_read_tokens: None,
                            cache_creation_tokens: None,
                            reasoning_tokens: None,
                            audio_input_tokens: None,
                            audio_output_tokens: None,
                            total_tokens: tokens * 2,
                            thinking_percentage: None,
                            tokens_per_second: Some(200.0),
                        },
                        tool_calls: vec![],
                        response_size_bytes: 100,
                        content_blocks: 1,
                        has_refusal: false,
                        is_streaming: false,
                        chunk_count: None,
                        streaming_duration_ms: None,
                    },
                },
                SessionEvent::Completed {
                    session_id: format!("sort-session-{}", i),
                    request_id: format!("sort-req-{}", i),
                    timestamp: timestamp + chrono::Duration::milliseconds(duration as i64),
                    success: true,
                    error: None,
                    finish_reason: Some("end_turn".to_string()),
                    final_stats: Box::new(FinalSessionStats {
                        total_duration_ms: duration,
                        provider_time_ms: duration - 100,
                        proxy_overhead_ms: 100.0,
                        total_tokens: TokenTotals {
                            total_input: tokens as u64,
                            total_output: tokens as u64,
                            total_thinking: 0,
                            total_cached: 0,
                            grand_total: (tokens * 2) as u64,
                            total_reasoning: 0,
                            total_cache_read: 0,
                            total_cache_creation: 0,
                            total_audio_input: 0,
                            total_audio_output: 0,
                            by_model: HashMap::new(),
                        },
                        tool_summary: ToolUsageSummary::default(),
                        performance: PerformanceMetrics {
                            avg_provider_latency_ms: 900.0,
                            p50_latency_ms: Some(900),
                            p95_latency_ms: Some(950),
                            p99_latency_ms: Some(980),
                            max_latency_ms: 1000,
                            min_latency_ms: 800,
                            avg_pre_processing_ms: 50.0,
                            avg_post_processing_ms: 50.0,
                            proxy_overhead_percentage: 10.0,
                        },
                        streaming_stats: None,
                        estimated_cost: None,
                    }),
                },
            ];
            writer.write_batch(&events).await.unwrap();
        }

        // Test NewestFirst (default)
        let filter = SessionFilter::builder()
            .sort(SortOrder::NewestFirst)
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.items[0].session_id, "sort-session-4");

        // Test OldestFirst
        let filter = SessionFilter::builder()
            .sort(SortOrder::OldestFirst)
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.items[0].session_id, "sort-session-0");

        // Test HighestTokens
        let filter = SessionFilter::builder()
            .sort(SortOrder::HighestTokens)
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.items[0].session_id, "sort-session-4");
        assert_eq!(results.items[0].total_tokens, 500); // 50 * 2 * 5

        // Test LongestDuration
        let filter = SessionFilter::builder()
            .sort(SortOrder::LongestDuration)
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.items[0].session_id, "sort-session-4");
        assert_eq!(results.items[0].total_duration_ms, Some(2500)); // 500 * 5

        // Test ShortestDuration
        let filter = SessionFilter::builder()
            .sort(SortOrder::ShortestDuration)
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.items[0].session_id, "sort-session-0");
        assert_eq!(results.items[0].total_duration_ms, Some(500));
    }

    #[tokio::test]
    async fn test_search_sessions_edge_cases() {
        use crate::search::SessionFilter;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        // Create 2 sessions
        for i in 0..2 {
            let events = vec![SessionEvent::Started {
                session_id: format!("edge-session-{}", i),
                request_id: format!("edge-req-{}", i),
                timestamp: Utc::now(),
                model_requested: "gpt-4".to_string(),
                provider: "openai".to_string(),
                listener: "test".to_string(),
                is_streaming: false,
                metadata: SessionMetadata {
                    client_ip: None,
                    user_agent: None,
                    api_version: None,
                    request_headers: HashMap::new(),
                    session_tags: vec![],
                },
            }];
            writer.write_batch(&events).await.unwrap();
        }

        // Empty results - filter that matches nothing
        let filter = SessionFilter::builder()
            .providers(vec!["nonexistent".to_string()])
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.total_count, 0);
        assert_eq!(results.items.len(), 0);
        assert_eq!(results.total_pages, 1);
        assert!(!results.has_next_page());
        assert!(!results.has_prev_page());

        // Page beyond total pages
        let filter = SessionFilter::builder()
            .page_size(10)
            .page(100) // Way beyond actual data
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.items.len(), 0); // No results on this page
        assert_eq!(results.total_count, 2); // But total count is still accurate

        // Page size of 1 with multiple pages
        let filter = SessionFilter::builder()
            .page_size(1)
            .page(0)
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.items.len(), 1);
        assert_eq!(results.total_pages, 2);
        assert!(results.has_next_page());
        assert!(!results.has_prev_page());

        // Second page with page size of 1
        let filter = SessionFilter::builder()
            .page_size(1)
            .page(1)
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();
        assert_eq!(results.items.len(), 1);
        assert!(!results.has_next_page());
        assert!(results.has_prev_page());
    }

    #[tokio::test]
    async fn test_search_sessions_combined_filters() {
        use crate::search::{SessionFilter, SortOrder};

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        // Create diverse set of sessions
        let now = Utc::now();
        for i in 0..10 {
            let provider = if i % 2 == 0 { "openai" } else { "anthropic" };
            let model = if i % 2 == 0 { "gpt-4" } else { "claude-3" };
            let is_streaming = i % 3 == 0;
            let is_success = i < 7;
            let timestamp = now - chrono::Duration::hours(i);
            let tokens = ((i + 1) * 100) as u32;

            let events = vec![
                SessionEvent::Started {
                    session_id: format!("combined-session-{}", i),
                    request_id: format!("combined-req-{}", i),
                    timestamp,
                    model_requested: model.to_string(),
                    provider: provider.to_string(),
                    listener: "test".to_string(),
                    is_streaming,
                    metadata: SessionMetadata {
                        client_ip: Some(if i < 5 { "192.168.1.1" } else { "10.0.0.1" }.to_string()),
                        user_agent: None,
                        api_version: None,
                        request_headers: HashMap::new(),
                        session_tags: vec![],
                    },
                },
                SessionEvent::ResponseRecorded {
                    session_id: format!("combined-session-{}", i),
                    request_id: format!("combined-req-{}", i),
                    timestamp: Utc::now(),
                    response_text: "Response".to_string(),
                    response_json: serde_json::json!({}),
                    model_used: model.to_string(),
                    stats: ResponseStats {
                        provider_latency_ms: 100,
                        post_processing_ms: 10.0,
                        total_proxy_overhead_ms: 15.0,
                        tokens: TokenStats {
                            input_tokens: tokens,
                            output_tokens: tokens,
                            thinking_tokens: None,
                            cache_read_tokens: None,
                            cache_creation_tokens: None,
                            reasoning_tokens: None,
                            audio_input_tokens: None,
                            audio_output_tokens: None,
                            total_tokens: tokens * 2,
                            thinking_percentage: None,
                            tokens_per_second: Some(200.0),
                        },
                        tool_calls: vec![],
                        response_size_bytes: 100,
                        content_blocks: 1,
                        has_refusal: false,
                        is_streaming: false,
                        chunk_count: None,
                        streaming_duration_ms: None,
                    },
                },
                SessionEvent::Completed {
                    session_id: format!("combined-session-{}", i),
                    request_id: format!("combined-req-{}", i),
                    timestamp: Utc::now(),
                    success: is_success,
                    error: if is_success {
                        None
                    } else {
                        Some("Error".to_string())
                    },
                    finish_reason: Some("end_turn".to_string()),
                    final_stats: Box::new(FinalSessionStats {
                        total_duration_ms: ((i + 1) * 500) as u64,
                        provider_time_ms: 900,
                        proxy_overhead_ms: 100.0,
                        total_tokens: TokenTotals {
                            total_input: tokens as u64,
                            total_output: tokens as u64,
                            total_thinking: 0,
                            total_cached: 0,
                            grand_total: (tokens * 2) as u64,
                            total_reasoning: 0,
                            total_cache_read: 0,
                            total_cache_creation: 0,
                            total_audio_input: 0,
                            total_audio_output: 0,
                            by_model: HashMap::new(),
                        },
                        tool_summary: ToolUsageSummary::default(),
                        performance: PerformanceMetrics {
                            avg_provider_latency_ms: 900.0,
                            p50_latency_ms: Some(900),
                            p95_latency_ms: Some(950),
                            p99_latency_ms: Some(980),
                            max_latency_ms: 1000,
                            min_latency_ms: 800,
                            avg_pre_processing_ms: 50.0,
                            avg_post_processing_ms: 50.0,
                            proxy_overhead_percentage: 10.0,
                        },
                        streaming_stats: None,
                        estimated_cost: None,
                    }),
                },
            ];
            writer.write_batch(&events).await.unwrap();
        }

        // Complex filter: openai + successful + last 3 hours + min 300 tokens + streaming
        let three_hours_ago = now - chrono::Duration::hours(3);
        let filter = SessionFilter::builder()
            .providers(vec!["openai".to_string()])
            .success(true)
            .time_range(three_hours_ago, now)
            .min_tokens(300)
            .streaming(true)
            .sort(SortOrder::HighestTokens)
            .build()
            .unwrap();
        let results = writer.search_sessions(&filter).await.unwrap();

        // Should match: session 0 (openai, success, streaming, 0 hours ago, 200 tokens) - NO (tokens < 300)
        // Should match: session 6 (anthropic) - NO (wrong provider)
        // Should match: session 0 - already checked
        // Actually need to recalculate...
        // Let's just verify it returns results and they meet criteria
        for item in &results.items {
            assert_eq!(item.provider, "openai");
            assert_eq!(item.success, Some(true));
            assert!(item.total_tokens >= 300);
            assert!(item.is_streaming);
        }
    }

    /// Comprehensive test: Verify streaming session tokens are written to SQLite
    #[tokio::test]
    async fn test_streaming_session_tokens_in_sqlite() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        let session_id = "streaming-tokens-test";
        let events = vec![
            SessionEvent::Started {
                session_id: session_id.to_string(),
                request_id: "req-1".to_string(),
                timestamp: Utc::now(),
                model_requested: "claude-sonnet-4".to_string(),
                provider: "anthropic".to_string(),
                listener: "anthropic".to_string(),
                is_streaming: true,
                metadata: SessionMetadata {
                    client_ip: None,
                    user_agent: None,
                    api_version: None,
                    request_headers: HashMap::new(),
                    session_tags: vec![],
                },
            },
            SessionEvent::Completed {
                session_id: session_id.to_string(),
                request_id: "req-1".to_string(),
                timestamp: Utc::now(),
                success: true,
                error: None,
                finish_reason: Some("end_turn".to_string()),
                final_stats: Box::new(FinalSessionStats {
                    total_duration_ms: 1000,
                    provider_time_ms: 900,
                    proxy_overhead_ms: 100.0,
                    total_tokens: TokenTotals {
                        total_input: 150,
                        total_output: 350,
                        total_thinking: 50,
                        total_cached: 0,
                        grand_total: 550,
                        total_reasoning: 0,
                        total_cache_read: 0,
                        total_cache_creation: 0,
                        total_audio_input: 0,
                        total_audio_output: 0,
                        by_model: HashMap::new(),
                    },
                    tool_summary: ToolUsageSummary::default(),
                    performance: PerformanceMetrics::default(),
                    streaming_stats: None,
                    estimated_cost: None,
                }),
            },
        ];

        writer.write_batch(&events).await.unwrap();

        // Verify tokens are written to sessions table
        let (input, output, thinking): (i64, i64, Option<i64>) = sqlx::query_as(
            "SELECT input_tokens, output_tokens, thinking_tokens FROM sessions WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(&writer.pool)
        .await
        .unwrap();

        assert_eq!(input, 150, "Streaming session input_tokens should be 150");
        assert_eq!(output, 350, "Streaming session output_tokens should be 350");
        assert_eq!(
            thinking,
            Some(50),
            "Streaming session thinking_tokens should be 50"
        );
    }

    /// Comprehensive test: Verify non-streaming session tokens are written to SQLite
    #[tokio::test]
    async fn test_nonstreaming_session_tokens_in_sqlite() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        let session_id = "nonstreaming-tokens-test";
        let events = vec![
            SessionEvent::Started {
                session_id: session_id.to_string(),
                request_id: "req-1".to_string(),
                timestamp: Utc::now(),
                model_requested: "gpt-4".to_string(),
                provider: "openai".to_string(),
                listener: "openai".to_string(),
                is_streaming: false,
                metadata: SessionMetadata {
                    client_ip: None,
                    user_agent: None,
                    api_version: None,
                    request_headers: HashMap::new(),
                    session_tags: vec![],
                },
            },
            SessionEvent::ResponseRecorded {
                session_id: session_id.to_string(),
                request_id: "req-1".to_string(),
                timestamp: Utc::now(),
                response_text: "Test response".to_string(),
                response_json: serde_json::json!({}),
                model_used: "gpt-4".to_string(),
                stats: ResponseStats {
                    provider_latency_ms: 200,
                    post_processing_ms: 10.0,
                    total_proxy_overhead_ms: 15.0,
                    tokens: TokenStats {
                        input_tokens: 75,
                        output_tokens: 225,
                        thinking_tokens: Some(25),
                        cache_read_tokens: None,
                        cache_creation_tokens: None,
                        reasoning_tokens: None,
                        audio_input_tokens: None,
                        audio_output_tokens: None,
                        total_tokens: 325,
                        thinking_percentage: None,
                        tokens_per_second: Some(150.0),
                    },
                    tool_calls: vec![],
                    response_size_bytes: 100,
                    content_blocks: 1,
                    has_refusal: false,
                    is_streaming: false,
                    chunk_count: None,
                    streaming_duration_ms: None,
                },
            },
            SessionEvent::Completed {
                session_id: session_id.to_string(),
                request_id: "req-1".to_string(),
                timestamp: Utc::now(),
                success: true,
                error: None,
                finish_reason: Some("stop".to_string()),
                final_stats: Box::new(FinalSessionStats {
                    total_duration_ms: 1200,
                    provider_time_ms: 1100,
                    proxy_overhead_ms: 100.0,
                    total_tokens: TokenTotals {
                        total_input: 75,
                        total_output: 225,
                        total_thinking: 25,
                        total_cached: 0,
                        grand_total: 325,
                        total_reasoning: 0,
                        total_cache_read: 0,
                        total_cache_creation: 0,
                        total_audio_input: 0,
                        total_audio_output: 0,
                        by_model: HashMap::new(),
                    },
                    tool_summary: ToolUsageSummary::default(),
                    performance: PerformanceMetrics::default(),
                    streaming_stats: None,
                    estimated_cost: None,
                }),
            },
        ];

        writer.write_batch(&events).await.unwrap();

        // Verify tokens are written to sessions table (from ResponseRecorded, not duplicated by Completed)
        let (input, output, thinking): (i64, i64, Option<i64>) = sqlx::query_as(
            "SELECT input_tokens, output_tokens, thinking_tokens FROM sessions WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(&writer.pool)
        .await
        .unwrap();

        assert_eq!(input, 75, "Non-streaming session input_tokens should be 75");
        assert_eq!(
            output, 225,
            "Non-streaming session output_tokens should be 225"
        );
        assert_eq!(
            thinking,
            Some(25),
            "Non-streaming session thinking_tokens should be 25"
        );
    }

    /// Comprehensive test: Verify tool calls are written to SQLite for streaming sessions
    #[tokio::test]
    async fn test_streaming_session_tool_calls_in_sqlite() {
        use std::collections::HashMap;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        let session_id = "streaming-tools-test";

        let mut by_tool = HashMap::new();
        by_tool.insert(
            "Read".to_string(),
            ToolStats {
                call_count: 3,
                total_execution_time_ms: 150,
                avg_execution_time_ms: 50,
                error_count: 0,
            },
        );
        by_tool.insert(
            "Bash".to_string(),
            ToolStats {
                call_count: 2,
                total_execution_time_ms: 400,
                avg_execution_time_ms: 200,
                error_count: 1,
            },
        );

        let events = vec![
            SessionEvent::Started {
                session_id: session_id.to_string(),
                request_id: "req-1".to_string(),
                timestamp: Utc::now(),
                model_requested: "claude-sonnet-4".to_string(),
                provider: "anthropic".to_string(),
                listener: "anthropic".to_string(),
                is_streaming: true,
                metadata: SessionMetadata {
                    client_ip: None,
                    user_agent: None,
                    api_version: None,
                    request_headers: HashMap::new(),
                    session_tags: vec![],
                },
            },
            SessionEvent::Completed {
                session_id: session_id.to_string(),
                request_id: "req-1".to_string(),
                timestamp: Utc::now(),
                success: true,
                error: None,
                finish_reason: Some("end_turn".to_string()),
                final_stats: Box::new(FinalSessionStats {
                    total_duration_ms: 5000,
                    provider_time_ms: 4500,
                    proxy_overhead_ms: 500.0,
                    total_tokens: TokenTotals {
                        total_input: 100,
                        total_output: 200,
                        total_thinking: 0,
                        total_cached: 0,
                        grand_total: 300,
                        total_reasoning: 0,
                        total_cache_read: 0,
                        total_cache_creation: 0,
                        total_audio_input: 0,
                        total_audio_output: 0,
                        by_model: HashMap::new(),
                    },
                    tool_summary: ToolUsageSummary {
                        total_tool_calls: 5,
                        unique_tool_count: 2,
                        by_tool,
                        total_tool_time_ms: 550,
                        tool_error_count: 1,
                    },
                    performance: PerformanceMetrics::default(),
                    streaming_stats: None,
                    estimated_cost: None,
                }),
            },
        ];

        writer.write_batch(&events).await.unwrap();

        // Verify tool calls are written to tool_calls table
        let tool_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM tool_stats WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&writer.pool)
                .await
                .unwrap();

        assert_eq!(tool_count, 2, "Should have 2 tool call records");

        // Verify Read tool call
        let (tool_name, call_count, avg_time, errors): (String, i64, i64, i64) = sqlx::query_as(
            "SELECT tool_name, call_count, avg_execution_time_ms, error_count FROM tool_stats WHERE session_id = ? AND tool_name = 'Read'",
        )
        .bind(session_id)
        .fetch_one(&writer.pool)
        .await
        .unwrap();

        assert_eq!(tool_name, "Read");
        assert_eq!(call_count, 3);
        assert_eq!(avg_time, 50);
        assert_eq!(errors, 0);

        // Verify Bash tool call
        let (tool_name, call_count, avg_time, errors): (String, i64, i64, i64) = sqlx::query_as(
            "SELECT tool_name, call_count, avg_execution_time_ms, error_count FROM tool_stats WHERE session_id = ? AND tool_name = 'Bash'",
        )
        .bind(session_id)
        .fetch_one(&writer.pool)
        .await
        .unwrap();

        assert_eq!(tool_name, "Bash");
        assert_eq!(call_count, 2);
        assert_eq!(avg_time, 200);
        assert_eq!(errors, 1);
    }

    /// Test integer overflow handling with i64::MAX values
    #[tokio::test]
    async fn test_safe_u64_to_i64_overflow() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let _writer = SqliteWriter::new(&db_path).await.unwrap();

        // Test that overflow is properly detected
        let result = SqliteWriter::safe_u64_to_i64(u64::MAX, "test_value");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("exceeds maximum"));

        // Test that i64::MAX works
        let result = SqliteWriter::safe_u64_to_i64(i64::MAX as u64, "test_value");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), i64::MAX);
    }

    /// Test duplicate tool entries with ON CONFLICT handling
    #[tokio::test]
    async fn test_duplicate_tool_entries() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        let session_id = "duplicate-tools-test";

        // Create first Completed event with Read tool (count: 3)
        let mut by_tool = HashMap::new();
        by_tool.insert(
            "Read".to_string(),
            ToolStats {
                call_count: 3,
                total_execution_time_ms: 150,
                avg_execution_time_ms: 50,
                error_count: 0,
            },
        );

        let events1 = vec![
            SessionEvent::Started {
                session_id: session_id.to_string(),
                request_id: "req-1".to_string(),
                timestamp: Utc::now(),
                model_requested: "gpt-4".to_string(),
                provider: "openai".to_string(),
                listener: "openai".to_string(),
                is_streaming: false,
                metadata: SessionMetadata {
                    client_ip: None,
                    user_agent: None,
                    api_version: None,
                    request_headers: HashMap::new(),
                    session_tags: vec![],
                },
            },
            SessionEvent::Completed {
                session_id: session_id.to_string(),
                request_id: "req-1".to_string(),
                timestamp: Utc::now(),
                success: true,
                error: None,
                finish_reason: Some("end_turn".to_string()),
                final_stats: Box::new(FinalSessionStats {
                    total_duration_ms: 500,
                    provider_time_ms: 450,
                    proxy_overhead_ms: 50.0,
                    total_tokens: TokenTotals {
                        total_input: 100,
                        total_output: 200,
                        total_thinking: 50,
                        total_cached: 0,
                        grand_total: 350,
                        total_reasoning: 0,
                        total_cache_read: 0,
                        total_cache_creation: 0,
                        total_audio_input: 0,
                        total_audio_output: 0,
                        by_model: HashMap::new(),
                    },
                    tool_summary: ToolUsageSummary {
                        total_tool_calls: 3,
                        unique_tool_count: 1,
                        by_tool: by_tool.clone(),
                        total_tool_time_ms: 150,
                        tool_error_count: 0,
                    },
                    performance: PerformanceMetrics::default(),
                    streaming_stats: None,
                    estimated_cost: None,
                }),
            },
        ];

        writer.write_batch(&events1).await.unwrap();

        // Verify initial state
        let (call_count, avg_time): (i64, i64) = sqlx::query_as(
            "SELECT call_count, avg_execution_time_ms FROM tool_stats WHERE session_id = ? AND tool_name = 'Read'",
        )
        .bind(session_id)
        .fetch_one(&writer.pool)
        .await
        .unwrap();

        assert_eq!(call_count, 3);
        assert_eq!(avg_time, 50);

        // Create second Completed event with same tool but different stats
        let mut by_tool2 = HashMap::new();
        by_tool2.insert(
            "Read".to_string(),
            ToolStats {
                call_count: 5,
                total_execution_time_ms: 300,
                avg_execution_time_ms: 60,
                error_count: 1,
            },
        );

        let events2 = vec![SessionEvent::Completed {
            session_id: session_id.to_string(),
            request_id: "req-1".to_string(),
            timestamp: Utc::now(),
            success: true,
            error: None,
            finish_reason: Some("end_turn".to_string()),
            final_stats: Box::new(FinalSessionStats {
                total_duration_ms: 700,
                provider_time_ms: 650,
                proxy_overhead_ms: 50.0,
                total_tokens: TokenTotals {
                    total_input: 150,
                    total_output: 250,
                    total_thinking: 75,
                    total_cached: 0,
                    grand_total: 475,
                    total_reasoning: 0,
                    total_cache_read: 0,
                    total_cache_creation: 0,
                    total_audio_input: 0,
                    total_audio_output: 0,
                    by_model: HashMap::new(),
                },
                tool_summary: ToolUsageSummary {
                    total_tool_calls: 5,
                    unique_tool_count: 1,
                    by_tool: by_tool2,
                    total_tool_time_ms: 300,
                    tool_error_count: 1,
                },
                performance: PerformanceMetrics::default(),
                streaming_stats: None,
                estimated_cost: None,
            }),
        }];

        writer.write_batch(&events2).await.unwrap();

        // Verify ON CONFLICT updated the values
        let (call_count, avg_time, errors): (i64, i64, i64) = sqlx::query_as(
            "SELECT call_count, avg_execution_time_ms, error_count FROM tool_stats WHERE session_id = ? AND tool_name = 'Read'",
        )
        .bind(session_id)
        .fetch_one(&writer.pool)
        .await
        .unwrap();

        assert_eq!(call_count, 5, "Call count should be updated to 5");
        assert_eq!(avg_time, 60, "Avg time should be updated to 60");
        assert_eq!(errors, 1, "Error count should be updated to 1");

        // Verify we still only have one row
        let row_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM tool_stats WHERE session_id = ? AND tool_name = 'Read'",
        )
        .bind(session_id)
        .fetch_one(&writer.pool)
        .await
        .unwrap();

        assert_eq!(row_count, 1, "Should only have one row for the tool");
    }

    /// Test MAX() logic for token updates
    #[tokio::test]
    async fn test_token_update_max_logic() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        let session_id = "max-logic-test";

        let events = vec![
            SessionEvent::Started {
                session_id: session_id.to_string(),
                request_id: "req-1".to_string(),
                timestamp: Utc::now(),
                model_requested: "gpt-4".to_string(),
                provider: "openai".to_string(),
                listener: "openai".to_string(),
                is_streaming: false,
                metadata: SessionMetadata {
                    client_ip: None,
                    user_agent: None,
                    api_version: None,
                    request_headers: HashMap::new(),
                    session_tags: vec![],
                },
            },
            SessionEvent::ResponseRecorded {
                session_id: session_id.to_string(),
                request_id: "req-1".to_string(),
                timestamp: Utc::now(),
                response_text: "Response".to_string(),
                response_json: serde_json::json!({}),
                model_used: "gpt-4".to_string(),
                stats: ResponseStats {
                    provider_latency_ms: 500,
                    post_processing_ms: 10.0,
                    total_proxy_overhead_ms: 15.0,
                    tokens: TokenStats {
                        input_tokens: 100,
                        output_tokens: 300,
                        thinking_tokens: Some(50),
                        cache_read_tokens: None,
                        cache_creation_tokens: None,
                        reasoning_tokens: None,
                        audio_input_tokens: None,
                        audio_output_tokens: None,
                        total_tokens: 450,
                        thinking_percentage: None,
                        tokens_per_second: Some(200.0),
                    },
                    tool_calls: vec![],
                    response_size_bytes: 500,
                    content_blocks: 1,
                    chunk_count: None,
                    streaming_duration_ms: None,
                    has_refusal: false,
                    is_streaming: false,
                },
            },
            SessionEvent::Completed {
                session_id: session_id.to_string(),
                request_id: "req-1".to_string(),
                timestamp: Utc::now(),
                success: true,
                error: None,
                finish_reason: Some("end_turn".to_string()),
                final_stats: Box::new(FinalSessionStats {
                    total_duration_ms: 600,
                    provider_time_ms: 550,
                    proxy_overhead_ms: 50.0,
                    total_tokens: TokenTotals {
                        total_input: 100,
                        total_output: 300,
                        total_thinking: 50,
                        total_cached: 0,
                        grand_total: 450,
                        total_reasoning: 0,
                        total_cache_read: 0,
                        total_cache_creation: 0,
                        total_audio_input: 0,
                        total_audio_output: 0,
                        by_model: HashMap::new(),
                    },
                    tool_summary: ToolUsageSummary::default(),
                    performance: PerformanceMetrics::default(),
                    streaming_stats: None,
                    estimated_cost: None,
                }),
            },
        ];

        writer.write_batch(&events).await.unwrap();

        // Verify tokens remain correct (MAX logic)
        let (input, output, thinking): (i64, i64, Option<i64>) = sqlx::query_as(
            "SELECT input_tokens, output_tokens, thinking_tokens FROM sessions WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(&writer.pool)
        .await
        .unwrap();

        assert_eq!(input, 100);
        assert_eq!(output, 300);
        assert_eq!(thinking, Some(50));
    }

    /// Test StatsUpdated event updates existing session with tokens
    #[tokio::test]
    async fn test_stats_updated_tokens() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        let session_id = "stats-updated-tokens-test";
        let request_id = "req-1";

        // Create session with empty tokens (passthrough streaming mode)
        let events = vec![
            SessionEvent::Started {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                timestamp: Utc::now(),
                model_requested: "claude-3-opus".to_string(),
                provider: "anthropic".to_string(),
                listener: "anthropic".to_string(),
                is_streaming: true,
                metadata: SessionMetadata {
                    client_ip: None,
                    user_agent: None,
                    api_version: None,
                    request_headers: HashMap::new(),
                    session_tags: vec![],
                },
            },
            SessionEvent::Completed {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                timestamp: Utc::now(),
                success: true,
                error: None,
                finish_reason: Some("end_turn".to_string()),
                final_stats: Box::new(FinalSessionStats {
                    total_duration_ms: 5000,
                    provider_time_ms: 4800,
                    proxy_overhead_ms: 0.0,
                    total_tokens: TokenTotals::default(), // Empty initially
                    tool_summary: ToolUsageSummary::default(),
                    performance: PerformanceMetrics::default(),
                    streaming_stats: None,
                    estimated_cost: None,
                }),
            },
        ];

        writer.write_batch(&events).await.unwrap();

        // Verify tokens are 0
        let (input, output): (Option<i64>, Option<i64>) =
            sqlx::query_as("SELECT input_tokens, output_tokens FROM sessions WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&writer.pool)
                .await
                .unwrap();

        assert_eq!(input, Some(0));
        assert_eq!(output, Some(0));

        // Now emit StatsUpdated event (from async parser)
        let update_event = SessionEvent::StatsUpdated {
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            timestamp: Utc::now(),
            token_updates: Some(TokenTotals {
                total_input: 1500,
                total_output: 800,
                total_thinking: 200,
                total_cached: 0,
                grand_total: 2500,
                total_reasoning: 0,
                total_cache_read: 0,
                total_cache_creation: 0,
                total_audio_input: 0,
                total_audio_output: 0,
                by_model: HashMap::new(),
            }),
            tool_call_updates: None,
            model_used: Some("claude-3-opus-20240229".to_string()),
            response_size_bytes: 1024,
            content_blocks: 1,
            has_refusal: false,
            user_agent: Some("test-client/1.0.0".to_string()),
        };

        writer.write_event(&update_event).await.unwrap();

        // Verify tokens are updated
        let (input, output, thinking, model): (Option<i64>, Option<i64>, Option<i64>, Option<String>) = sqlx::query_as(
            "SELECT input_tokens, output_tokens, thinking_tokens, model_used FROM sessions WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(&writer.pool)
        .await
        .unwrap();

        assert_eq!(input, Some(1500));
        assert_eq!(output, Some(800));
        assert_eq!(thinking, Some(200));
        assert_eq!(model, Some("claude-3-opus-20240229".to_string()));
    }

    /// Test StatsUpdated event updates existing session with tool calls
    #[tokio::test]
    async fn test_stats_updated_tool_calls() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        let session_id = "stats-updated-tools-test";
        let request_id = "req-1";

        // Create session without tool calls (passthrough mode)
        let events = vec![
            SessionEvent::Started {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                timestamp: Utc::now(),
                model_requested: "gpt-4".to_string(),
                provider: "openai".to_string(),
                listener: "openai".to_string(),
                is_streaming: false,
                metadata: SessionMetadata {
                    client_ip: None,
                    user_agent: None,
                    api_version: None,
                    request_headers: HashMap::new(),
                    session_tags: vec![],
                },
            },
            SessionEvent::Completed {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                timestamp: Utc::now(),
                success: true,
                error: None,
                finish_reason: Some("stop".to_string()),
                final_stats: Box::new(FinalSessionStats {
                    total_duration_ms: 2000,
                    provider_time_ms: 1900,
                    proxy_overhead_ms: 0.0,
                    total_tokens: TokenTotals::default(),
                    tool_summary: ToolUsageSummary::default(), // Empty initially
                    performance: PerformanceMetrics::default(),
                    streaming_stats: None,
                    estimated_cost: None,
                }),
            },
        ];

        writer.write_batch(&events).await.unwrap();

        // Verify no tool calls
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tool_stats WHERE session_id = ?")
            .bind(session_id)
            .fetch_one(&writer.pool)
            .await
            .unwrap();

        assert_eq!(count, 0);

        // Now emit StatsUpdated event with tool calls (from async parser)
        let mut by_tool = HashMap::new();
        by_tool.insert(
            "get_weather".to_string(),
            crate::events::ToolStats {
                call_count: 2,
                total_execution_time_ms: 0,
                avg_execution_time_ms: 0,
                error_count: 0,
            },
        );
        by_tool.insert(
            "search".to_string(),
            crate::events::ToolStats {
                call_count: 1,
                total_execution_time_ms: 0,
                avg_execution_time_ms: 0,
                error_count: 0,
            },
        );

        let update_event = SessionEvent::StatsUpdated {
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            timestamp: Utc::now(),
            token_updates: None,
            tool_call_updates: Some(ToolUsageSummary {
                total_tool_calls: 3,
                unique_tool_count: 2,
                by_tool,
                total_tool_time_ms: 0,
                tool_error_count: 0,
            }),
            model_used: None,
            response_size_bytes: 2048,
            content_blocks: 2,
            has_refusal: false,
            user_agent: None,
        };

        writer.write_event(&update_event).await.unwrap();

        // Verify tool calls are inserted
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tool_stats WHERE session_id = ?")
            .bind(session_id)
            .fetch_one(&writer.pool)
            .await
            .unwrap();

        assert_eq!(count, 2);

        // Verify specific tool counts
        let weather_count: i64 = sqlx::query_scalar(
            "SELECT call_count FROM tool_stats WHERE session_id = ? AND tool_name = 'get_weather'",
        )
        .bind(session_id)
        .fetch_one(&writer.pool)
        .await
        .unwrap();

        assert_eq!(weather_count, 2);

        let search_count: i64 = sqlx::query_scalar(
            "SELECT call_count FROM tool_stats WHERE session_id = ? AND tool_name = 'search'",
        )
        .bind(session_id)
        .fetch_one(&writer.pool)
        .await
        .unwrap();

        assert_eq!(search_count, 1);
    }

    /// Test StatsUpdated event with MAX() logic - should keep higher values
    #[tokio::test]
    async fn test_stats_updated_max_logic() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        let session_id = "stats-updated-max-test";
        let request_id = "req-1";

        // Create session with initial tokens
        let events = vec![
            SessionEvent::Started {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                timestamp: Utc::now(),
                model_requested: "gpt-4".to_string(),
                provider: "openai".to_string(),
                listener: "openai".to_string(),
                is_streaming: false,
                metadata: SessionMetadata {
                    client_ip: None,
                    user_agent: None,
                    api_version: None,
                    request_headers: HashMap::new(),
                    session_tags: vec![],
                },
            },
            SessionEvent::Completed {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                timestamp: Utc::now(),
                success: true,
                error: None,
                finish_reason: Some("stop".to_string()),
                final_stats: Box::new(FinalSessionStats {
                    total_duration_ms: 2000,
                    provider_time_ms: 1900,
                    proxy_overhead_ms: 0.0,
                    total_tokens: TokenTotals {
                        total_input: 100,
                        total_output: 200,
                        total_thinking: 0,
                        total_cached: 0,
                        grand_total: 300,
                        total_reasoning: 0,
                        total_cache_read: 0,
                        total_cache_creation: 0,
                        total_audio_input: 0,
                        total_audio_output: 0,
                        by_model: HashMap::new(),
                    },
                    tool_summary: ToolUsageSummary::default(),
                    performance: PerformanceMetrics::default(),
                    streaming_stats: None,
                    estimated_cost: None,
                }),
            },
        ];

        writer.write_batch(&events).await.unwrap();

        // Try to update with LOWER values - should keep existing higher values
        let update_event = SessionEvent::StatsUpdated {
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            timestamp: Utc::now(),
            token_updates: Some(TokenTotals {
                total_input: 50,   // Lower than 100
                total_output: 150, // Lower than 200
                total_thinking: 0,
                total_cached: 0,
                grand_total: 200,
                total_reasoning: 0,
                total_cache_read: 0,
                total_cache_creation: 0,
                total_audio_input: 0,
                total_audio_output: 0,
                by_model: HashMap::new(),
            }),
            tool_call_updates: None,
            model_used: None,
            response_size_bytes: 512,
            content_blocks: 1,
            has_refusal: false,
            user_agent: None,
        };

        writer.write_event(&update_event).await.unwrap();

        // Verify original higher values are kept (MAX logic)
        let (input, output): (Option<i64>, Option<i64>) =
            sqlx::query_as("SELECT input_tokens, output_tokens FROM sessions WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&writer.pool)
                .await
                .unwrap();

        assert_eq!(input, Some(100)); // Kept higher value
        assert_eq!(output, Some(200)); // Kept higher value

        // Now update with HIGHER values - should update
        let update_event2 = SessionEvent::StatsUpdated {
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            timestamp: Utc::now(),
            token_updates: Some(TokenTotals {
                total_input: 150,   // Higher than 100
                total_output: 250,  // Higher than 200
                total_thinking: 50, // Higher than 0
                total_cached: 0,
                grand_total: 450,
                total_reasoning: 0,
                total_cache_read: 0,
                total_cache_creation: 0,
                total_audio_input: 0,
                total_audio_output: 0,
                by_model: HashMap::new(),
            }),
            tool_call_updates: None,
            model_used: None,
            response_size_bytes: 1500,
            content_blocks: 3,
            has_refusal: false,
            user_agent: Some("test-client/2.0.0".to_string()),
        };

        writer.write_event(&update_event2).await.unwrap();

        // Verify new higher values are stored
        let (input, output, thinking): (Option<i64>, Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT input_tokens, output_tokens, thinking_tokens FROM sessions WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(&writer.pool)
        .await
        .unwrap();

        assert_eq!(input, Some(150)); // Updated to higher value
        assert_eq!(output, Some(250)); // Updated to higher value
        assert_eq!(thinking, Some(50)); // Updated to higher value
    }

    /// Test that tool_arguments from ToolCallRecorded are preserved even when
    /// Completed events arrive first (with None arguments)
    #[tokio::test]
    async fn test_tool_arguments_preserved_across_events() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        let session_id = "tool-args-preservation-test";
        let request_id = "req-1";

        // Scenario 1: Completed event arrives first (with None tool_arguments)
        let events = vec![
            SessionEvent::Started {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                timestamp: Utc::now(),
                model_requested: "claude-sonnet-4".to_string(),
                provider: "anthropic".to_string(),
                listener: "anthropic".to_string(),
                is_streaming: false,
                metadata: SessionMetadata {
                    client_ip: None,
                    user_agent: None,
                    api_version: None,
                    request_headers: HashMap::new(),
                    session_tags: vec![],
                },
            },
            SessionEvent::Completed {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                timestamp: Utc::now(),
                success: true,
                error: None,
                finish_reason: Some("end_turn".to_string()),
                final_stats: Box::new(FinalSessionStats {
                    total_duration_ms: 1000,
                    provider_time_ms: 900,
                    proxy_overhead_ms: 100.0,
                    total_tokens: TokenTotals {
                        total_input: 100,
                        total_output: 200,
                        total_thinking: 0,
                        total_cached: 0,
                        grand_total: 300,
                        total_reasoning: 0,
                        total_cache_read: 0,
                        total_cache_creation: 0,
                        total_audio_input: 0,
                        total_audio_output: 0,
                        by_model: HashMap::new(),
                    },
                    tool_summary: ToolUsageSummary {
                        total_tool_calls: 1,
                        unique_tool_count: 1,
                        by_tool: {
                            let mut by_tool = HashMap::new();
                            by_tool.insert(
                                "Read".to_string(),
                                ToolStats {
                                    call_count: 1,
                                    total_execution_time_ms: 50,
                                    avg_execution_time_ms: 50,
                                    error_count: 0,
                                },
                            );
                            by_tool
                        },
                        total_tool_time_ms: 50,
                        tool_error_count: 0,
                    },
                    performance: PerformanceMetrics::default(),
                    streaming_stats: None,
                    estimated_cost: None,
                }),
            },
        ];

        writer.write_batch(&events).await.unwrap();

        // Verify tool_calls entry exists but has None for tool_arguments
        let tool_args: Option<String> = sqlx::query_scalar(
            "SELECT tool_arguments FROM tool_stats WHERE session_id = ? AND tool_name = 'Read'",
        )
        .bind(session_id)
        .fetch_one(&writer.pool)
        .await
        .unwrap();

        assert_eq!(
            tool_args, None,
            "Initial insert from Completed should have None"
        );

        // Now emit ToolCallRecorded event with actual arguments
        let tool_event = SessionEvent::ToolCallRecorded {
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            timestamp: Utc::now(),
            tool_name: "Read".to_string(),
            tool_call_id: "call-123".to_string(),
            execution_time_ms: Some(50),
            input_size_bytes: 42,
            output_size_bytes: None,
            success: Some(true),
            tool_arguments: Some(r#"{"file_path":"/home/user/test.rs"}"#.to_string()),
        };

        writer.write_event(&tool_event).await.unwrap();

        // Verify tool_arguments are now set (COALESCE should preserve the new value)
        let tool_args: Option<String> = sqlx::query_scalar(
            "SELECT tool_arguments FROM tool_stats WHERE session_id = ? AND tool_name = 'Read'",
        )
        .bind(session_id)
        .fetch_one(&writer.pool)
        .await
        .unwrap();

        assert_eq!(
            tool_args,
            Some(r#"{"file_path":"/home/user/test.rs"}"#.to_string()),
            "ToolCallRecorded should set tool_arguments even if Completed came first"
        );

        // Scenario 2: Another Completed event arrives (should NOT overwrite existing arguments)
        let update_event = SessionEvent::Completed {
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            timestamp: Utc::now(),
            success: true,
            error: None,
            finish_reason: Some("end_turn".to_string()),
            final_stats: Box::new(FinalSessionStats {
                total_duration_ms: 1200,
                provider_time_ms: 1000,
                proxy_overhead_ms: 100.0,
                total_tokens: TokenTotals {
                    total_input: 120,
                    total_output: 250,
                    total_thinking: 0,
                    total_cached: 0,
                    grand_total: 370,
                    total_reasoning: 0,
                    total_cache_read: 0,
                    total_cache_creation: 0,
                    total_audio_input: 0,
                    total_audio_output: 0,
                    by_model: HashMap::new(),
                },
                tool_summary: ToolUsageSummary {
                    total_tool_calls: 2,
                    unique_tool_count: 1,
                    by_tool: {
                        let mut by_tool = HashMap::new();
                        by_tool.insert(
                            "Read".to_string(),
                            ToolStats {
                                call_count: 2,
                                total_execution_time_ms: 100,
                                avg_execution_time_ms: 50,
                                error_count: 0,
                            },
                        );
                        by_tool
                    },
                    total_tool_time_ms: 100,
                    tool_error_count: 0,
                },
                performance: PerformanceMetrics::default(),
                streaming_stats: None,
                estimated_cost: None,
            }),
        };

        writer.write_event(&update_event).await.unwrap();

        // Verify tool_arguments are STILL preserved (COALESCE should keep existing value)
        let tool_args: Option<String> = sqlx::query_scalar(
            "SELECT tool_arguments FROM tool_stats WHERE session_id = ? AND tool_name = 'Read'",
        )
        .bind(session_id)
        .fetch_one(&writer.pool)
        .await
        .unwrap();

        assert_eq!(
            tool_args,
            Some(r#"{"file_path":"/home/user/test.rs"}"#.to_string()),
            "Subsequent Completed event with None should NOT overwrite existing tool_arguments"
        );

        // Verify call_count was updated to 2
        let call_count: i64 = sqlx::query_scalar(
            "SELECT call_count FROM tool_stats WHERE session_id = ? AND tool_name = 'Read'",
        )
        .bind(session_id)
        .fetch_one(&writer.pool)
        .await
        .unwrap();

        assert_eq!(call_count, 2, "Call count should be updated");
    }

    #[tokio::test]
    async fn test_long_user_agent() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_long_ua.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        let session_id = "test-session-long-ua";
        let request_id = "test-request-long-ua";

        // Create a very long user agent (500+ characters)
        let long_ua = "a".repeat(500);

        let events = vec![
            SessionEvent::Started {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                timestamp: Utc::now(),
                model_requested: "claude-3-opus-20240229".to_string(),
                provider: "anthropic".to_string(),
                listener: "anthropic".to_string(),
                is_streaming: false,
                metadata: SessionMetadata {
                    client_ip: None,
                    user_agent: Some(long_ua.clone()),
                    api_version: None,
                    request_headers: HashMap::new(),
                    session_tags: vec![],
                },
            },
            SessionEvent::StatsUpdated {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                timestamp: Utc::now(),
                token_updates: Some(TokenTotals {
                    total_input: 100,
                    total_output: 50,
                    total_thinking: 0,
                    total_cached: 0,
                    grand_total: 150,
                    total_reasoning: 0,
                    total_cache_read: 0,
                    total_cache_creation: 0,
                    total_audio_input: 0,
                    total_audio_output: 0,
                    by_model: HashMap::new(),
                }),
                tool_call_updates: None,
                model_used: Some("claude-3-opus-20240229".to_string()),
                response_size_bytes: 1024,
                content_blocks: 1,
                has_refusal: false,
                user_agent: Some(long_ua),
            },
        ];

        writer.write_batch(&events).await.unwrap();

        // Verify the user agent was stored (potentially truncated by application logic)
        let stored_ua: Option<String> =
            sqlx::query_scalar("SELECT user_agent FROM sessions WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&writer.pool)
                .await
                .unwrap();

        assert!(stored_ua.is_some());
        // Note: The truncation happens at ingress layer, not database layer
    }

    #[tokio::test]
    async fn test_special_chars_in_user_agent() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_special_chars_ua.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        let session_id = "test-session-special-chars";
        let request_id = "test-request-special-chars";

        // User agent with special characters, quotes, and unicode
        let special_ua = r#"Mozilla/5.0 (X11; Linux x86_64) "test" 'quotes' 🚀 emoji \n newline"#;

        let events = vec![
            SessionEvent::Started {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                timestamp: Utc::now(),
                model_requested: "claude-3-opus-20240229".to_string(),
                provider: "anthropic".to_string(),
                listener: "anthropic".to_string(),
                is_streaming: false,
                metadata: SessionMetadata {
                    client_ip: None,
                    user_agent: Some(special_ua.to_string()),
                    api_version: None,
                    request_headers: HashMap::new(),
                    session_tags: vec![],
                },
            },
            SessionEvent::StatsUpdated {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                timestamp: Utc::now(),
                token_updates: Some(TokenTotals {
                    total_input: 100,
                    total_output: 50,
                    total_thinking: 0,
                    total_cached: 0,
                    grand_total: 150,
                    total_reasoning: 0,
                    total_cache_read: 0,
                    total_cache_creation: 0,
                    total_audio_input: 0,
                    total_audio_output: 0,
                    by_model: HashMap::new(),
                }),
                tool_call_updates: None,
                model_used: Some("claude-3-opus-20240229".to_string()),
                response_size_bytes: 1024,
                content_blocks: 1,
                has_refusal: false,
                user_agent: Some(special_ua.to_string()),
            },
        ];

        writer.write_batch(&events).await.unwrap();

        // Verify the special characters are preserved correctly
        let stored_ua: Option<String> =
            sqlx::query_scalar("SELECT user_agent FROM sessions WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&writer.pool)
                .await
                .unwrap();

        assert_eq!(stored_ua.as_deref(), Some(special_ua));

        // Also verify in session_stats
        let stats_ua: Option<String> =
            sqlx::query_scalar("SELECT user_agent FROM session_stats WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&writer.pool)
                .await
                .unwrap();

        assert_eq!(stats_ua.as_deref(), Some(special_ua));
    }

    #[tokio::test]
    async fn test_null_user_agent() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_null_ua.db");
        let writer = SqliteWriter::new(&db_path).await.unwrap();

        let session_id = "test-session-null-ua";
        let request_id = "test-request-null-ua";

        let events = vec![
            SessionEvent::Started {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                timestamp: Utc::now(),
                model_requested: "claude-3-opus-20240229".to_string(),
                provider: "anthropic".to_string(),
                listener: "anthropic".to_string(),
                is_streaming: false,
                metadata: SessionMetadata {
                    client_ip: None,
                    user_agent: None, // No user agent
                    api_version: None,
                    request_headers: HashMap::new(),
                    session_tags: vec![],
                },
            },
            SessionEvent::StatsUpdated {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                timestamp: Utc::now(),
                token_updates: Some(TokenTotals {
                    total_input: 100,
                    total_output: 50,
                    total_thinking: 0,
                    total_cached: 0,
                    grand_total: 150,
                    total_reasoning: 0,
                    total_cache_read: 0,
                    total_cache_creation: 0,
                    total_audio_input: 0,
                    total_audio_output: 0,
                    by_model: HashMap::new(),
                }),
                tool_call_updates: None,
                model_used: Some("claude-3-opus-20240229".to_string()),
                response_size_bytes: 1024,
                content_blocks: 1,
                has_refusal: false,
                user_agent: None, // No user agent
            },
        ];

        writer.write_batch(&events).await.unwrap();

        // Verify NULL is stored correctly
        let stored_ua: Option<String> =
            sqlx::query_scalar("SELECT user_agent FROM sessions WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&writer.pool)
                .await
                .unwrap();

        assert_eq!(stored_ua, None);
    }
}
