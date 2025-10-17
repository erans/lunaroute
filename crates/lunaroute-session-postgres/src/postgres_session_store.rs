//! PostgresSessionStore - SessionStore trait implementation for PostgreSQL multi-tenant storage
//!
//! Supports both vanilla PostgreSQL and PostgreSQL with TimescaleDB extension.
//! TimescaleDB features (hypertables, compression, etc.) are automatically enabled if available.

use async_trait::async_trait;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use std::sync::Arc;
use std::time::Instant;

use crate::config::PostgresSessionStoreConfig;
use crate::metrics::SessionStoreMetrics;
use lunaroute_core::{
    Error, Result,
    events::SessionEvent,
    session_store::{
        AggregateStats, CleanupStats, RetentionPolicy, SearchQuery, SearchResults, Session,
        SessionStore, TimeRange,
    },
    tenant::TenantId,
};

/// PostgreSQL-backed session store for multi-tenant mode
///
/// Works with vanilla PostgreSQL or PostgreSQL + TimescaleDB extension.
/// When TimescaleDB is available, automatically enables:
/// - Hypertable partitioning by tenant_id and created_at
/// - Automatic compression for old data (when configured)
/// - Continuous aggregates for dashboards (when configured)
/// - Built-in retention policies (when configured)
#[derive(Clone)]
pub struct PostgresSessionStore {
    /// PostgreSQL connection pool
    pool: Arc<PgPool>,
    /// Optional metrics collector
    metrics: Option<Arc<SessionStoreMetrics>>,
}

impl PostgresSessionStore {
    /// Create a new PostgreSQL session store with default configuration
    ///
    /// Automatically detects and enables TimescaleDB features if available.
    ///
    /// # Arguments
    /// * `database_url` - PostgreSQL connection string
    ///
    /// # Errors
    /// - `Error::Database` if connection fails or schema migration fails
    pub async fn new(database_url: &str) -> Result<Self> {
        Self::with_config(database_url, PostgresSessionStoreConfig::default()).await
    }

    /// Create a new PostgreSQL session store with custom configuration
    ///
    /// Automatically detects and enables TimescaleDB features if available.
    ///
    /// # Arguments
    /// * `database_url` - PostgreSQL connection string
    /// * `config` - Connection pool configuration
    ///
    /// # Errors
    /// - `Error::Database` if connection fails or schema migration fails
    ///
    /// # Example
    /// ```no_run
    /// # use lunaroute_session_postgres::{PostgresSessionStore, PostgresSessionStoreConfig};
    /// # async fn example() -> lunaroute_core::Result<()> {
    /// let config = PostgresSessionStoreConfig::default()
    ///     .with_max_connections(50)
    ///     .with_min_connections(10);
    /// let store = PostgresSessionStore::with_config(
    ///     "postgres://localhost/lunaroute",
    ///     config
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn with_config(
        database_url: &str,
        config: PostgresSessionStoreConfig,
    ) -> Result<Self> {
        Self::with_config_and_metrics(database_url, config, None).await
    }

    /// Create a new PostgreSQL session store with custom configuration and metrics
    ///
    /// Automatically detects and enables TimescaleDB features if available.
    ///
    /// # Arguments
    /// * `database_url` - PostgreSQL connection string
    /// * `config` - Connection pool configuration
    /// * `metrics` - Optional metrics collector
    ///
    /// # Errors
    /// - `Error::Database` if connection fails or schema migration fails
    pub async fn with_config_and_metrics(
        database_url: &str,
        config: PostgresSessionStoreConfig,
        metrics: Option<SessionStoreMetrics>,
    ) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .min_connections(config.min_connections)
            .acquire_timeout(config.acquire_timeout)
            .idle_timeout(Some(config.idle_timeout))
            .max_lifetime(Some(config.max_lifetime))
            .connect(database_url)
            .await
            .map_err(|e| Error::Database(format!("Failed to connect to PostgreSQL: {}", e)))?;

        let store = Self {
            pool: Arc::new(pool),
            metrics: metrics.map(Arc::new),
        };

        // Run schema migrations
        store.run_migrations().await?;

