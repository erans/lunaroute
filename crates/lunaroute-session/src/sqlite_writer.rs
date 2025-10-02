//! SQLite session writer implementation

#[cfg(feature = "sqlite-writer")]
use crate::events::SessionEvent;
#[cfg(feature = "sqlite-writer")]
use crate::search::{SearchResults, SessionAggregates, SessionFilter, SessionRecord, SortOrder};
#[cfg(feature = "sqlite-writer")]
use crate::writer::{SessionWriter, WriterError, WriterResult};
#[cfg(feature = "sqlite-writer")]
use async_trait::async_trait;
#[cfg(feature = "sqlite-writer")]
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous};
#[cfg(feature = "sqlite-writer")]
use sqlx::Row;
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
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_stream_metrics_session ON stream_metrics(session_id)")
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

        Ok(())
    }

    /// Search for sessions matching the given filter
    pub async fn search_sessions(
        &self,
        filter: &SessionFilter,
    ) -> WriterResult<SearchResults<SessionRecord>> {
        filter.validate().map_err(|e| WriterError::Database(e))?;

        // Build the WHERE clause
        let (where_clause, bind_values) = Self::build_where_clause(filter);

        // Build the ORDER BY clause
        let order_by = match filter.sort {
            SortOrder::NewestFirst => "created_at DESC",
            SortOrder::OldestFirst => "created_at ASC",
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
            query
                .bind(filter.page_size as i64)
                .bind(offset as i64)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| WriterError::Database(e.to_string()))?
        };

        Ok(SearchResults::new(
            records,
            total_count as u64,
            filter.page,
            filter.page_size,
        ))
    }

    /// Get session aggregates for the given filter
    pub async fn get_aggregates(
        &self,
        filter: &SessionFilter,
    ) -> WriterResult<SessionAggregates> {
        filter.validate().map_err(|e| WriterError::Database(e))?;

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
            let placeholders = filter.providers.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            conditions.push(format!("provider IN ({})", placeholders));
            bind_values.extend(filter.providers.clone());
        }

        if !filter.models.is_empty() {
            let placeholders = filter.models.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            conditions.push(format!("(model_requested IN ({}) OR model_used IN ({}))", placeholders, placeholders));
            bind_values.extend(filter.models.clone());
            bind_values.extend(filter.models.clone());
        }

        if !filter.request_ids.is_empty() {
            let placeholders = filter.request_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            conditions.push(format!("request_id IN ({})", placeholders));
            bind_values.extend(filter.request_ids.clone());
        }

        if !filter.session_ids.is_empty() {
            let placeholders = filter.session_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            conditions.push(format!("session_id IN ({})", placeholders));
            bind_values.extend(filter.session_ids.clone());
        }

        if let Some(success) = filter.success {
            conditions.push("success = ?".to_string());
            bind_values.push(if success { "1".to_string() } else { "0".to_string() });
        }

        if let Some(is_streaming) = filter.is_streaming {
            conditions.push("is_streaming = ?".to_string());
            bind_values.push(if is_streaming { "1".to_string() } else { "0".to_string() });
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
            let placeholders = filter.client_ips.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            conditions.push(format!("client_ip IN ({})", placeholders));
            bind_values.extend(filter.client_ips.clone());
        }

        if !filter.finish_reasons.is_empty() {
            let placeholders = filter.finish_reasons.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            conditions.push(format!("finish_reason IN ({})", placeholders));
            bind_values.extend(filter.finish_reasons.clone());
        }

        if let Some(ref text_search) = filter.text_search {
            conditions.push("(request_text LIKE ? OR response_text LIKE ?)".to_string());
            let pattern = format!("%{}%", text_search);
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
                    sqlx::query(
                        r#"
                        UPDATE sessions
                        SET time_to_first_token_ms = ?
                        WHERE session_id = ?
                        "#,
                    )
                    .bind(*time_to_first_token_ms as i64)
                    .bind(session_id)
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
                            provider_latency_ms = ?,
                            chunk_count = ?,
                            streaming_duration_ms = ?
                        WHERE session_id = ?
                        "#,
                    )
                    .bind(response_text)
                    .bind(model_used)
                    .bind(stats.tokens.output_tokens as i64)
                    .bind(stats.tokens.thinking_tokens.map(|t| t as i64))
                    .bind(stats.tokens.input_tokens as i64)
                    .bind(stats.provider_latency_ms as i64)
                    .bind(stats.chunk_count.map(|c| c as i64))
                    .bind(stats.streaming_duration_ms.map(|d| d as i64))
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
                    request_id,
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

                    // Insert stream metrics if this was a streaming session
                    if let Some(streaming_stats) = &final_stats.streaming_stats {
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
                        .bind(streaming_stats.time_to_first_token_ms as i64)
                        .bind(streaming_stats.total_chunks as i64)
                        .bind(streaming_stats.streaming_duration_ms as i64)
                        .bind(streaming_stats.avg_chunk_latency_ms)
                        .bind(streaming_stats.p50_chunk_latency_ms.map(|p| p as i64))
                        .bind(streaming_stats.p95_chunk_latency_ms.map(|p| p as i64))
                        .bind(streaming_stats.p99_chunk_latency_ms.map(|p| p as i64))
                        .bind(streaming_stats.max_chunk_latency_ms as i64)
                        .bind(streaming_stats.min_chunk_latency_ms as i64)
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| WriterError::Database(e.to_string()))?;
                    }
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
                        cache_write_tokens: None,
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
        let is_streaming: bool = sqlx::query_scalar(
            "SELECT is_streaming FROM sessions WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(&writer.pool)
        .await
        .unwrap();
        assert!(is_streaming);

        // Verify TTFT was recorded
        let ttft: Option<i64> = sqlx::query_scalar(
            "SELECT time_to_first_token_ms FROM sessions WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(&writer.pool)
        .await
        .unwrap();
        assert_eq!(ttft, Some(150));

        // Verify stream_metrics table has the data
        let stream_metrics_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM stream_metrics WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(&writer.pool)
        .await
        .unwrap();
        assert_eq!(stream_metrics_count, 1);

        // Verify streaming stats details
        let (total_chunks, streaming_duration, avg_latency, p95_latency): (i64, i64, f64, Option<i64>) =
            sqlx::query_as(
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
                    model_requested: if i % 2 == 0 { "gpt-4".to_string() } else { "claude-3".to_string() },
                    provider: if i % 2 == 0 { "openai".to_string() } else { "anthropic".to_string() },
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
                    model_used: if i % 2 == 0 { "gpt-4".to_string() } else { "claude-3".to_string() },
                    stats: ResponseStats {
                        provider_latency_ms: 100 + (i as u64 * 10),
                        post_processing_ms: 10.0,
                        total_proxy_overhead_ms: 15.0,
                        tokens: TokenStats {
                            input_tokens: 10,
                            output_tokens: 20 + (i as u32 * 5),
                            thinking_tokens: None,
                            cache_read_tokens: None,
                            cache_write_tokens: None,
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
                            cache_write_tokens: None,
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
                    error: if i >= 4 { Some("Error".to_string()) } else { None },
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
}