        // Update pool metrics if metrics are enabled
        if let Some(ref m) = store.metrics {
            let size = store.pool.size() as usize;
            let idle = store.pool.num_idle();
            m.update_pool_metrics("postgres", size, idle);
        }

        Ok(store)
    }

    /// Create from an existing pool (useful for testing)
    pub fn from_pool(pool: PgPool) -> Self {
        Self {
            pool: Arc::new(pool),
            metrics: None,
        }
    }

    /// Run database schema migrations
    async fn run_migrations(&self) -> Result<()> {
        // Check if TimescaleDB extension is available
        let timescale_available: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname = 'timescaledb')",
        )
        .fetch_one(&*self.pool)
        .await
        .unwrap_or(false);

        // Record TimescaleDB availability
        if let Some(ref m) = self.metrics {
            m.set_timescaledb_enabled("postgres", timescale_available);
        }

        // Run versioned migrations
        let migration_result =
            crate::migrations::run_migrations(&self.pool, timescale_available).await;

        // Update migration metrics
        if let Some(ref m) = self.metrics
            && let Ok(Some(version)) = crate::migrations::get_current_version(&self.pool).await
        {
            m.update_migration_metrics(
                "postgres",
                crate::migrations::MIGRATIONS.len(),
                Some(version),
            );
        }

        migration_result
    }

    /// Get the underlying connection pool
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Handle SessionEvent::Started
    async fn handle_started_event(&self, tenant_id: TenantId, event: &SessionEvent) -> Result<()> {
        if let SessionEvent::Started {
            session_id,
            request_id,
            timestamp,
            model_requested,
            provider,
            listener,
            is_streaming,
            metadata,
        } = event
        {
            sqlx::query(
                r#"
                INSERT INTO sessions (
                    tenant_id, session_id, request_id, started_at, created_at,
                    model_requested, provider, listener, is_streaming,
                    client_ip, user_agent
                ) VALUES ($1, $2, $3, $4, $4, $5, $6, $7, $8, $9::INET, $10)
                ON CONFLICT (tenant_id, created_at, session_id) DO NOTHING
                "#,
            )
            .bind(tenant_id.as_uuid())
            .bind(session_id)
            .bind(request_id)
            .bind(timestamp)
            .bind(model_requested)
            .bind(provider)
            .bind(listener)
            .bind(is_streaming)
            .bind(&metadata.client_ip)
            .bind(&metadata.user_agent)
            .execute(&*self.pool)
            .await
            .map_err(|e| Error::SessionStore(format!("Failed to insert started event: {}", e)))?;
        }
        Ok(())
    }

    /// Handle SessionEvent::RequestRecorded
    async fn handle_request_recorded_event(
        &self,
        tenant_id: TenantId,
        event: &SessionEvent,
    ) -> Result<()> {
        if let SessionEvent::RequestRecorded {
            session_id,
            request_text,
            estimated_tokens,
            ..
        } = event
        {
            sqlx::query(
                r#"
                UPDATE sessions SET
                    request_text = $3,
                    input_tokens = $4
                WHERE tenant_id = $1 AND session_id = $2
                "#,
            )
            .bind(tenant_id.as_uuid())
            .bind(session_id)
            .bind(request_text)
            .bind(*estimated_tokens as i32)
            .execute(&*self.pool)
            .await
            .map_err(|e| Error::SessionStore(format!("Failed to update request: {}", e)))?;
        }
        Ok(())
    }

    /// Handle SessionEvent::ResponseRecorded
    async fn handle_response_recorded_event(
        &self,
        tenant_id: TenantId,
        event: &SessionEvent,
    ) -> Result<()> {
        if let SessionEvent::ResponseRecorded {
            session_id,
            request_id,
            response_text,
            model_used,
            stats,
            ..
        } = event
        {
            sqlx::query(
                r#"
                UPDATE sessions SET
                    response_text = $3,
                    output_tokens = $4,
                    thinking_tokens = $5,
                    model_used = $6,
                    provider_latency_ms = $7,
                    total_tokens = input_tokens + $4
                WHERE tenant_id = $1 AND session_id = $2
                "#,
            )
            .bind(tenant_id.as_uuid())
            .bind(session_id)
            .bind(response_text)
            .bind(stats.tokens.output_tokens as i32)
            .bind(stats.tokens.thinking_tokens.map(|t| t as i32))
            .bind(model_used)
            .bind(stats.provider_latency_ms as i64)
            .execute(&*self.pool)
            .await
            .map_err(|e| Error::SessionStore(format!("Failed to update response: {}", e)))?;

            // Insert session stats for this request/response
            sqlx::query(
                r#"
                INSERT INTO session_stats (
                    tenant_id, session_id, request_id,
                    model_name, post_processing_ms, proxy_overhead_ms,
                    input_tokens, output_tokens, thinking_tokens, reasoning_tokens,
                    cache_read_tokens, cache_creation_tokens,
                    audio_input_tokens, audio_output_tokens,
                    tokens_per_second, thinking_percentage,
                    response_size_bytes, content_blocks, has_refusal
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)
                "#,
            )
            .bind(tenant_id.as_uuid())
            .bind(session_id)
            .bind(request_id)
            .bind(model_used)
            .bind(stats.post_processing_ms)
            .bind(stats.total_proxy_overhead_ms)
            .bind(stats.tokens.input_tokens as i32)
            .bind(stats.tokens.output_tokens as i32)
            .bind(stats.tokens.thinking_tokens.map(|t| t as i32))
            .bind(stats.tokens.reasoning_tokens.map(|t| t as i32))
            .bind(stats.tokens.cache_read_tokens.map(|t| t as i32))
            .bind(stats.tokens.cache_creation_tokens.map(|t| t as i32))
            .bind(stats.tokens.audio_input_tokens.map(|t| t as i32))
            .bind(stats.tokens.audio_output_tokens.map(|t| t as i32))
            .bind(stats.tokens.tokens_per_second)
            .bind(stats.tokens.thinking_percentage)
            .bind(stats.response_size_bytes as i64)
            .bind(stats.content_blocks as i32)
            .bind(stats.has_refusal)
            .execute(&*self.pool)
            .await
            .map_err(|e| Error::SessionStore(format!("Failed to insert session stats: {}", e)))?;

            // Record tool calls if any
            for tool_call in &stats.tool_calls {
                self.handle_tool_call(tenant_id, session_id, tool_call)
                    .await?;
            }
        }
        Ok(())
    }

    /// Handle tool call recording
    async fn handle_tool_call(
        &self,
        tenant_id: TenantId,
        session_id: &str,
        tool_call: &lunaroute_core::events::ToolCallStats,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO tool_call_executions (
                tenant_id, session_id, request_id, created_at,
                tool_call_id, tool_name, execution_time_ms, input_size_bytes,
                output_size_bytes, success
            ) VALUES ($1, $2, '', NOW(), $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(tenant_id.as_uuid())
        .bind(session_id)
        .bind(&tool_call.tool_call_id)
        .bind(&tool_call.tool_name)
        .bind(tool_call.execution_time_ms.map(|t| t as i64))
        .bind(tool_call.input_size_bytes as i64)
        .bind(tool_call.output_size_bytes.map(|s| s as i64))
        .bind(tool_call.success)
        .execute(&*self.pool)
        .await
        .map_err(|e| Error::SessionStore(format!("Failed to insert tool call: {}", e)))?;

        Ok(())
    }

    /// Handle SessionEvent::ToolCallRecorded
    async fn handle_tool_call_recorded_event(
        &self,
        tenant_id: TenantId,
        event: &SessionEvent,
    ) -> Result<()> {
        if let SessionEvent::ToolCallRecorded {
            session_id,
            tool_name,
            tool_call_id,
            execution_time_ms,
            input_size_bytes,
            output_size_bytes,
            success,
            tool_arguments,
            ..
        } = event
        {
            sqlx::query(
                r#"
                INSERT INTO tool_call_executions (
                    tenant_id, session_id, request_id, created_at,
                    tool_call_id, tool_name, tool_arguments, execution_time_ms,
                    input_size_bytes, output_size_bytes, success
                ) VALUES ($1, $2, '', NOW(), $3, $4, $5, $6, $7, $8, $9)
                "#,
            )
            .bind(tenant_id.as_uuid())
            .bind(session_id)
            .bind(tool_call_id)
            .bind(tool_name)
            .bind(tool_arguments)
            .bind(execution_time_ms.map(|t| t as i64))
            .bind(*input_size_bytes as i64)
            .bind(output_size_bytes.map(|s| s as i64))
            .bind(success)
            .execute(&*self.pool)
            .await
            .map_err(|e| Error::SessionStore(format!("Failed to insert tool call: {}", e)))?;
        }
        Ok(())
    }

    /// Handle SessionEvent::Completed
    async fn handle_completed_event(
        &self,
        tenant_id: TenantId,
        event: &SessionEvent,
    ) -> Result<()> {
        if let SessionEvent::Completed {
            session_id,
            success,
            error,
            finish_reason,
            final_stats,
            ..
        } = event
        {
            sqlx::query(
                r#"
                UPDATE sessions SET
                    completed_at = NOW(),
                    success = $3,
                    error_message = $4,
                    finish_reason = $5,
                    total_duration_ms = $6
                WHERE tenant_id = $1 AND session_id = $2
                "#,
            )
            .bind(tenant_id.as_uuid())
            .bind(session_id)
            .bind(success)
            .bind(error)
            .bind(finish_reason)
            .bind(final_stats.total_duration_ms as i64)
            .execute(&*self.pool)
            .await
            .map_err(|e| Error::SessionStore(format!("Failed to complete session: {}", e)))?;

            // Record streaming metrics if present
            if let Some(streaming_stats) = &final_stats.streaming_stats {
                sqlx::query(
                    r#"
                    INSERT INTO stream_metrics (
                        tenant_id, session_id, request_id, created_at,
                        time_to_first_token_ms, total_chunks, streaming_duration_ms,
                        avg_chunk_latency_ms, p50_chunk_latency_ms, p95_chunk_latency_ms,
                        p99_chunk_latency_ms, max_chunk_latency_ms, min_chunk_latency_ms
                    ) VALUES ($1, $2, '', NOW(), $3, $4, $5, $6, $7, $8, $9, $10, $11)
                    ON CONFLICT (tenant_id, created_at, session_id) DO NOTHING
                    "#,
                )
                .bind(tenant_id.as_uuid())
                .bind(session_id)
                .bind(streaming_stats.time_to_first_token_ms as i64)
                .bind(streaming_stats.total_chunks as i32)
                .bind(streaming_stats.streaming_duration_ms as i64)
                .bind(streaming_stats.avg_chunk_latency_ms)
                .bind(streaming_stats.p50_chunk_latency_ms.map(|v| v as i64))
                .bind(streaming_stats.p95_chunk_latency_ms.map(|v| v as i64))
                .bind(streaming_stats.p99_chunk_latency_ms.map(|v| v as i64))
                .bind(streaming_stats.max_chunk_latency_ms as i64)
                .bind(streaming_stats.min_chunk_latency_ms as i64)
                .execute(&*self.pool)
                .await
                .ok();
            }
        }
        Ok(())
    }

    /// Handle SessionEvent::StreamStarted
    async fn handle_stream_started_event(
        &self,
        _tenant_id: TenantId,
        event: &SessionEvent,
    ) -> Result<()> {
        if let SessionEvent::StreamStarted {
            session_id,
            time_to_first_token_ms,
            ..
        } = event
        {
            // We'll store this temporarily or just log it
            // Full streaming metrics are recorded in Completed event
            tracing::debug!(
                "Stream started for session {} with TTFT: {}ms",
                session_id,
                time_to_first_token_ms
            );
        }
        Ok(())
    }

    /// Handle SessionEvent::StatsUpdated
    async fn handle_stats_updated_event(
        &self,
        tenant_id: TenantId,
        event: &SessionEvent,
    ) -> Result<()> {
        if let SessionEvent::StatsUpdated {
            session_id,
            token_updates: Some(tokens),
            model_used,
            ..
        } = event
        {
            sqlx::query(
                r#"
                UPDATE sessions SET
                    input_tokens = $3,
                    output_tokens = $4,
                    thinking_tokens = $5,
                    total_tokens = $3 + $4,
                    model_used = COALESCE($6, model_used)
                WHERE tenant_id = $1 AND session_id = $2
                "#,
            )
            .bind(tenant_id.as_uuid())
            .bind(session_id)
            .bind(tokens.total_input as i32)
            .bind(tokens.total_output as i32)
            .bind(tokens.total_thinking as i32)
            .bind(model_used)
            .execute(&*self.pool)
            .await
            .ok();
        }
        Ok(())
    }
}

#[async_trait]
impl SessionStore for PostgresSessionStore {
    async fn write_event(
        &self,
        tenant_id: Option<TenantId>,
        event: serde_json::Value,
    ) -> Result<()> {
        let start = Instant::now();

        // Multi-tenant mode only - require tenant_id
        let tenant_id = tenant_id.ok_or_else(|| {
            Error::TenantRequired(
                "PostgresSessionStore requires a tenant_id (multi-tenant mode only)".to_string(),
            )
        })?;

        // Convert JSON to SessionEvent
        let event: SessionEvent = serde_json::from_value(event)
            .map_err(|e| Error::SessionStore(format!("Failed to deserialize event: {}", e)))?;

        // Determine event type for metrics
        let event_type = match &event {
            SessionEvent::Started { .. } => "Started",
            SessionEvent::StreamStarted { .. } => "StreamStarted",
            SessionEvent::RequestRecorded { .. } => "RequestRecorded",
            SessionEvent::ResponseRecorded { .. } => "ResponseRecorded",
            SessionEvent::ToolCallRecorded { .. } => "ToolCallRecorded",
            SessionEvent::Completed { .. } => "Completed",
            SessionEvent::StatsUpdated { .. } => "StatsUpdated",
            SessionEvent::StatsSnapshot { .. } => "StatsSnapshot",
        };

        // Handle event based on type
        let result = match &event {
            SessionEvent::Started { .. } => self.handle_started_event(tenant_id, &event).await,
            SessionEvent::StreamStarted { .. } => {
                self.handle_stream_started_event(tenant_id, &event).await
            }
            SessionEvent::RequestRecorded { .. } => {
                self.handle_request_recorded_event(tenant_id, &event).await
            }
            SessionEvent::ResponseRecorded { .. } => {
                self.handle_response_recorded_event(tenant_id, &event).await
            }
            SessionEvent::ToolCallRecorded { .. } => {
                self.handle_tool_call_recorded_event(tenant_id, &event)
                    .await
            }
            SessionEvent::Completed { .. } => self.handle_completed_event(tenant_id, &event).await,
            SessionEvent::StatsUpdated { .. } => {
                self.handle_stats_updated_event(tenant_id, &event).await
            }
            SessionEvent::StatsSnapshot { .. } => {
                // Stats snapshots are not persisted to PostgreSQL (only final stats matter)
                Ok(())
            }
        };

        // Record metrics
        if let Some(ref m) = self.metrics {
            let duration = start.elapsed().as_secs_f64();
            match &result {
                Ok(_) => m.record_event_written(&tenant_id.to_string(), event_type, duration),
                Err(e) => {
                    let error_type = match e {
                        Error::Database(_) => "database_error",
                        Error::SessionStore(_) => "session_store_error",
                        _ => "unknown_error",
                    };
                    m.record_event_write_error(&tenant_id.to_string(), event_type, error_type);
                }
            }
        }

        result
    }

    async fn search(
        &self,
        tenant_id: Option<TenantId>,
        _query: SearchQuery,
    ) -> Result<SearchResults> {
        let start = Instant::now();

        // Multi-tenant mode only - require tenant_id
        let tenant_id = tenant_id.ok_or_else(|| {
            Error::TenantRequired(
                "PostgresSessionStore requires a tenant_id (multi-tenant mode only)".to_string(),
            )
        })?;

        // For now, implement basic search
        // TODO: Add filters, pagination, sorting
        let result = async {
            let rows = sqlx::query(
                r#"
                SELECT
                    session_id, request_id, started_at, completed_at,
                    provider, model_requested, model_used, success,
                    error_message, finish_reason, total_duration_ms,
                    input_tokens, output_tokens, total_tokens, is_streaming,
                    client_ip::TEXT as client_ip
                FROM sessions
                WHERE tenant_id = $1
                ORDER BY created_at DESC
                LIMIT 50
                "#,
            )
            .bind(tenant_id.as_uuid())
            .fetch_all(&*self.pool)
            .await
            .map_err(|e| Error::SessionStore(format!("Search failed: {}", e)))?;

            // Convert rows to session records
            let items: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|row| {
                    serde_json::json!({
                        "session_id": row.get::<String, _>("session_id"),
                        "request_id": row.get::<Option<String>, _>("request_id"),
                        "started_at": row.get::<chrono::DateTime<chrono::Utc>, _>("started_at").to_rfc3339(),
                        "completed_at": row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("completed_at").map(|dt| dt.to_rfc3339()),
                        "provider": row.get::<String, _>("provider"),
                        "model_requested": row.get::<String, _>("model_requested"),
                        "model_used": row.get::<Option<String>, _>("model_used"),
                        "success": row.get::<Option<bool>, _>("success"),
                        "error_message": row.get::<Option<String>, _>("error_message"),
                        "finish_reason": row.get::<Option<String>, _>("finish_reason"),
                        "total_duration_ms": row.get::<Option<i64>, _>("total_duration_ms"),
                        "input_tokens": row.get::<i32, _>("input_tokens"),
                        "output_tokens": row.get::<i32, _>("output_tokens"),
                        "total_tokens": row.get::<i32, _>("total_tokens"),
                        "is_streaming": row.get::<bool, _>("is_streaming"),
                        "client_ip": row.get::<Option<String>, _>("client_ip"),
                    })
                })
                .collect();

            let results = serde_json::json!({
                "items": items,
                "total_count": items.len(),
                "page": 0,
                "page_size": 50,
                "total_pages": 1,
            });

            Ok(results)
        }.await;

        // Record metrics
        if let Some(ref m) = self.metrics {
            let duration = start.elapsed().as_secs_f64();
            m.record_search(&tenant_id.to_string(), duration, result.is_ok());
        }

        result
    }

    async fn get_session(&self, tenant_id: Option<TenantId>, session_id: &str) -> Result<Session> {
        let start = Instant::now();

        // Multi-tenant mode only - require tenant_id
        let tenant_id = tenant_id.ok_or_else(|| {
            Error::TenantRequired(
                "PostgresSessionStore requires a tenant_id (multi-tenant mode only)".to_string(),
            )
        })?;

        let result = async {
            let row = sqlx::query(
                r#"
                SELECT
                    session_id, request_id, started_at, completed_at,
                    provider, model_requested, model_used, success,
                    error_message, finish_reason, total_duration_ms,
                    input_tokens, output_tokens, total_tokens, is_streaming,
                    client_ip::TEXT as client_ip
                FROM sessions
                WHERE tenant_id = $1 AND session_id = $2
                LIMIT 1
                "#,
            )
            .bind(tenant_id.as_uuid())
            .bind(session_id)
            .fetch_optional(&*self.pool)
            .await
            .map_err(|e| Error::SessionStore(format!("Failed to get session: {}", e)))?;

            match row {
                Some(row) => {
                    let session = serde_json::json!({
                        "session_id": row.get::<String, _>("session_id"),
                        "request_id": row.get::<Option<String>, _>("request_id"),
                        "started_at": row.get::<chrono::DateTime<chrono::Utc>, _>("started_at").to_rfc3339(),
                        "completed_at": row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("completed_at").map(|dt| dt.to_rfc3339()),
                        "provider": row.get::<String, _>("provider"),
                        "model_requested": row.get::<String, _>("model_requested"),
                        "model_used": row.get::<Option<String>, _>("model_used"),
                        "success": row.get::<Option<bool>, _>("success"),
                        "error_message": row.get::<Option<String>, _>("error_message"),
                        "finish_reason": row.get::<Option<String>, _>("finish_reason"),
                        "total_duration_ms": row.get::<Option<i64>, _>("total_duration_ms"),
                        "input_tokens": row.get::<i32, _>("input_tokens"),
                        "output_tokens": row.get::<i32, _>("output_tokens"),
                        "total_tokens": row.get::<i32, _>("total_tokens"),
                        "is_streaming": row.get::<bool, _>("is_streaming"),
                        "client_ip": row.get::<Option<String>, _>("client_ip"),
                    });
                    Ok(session)
                }
                None => Err(Error::SessionNotFound(format!(
                    "Session not found: {}",
                    session_id
                ))),
            }
        }.await;

        // Record metrics
        if let Some(ref m) = self.metrics {
            let duration = start.elapsed().as_secs_f64();
            let found = result.is_ok();
            m.record_session_retrieved(&tenant_id.to_string(), duration, found);

            if let Err(ref e) = result {
                let error_type = match e {
                    Error::SessionNotFound(_) => "not_found",
                    Error::Database(_) => "database_error",
                    Error::SessionStore(_) => "session_store_error",
                    _ => "unknown_error",
                };
                m.record_session_retrieval_error(&tenant_id.to_string(), error_type);
            }
        }

        result
    }

    async fn cleanup(
        &self,
        tenant_id: Option<TenantId>,
        _retention: RetentionPolicy,
    ) -> Result<CleanupStats> {
        // Multi-tenant mode only - require tenant_id
        let _tenant_id = tenant_id.ok_or_else(|| {
            Error::TenantRequired(
                "PostgresSessionStore requires a tenant_id (multi-tenant mode only)".to_string(),
            )
        })?;

        // TODO: Implement actual cleanup logic
        // For now, return empty stats

        let stats = serde_json::json!({
            "sessions_deleted": 0,
            "bytes_freed": 0,
            "files_deleted": 0,
        });

        Ok(stats)
    }

    async fn get_stats(
        &self,
        tenant_id: Option<TenantId>,
        _time_range: TimeRange,
    ) -> Result<AggregateStats> {
        // Multi-tenant mode only - require tenant_id
        let _tenant_id = tenant_id.ok_or_else(|| {
            Error::TenantRequired(
                "PostgresSessionStore requires a tenant_id (multi-tenant mode only)".to_string(),
            )
        })?;

        // TODO: Implement aggregate statistics
        // For now, return basic stats

        let stats = serde_json::json!({
            "total_sessions": 0,
            "total_requests": 0,
            "total_input_tokens": 0,
            "total_output_tokens": 0,
        });

        Ok(stats)
    }

    async fn flush(&self) -> Result<()> {
        // PostgreSQL writes are synchronous, no flush needed
        Ok(())
    }

    async fn list_sessions(
        &self,
        tenant_id: Option<TenantId>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Session>> {
        let start = Instant::now();

        // Multi-tenant mode only - require tenant_id
        let tenant_id = tenant_id.ok_or_else(|| {
            Error::TenantRequired(
                "PostgresSessionStore requires a tenant_id (multi-tenant mode only)".to_string(),
            )
        })?;

        let result = async {
            let rows = sqlx::query(
                r#"
                SELECT
                    session_id, request_id, started_at, completed_at,
                    provider, model_requested, model_used, success,
                    error_message, finish_reason, total_duration_ms,
                    input_tokens, output_tokens, total_tokens, is_streaming,
                    client_ip::TEXT as client_ip
                FROM sessions
                WHERE tenant_id = $1
                ORDER BY created_at DESC
                LIMIT $2 OFFSET $3
                "#,
            )
            .bind(tenant_id.as_uuid())
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(&*self.pool)
            .await
            .map_err(|e| Error::SessionStore(format!("Failed to list sessions: {}", e)))?;

            let sessions: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|row| {
                    serde_json::json!({
                        "session_id": row.get::<String, _>("session_id"),
                        "request_id": row.get::<Option<String>, _>("request_id"),
                        "started_at": row.get::<chrono::DateTime<chrono::Utc>, _>("started_at").to_rfc3339(),
                        "completed_at": row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("completed_at").map(|dt| dt.to_rfc3339()),
                        "provider": row.get::<String, _>("provider"),
                        "model_requested": row.get::<String, _>("model_requested"),
                        "model_used": row.get::<Option<String>, _>("model_used"),
                        "success": row.get::<Option<bool>, _>("success"),
                        "error_message": row.get::<Option<String>, _>("error_message"),
                        "finish_reason": row.get::<Option<String>, _>("finish_reason"),
                        "total_duration_ms": row.get::<Option<i64>, _>("total_duration_ms"),
                        "input_tokens": row.get::<i32, _>("input_tokens"),
                        "output_tokens": row.get::<i32, _>("output_tokens"),
                        "total_tokens": row.get::<i32, _>("total_tokens"),
                        "is_streaming": row.get::<bool, _>("is_streaming"),
                        "client_ip": row.get::<Option<String>, _>("client_ip"),
                    })
                })
                .collect();

            Ok(sessions)
        }.await;

        // Record metrics
        if let Some(ref m) = self.metrics {
            let duration = start.elapsed().as_secs_f64();
            m.record_list(&tenant_id.to_string(), duration, result.is_ok());
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn create_test_store() -> Result<PostgresSessionStore> {
        let database_url = std::env::var("TEST_DATABASE_URL").unwrap_or_else(|_| {
            "postgres://postgres:postgres@localhost:5432/lunaroute_test".to_string()
        });

        PostgresSessionStore::new(&database_url).await
    }

    #[tokio::test]
    #[ignore] // Requires PostgreSQL instance (TimescaleDB extension optional)
    async fn test_create_store() {
        let store = create_test_store().await;
        assert!(store.is_ok());
    }

    #[tokio::test]
    #[ignore] // Requires PostgreSQL instance (TimescaleDB extension optional)
    async fn test_write_started_event() {
        let store = create_test_store().await.unwrap();
        let tenant_id = TenantId::new();

        let event = serde_json::json!({
            "type": "started",
            "session_id": "test-session-1",
            "request_id": "req-1",
            "timestamp": "2024-01-01T00:00:00Z",
            "model_requested": "gpt-4",
            "provider": "openai",
            "listener": "openai",
            "is_streaming": false,
            "client_ip": null,
            "user_agent": null,
            "api_version": null,
            "request_headers": {},
            "session_tags": []
        });

        let result = store.write_event(Some(tenant_id), event).await;
        assert!(result.is_ok());

        // Cleanup
        sqlx::query("DELETE FROM sessions WHERE tenant_id = $1")
            .bind(tenant_id.as_uuid())
            .execute(&*store.pool)
            .await
            .ok();
    }

    #[tokio::test]
    #[ignore] // Requires PostgreSQL instance (TimescaleDB extension optional)
    async fn test_get_session() {
        let store = create_test_store().await.unwrap();
        let tenant_id = TenantId::new();
        let session_id = "test-session-get";

        // Insert a session
        let event = serde_json::json!({
            "type": "started",
            "session_id": session_id,
            "request_id": "req-1",
            "timestamp": "2024-01-01T00:00:00Z",
            "model_requested": "gpt-4",
            "provider": "openai",
            "listener": "openai",
            "is_streaming": false,
            "client_ip": null,
            "user_agent": null,
            "api_version": null,
            "request_headers": {},
            "session_tags": []
        });

        store.write_event(Some(tenant_id), event).await.unwrap();

        // Get the session
        let result = store.get_session(Some(tenant_id), session_id).await;
        assert!(result.is_ok());

        let session = result.unwrap();
        assert_eq!(session["session_id"], session_id);

        // Cleanup
        sqlx::query("DELETE FROM sessions WHERE tenant_id = $1")
            .bind(tenant_id.as_uuid())
            .execute(&*store.pool)
            .await
            .ok();
    }

    #[tokio::test]
    #[ignore] // Requires PostgreSQL instance (TimescaleDB extension optional)
    async fn test_requires_tenant_id() {
        let store = create_test_store().await.unwrap();

        // write_event without tenant_id should fail
        let event = serde_json::json!({"type": "test"});
        let result = store.write_event(None, event).await;
        assert!(matches!(result, Err(Error::TenantRequired(_))));

        // get_session without tenant_id should fail
        let result = store.get_session(None, "session-id").await;
        assert!(matches!(result, Err(Error::TenantRequired(_))));
    }
}
