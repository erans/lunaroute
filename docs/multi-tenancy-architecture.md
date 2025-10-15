# Multi-Tenancy Architecture

## Overview

This document outlines the architecture for refactoring LunaRoute to support multi-tenant deployments while maintaining the existing single-tenant local version. The design uses trait-based abstractions to share 95%+ of the codebase between both deployment models.

## Design Goals

1. **Zero Breaking Changes**: Local single-tenant version continues to work exactly as before
2. **Maximum Code Reuse**: Share all business logic between local and multi-tenant versions
3. **Database-Backed Multi-Tenant**: Support for PostgreSQL/TimescaleDB configuration and session storage
4. **Flexible Deployment**: Single codebase producing two separate binaries (`lunaroute` and `lunaroute-cloud`)
5. **Testability**: Mock implementations via traits for comprehensive testing
6. **Scalability**: Horizontal scaling for multi-tenant SaaS deployments

## Current Architecture

### Configuration Storage
- **Source**: YAML/TOML files (`config.yaml`, `config.toml`)
- **Overrides**: Environment variables
- **Location**: File system paths with tilde expansion
- **Hot-Reload**: File watcher (`notify` crate) detects config changes

### Session Storage
- **Dual-Writer System**:
  - **JSONL Writer**: Newline-delimited JSON files in `~/.lunaroute/sessions/`
  - **SQLite Writer**: Relational database at `~/.lunaroute/sessions.db`
- **Event-Driven**: Asynchronous batch processing via MPSC channel
- **Schema**: 5 tables with 20+ indexes for query optimization

### Limitations for Multi-Tenancy
- File-based configuration not suitable for dynamic tenant management
- SQLite limited to single-machine deployments
- No tenant isolation or per-tenant quotas
- No horizontal scaling for high-volume scenarios

## Proposed Architecture

### 1. Core Abstraction Layer

Create **`lunaroute-core`** crate defining traits for all storage operations:

```rust
// lunaroute-core/src/config_store.rs
use async_trait::async_trait;
use std::sync::Arc;
use futures::stream::Stream;

#[async_trait]
pub trait ConfigStore: Send + Sync {
    /// Get configuration for a tenant (None = single-tenant mode)
    async fn get_config(&self, tenant_id: Option<TenantId>) -> Result<ServerConfig>;

    /// Update configuration for a tenant
    async fn update_config(&self, tenant_id: Option<TenantId>, config: ServerConfig) -> Result<()>;

    /// Watch for configuration changes
    async fn watch_changes(&self, tenant_id: Option<TenantId>) -> ConfigChangeStream;

    /// Validate configuration before saving
    async fn validate_config(&self, config: &ServerConfig) -> Result<()>;
}

// lunaroute-core/src/session_store.rs
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Write a session event (batching handled by implementation)
    async fn write_event(&self, tenant_id: Option<TenantId>, event: SessionEvent) -> Result<()>;

    /// Search sessions with filters
    async fn search(&self, tenant_id: Option<TenantId>, query: SearchQuery) -> Result<SearchResults>;

    /// Get a single session by ID
    async fn get_session(&self, tenant_id: Option<TenantId>, session_id: &str) -> Result<Session>;

    /// Apply retention policies
    async fn cleanup(&self, tenant_id: Option<TenantId>, retention: RetentionPolicy) -> Result<CleanupStats>;

    /// Get aggregated statistics
    async fn get_stats(&self, tenant_id: Option<TenantId>, time_range: TimeRange) -> Result<AggregateStats>;
}

// lunaroute-core/src/tenant.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TenantId(uuid::Uuid);

pub struct TenantContext {
    pub tenant_id: Option<TenantId>,  // None for single-tenant mode
    pub config_store: Arc<dyn ConfigStore>,
    pub session_store: Arc<dyn SessionStore>,
}
```

### 2. Implementation Crates

#### A. Local/Single-Tenant Implementations

**`lunaroute-config-file`** (refactored from current `config.rs`):

```rust
pub struct FileConfigStore {
    config_path: PathBuf,
    watcher: Arc<Mutex<notify::RecommendedWatcher>>,
    cached_config: Arc<RwLock<ServerConfig>>,
}

#[async_trait]
impl ConfigStore for FileConfigStore {
    async fn get_config(&self, tenant_id: Option<TenantId>) -> Result<ServerConfig> {
        // tenant_id is ignored in single-tenant mode
        Ok(self.cached_config.read().await.clone())
    }

    async fn update_config(&self, _tenant_id: Option<TenantId>, config: ServerConfig) -> Result<()> {
        // Write to YAML/TOML file
        let content = serde_yaml::to_string(&config)?;
        tokio::fs::write(&self.config_path, content).await?;

        // Update cache
        *self.cached_config.write().await = config;
        Ok(())
    }

    async fn watch_changes(&self, _tenant_id: Option<TenantId>) -> ConfigChangeStream {
        // Return stream from file watcher
        // Current implementation uses notify crate
    }
}
```

**`lunaroute-session-sqlite`** (refactored from current `sqlite_writer.rs`):

```rust
pub struct SqliteSessionStore {
    pool: SqlitePool,
    jsonl_writer: Option<Arc<JsonlWriter>>,
    event_buffer: Arc<Mutex<Vec<SessionEvent>>>,
    worker_handle: Option<JoinHandle<()>>,
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn write_event(&self, _tenant_id: Option<TenantId>, event: SessionEvent) -> Result<()> {
        // tenant_id is ignored in single-tenant mode
        // Buffer event and let worker handle batching
        self.event_buffer.lock().await.push(event);
        Ok(())
    }

    async fn search(&self, _tenant_id: Option<TenantId>, query: SearchQuery) -> Result<SearchResults> {
        // Execute SQL query against SQLite database
        // Current implementation from session/search.rs
    }
}
```

#### B. Multi-Tenant Implementations

**`lunaroute-config-postgres`**:

```rust
pub struct PostgresConfigStore {
    pool: PgPool,
}

// Schema:
// CREATE TABLE tenant_configs (
//     tenant_id UUID PRIMARY KEY,
//     config JSONB NOT NULL,
//     created_at TIMESTAMPTZ DEFAULT NOW(),
//     updated_at TIMESTAMPTZ DEFAULT NOW(),
//     version INT NOT NULL DEFAULT 1,
//     CONSTRAINT valid_config CHECK (jsonb_typeof(config) = 'object')
// );
//
// CREATE INDEX idx_tenant_configs_updated ON tenant_configs(updated_at DESC);
//
// -- Audit log for config changes
// CREATE TABLE tenant_config_history (
//     id BIGSERIAL PRIMARY KEY,
//     tenant_id UUID NOT NULL REFERENCES tenant_configs(tenant_id),
//     config JSONB NOT NULL,
//     version INT NOT NULL,
//     changed_by TEXT,
//     changed_at TIMESTAMPTZ DEFAULT NOW()
// );

#[async_trait]
impl ConfigStore for PostgresConfigStore {
    async fn get_config(&self, tenant_id: Option<TenantId>) -> Result<ServerConfig> {
        let tenant_id = tenant_id.ok_or(Error::TenantRequired)?;

        let row: (serde_json::Value,) = sqlx::query_as(
            "SELECT config FROM tenant_configs WHERE tenant_id = $1"
        )
        .bind(tenant_id.0)
        .fetch_one(&self.pool)
        .await?;

        let config: ServerConfig = serde_json::from_value(row.0)?;
        Ok(config)
    }

    async fn update_config(&self, tenant_id: Option<TenantId>, config: ServerConfig) -> Result<()> {
        let tenant_id = tenant_id.ok_or(Error::TenantRequired)?;

        // Validate first
        self.validate_config(&config).await?;

        let config_json = serde_json::to_value(&config)?;

        // Update with version increment and history
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            "INSERT INTO tenant_configs (tenant_id, config, version)
             VALUES ($1, $2, 1)
             ON CONFLICT (tenant_id) DO UPDATE
             SET config = $2, version = tenant_configs.version + 1, updated_at = NOW()"
        )
        .bind(tenant_id.0)
        .bind(&config_json)
        .execute(&mut *tx)
        .await?;

        // Record in history
        sqlx::query(
            "INSERT INTO tenant_config_history (tenant_id, config, version)
             SELECT tenant_id, config, version FROM tenant_configs WHERE tenant_id = $1"
        )
        .bind(tenant_id.0)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }
}
```

**`lunaroute-session-postgres`** (PostgreSQL with optional TimescaleDB):

```rust
pub struct PostgresSessionStore {
    pool: PgPool,
}

// Schema with tenant_id partitioning and time-series optimization:
//
// CREATE TABLE sessions (
//     tenant_id UUID NOT NULL,
//     session_id TEXT NOT NULL,
//     request_id TEXT NOT NULL,
//     created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
//
//     -- Session metadata
//     provider TEXT NOT NULL,
//     listener TEXT NOT NULL,
//     model_requested TEXT NOT NULL,
//     model_used TEXT,
//
//     -- Timing
//     started_at TIMESTAMPTZ NOT NULL,
//     completed_at TIMESTAMPTZ,
//     total_duration_ms BIGINT,
//     provider_latency_ms BIGINT,
//
//     -- Token usage
//     input_tokens INTEGER DEFAULT 0,
//     output_tokens INTEGER DEFAULT 0,
//     thinking_tokens INTEGER,
//     reasoning_tokens INTEGER,
//     cache_read_tokens INTEGER,
//     cache_creation_tokens INTEGER,
//     audio_input_tokens INTEGER,
//     audio_output_tokens INTEGER,
//     total_tokens INTEGER GENERATED ALWAYS AS (
//         input_tokens + output_tokens +
//         COALESCE(thinking_tokens, 0) +
//         COALESCE(reasoning_tokens, 0)
//     ) STORED,
//
//     -- Status
//     success BOOLEAN,
//     error_message TEXT,
//     finish_reason TEXT,
//
//     -- Content (consider separate table for large text)
//     request_text TEXT,
//     response_text TEXT,
//
//     -- Client metadata
//     client_ip INET,
//     user_agent TEXT,
//
//     -- Streaming metadata
//     is_streaming BOOLEAN DEFAULT FALSE,
//
//     PRIMARY KEY (tenant_id, created_at, session_id)
// );
//
// -- Convert to hypertable (TimescaleDB)
// SELECT create_hypertable('sessions', 'created_at',
//     partitioning_column => 'tenant_id',
//     number_partitions => 4,
//     chunk_time_interval => INTERVAL '1 day'
// );
//
// -- Indexes for common queries
// CREATE INDEX idx_sessions_tenant_time ON sessions(tenant_id, created_at DESC);
// CREATE INDEX idx_sessions_provider ON sessions(tenant_id, provider, created_at DESC);
// CREATE INDEX idx_sessions_model ON sessions(tenant_id, model_used, created_at DESC);
// CREATE INDEX idx_sessions_session_id ON sessions(tenant_id, session_id);
//
// -- Retention policy (auto-delete after 90 days)
// SELECT add_retention_policy('sessions', INTERVAL '90 days');
//
// -- Continuous aggregates for dashboards
// CREATE MATERIALIZED VIEW sessions_hourly
// WITH (timescaledb.continuous) AS
// SELECT
//     tenant_id,
//     time_bucket('1 hour', created_at) AS hour,
//     provider,
//     model_used,
//     COUNT(*) as total_requests,
//     COUNT(*) FILTER (WHERE success = true) as successful_requests,
//     SUM(input_tokens) as total_input_tokens,
//     SUM(output_tokens) as total_output_tokens,
//     AVG(provider_latency_ms) as avg_latency_ms,
//     PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY provider_latency_ms) as p95_latency_ms
// FROM sessions
// GROUP BY tenant_id, hour, provider, model_used;
//
// -- Tool usage tracking
// CREATE TABLE tool_stats (
//     tenant_id UUID NOT NULL,
//     session_id TEXT NOT NULL,
//     request_id TEXT NOT NULL,
//     created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
//
//     tool_name TEXT NOT NULL,
//     call_count INTEGER DEFAULT 1,
//     avg_execution_time_ms BIGINT,
//     error_count INTEGER DEFAULT 0,
//     tool_arguments JSONB,
//
//     PRIMARY KEY (tenant_id, created_at, session_id, tool_name),
//     FOREIGN KEY (tenant_id, created_at, session_id)
//         REFERENCES sessions(tenant_id, created_at, session_id) ON DELETE CASCADE
// );
//
// SELECT create_hypertable('tool_stats', 'created_at',
//     partitioning_column => 'tenant_id',
//     number_partitions => 4
// );
//
// CREATE INDEX idx_tool_stats_tenant_tool ON tool_stats(tenant_id, tool_name, created_at DESC);
//
// -- Streaming metrics
// CREATE TABLE stream_metrics (
//     tenant_id UUID NOT NULL,
//     session_id TEXT NOT NULL,
//     request_id TEXT NOT NULL,
//     created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
//
//     time_to_first_token_ms BIGINT NOT NULL,
//     total_chunks INTEGER,
//     streaming_duration_ms BIGINT,
//     avg_chunk_latency_ms DOUBLE PRECISION,
//     p50_chunk_latency_ms BIGINT,
//     p95_chunk_latency_ms BIGINT,
//     p99_chunk_latency_ms BIGINT,
//     max_chunk_latency_ms BIGINT,
//     min_chunk_latency_ms BIGINT,
//
//     PRIMARY KEY (tenant_id, created_at, session_id),
//     FOREIGN KEY (tenant_id, created_at, session_id)
//         REFERENCES sessions(tenant_id, created_at, session_id) ON DELETE CASCADE
// );
//
// SELECT create_hypertable('stream_metrics', 'created_at',
//     partitioning_column => 'tenant_id',
//     number_partitions => 4
// );

#[async_trait]
impl SessionStore for PostgresSessionStore {
    async fn write_event(&self, tenant_id: Option<TenantId>, event: SessionEvent) -> Result<()> {
        let tenant_id = tenant_id.ok_or(Error::TenantRequired)?;

        match event {
            SessionEvent::Started { session_id, request_id, timestamp, model_requested, provider, is_streaming, metadata, .. } => {
                sqlx::query(
                    "INSERT INTO sessions (
                        tenant_id, session_id, request_id, started_at, created_at,
                        model_requested, provider, listener, is_streaming,
                        client_ip, user_agent
                    ) VALUES ($1, $2, $3, $4, $4, $5, $6, $7, $8, $9, $10)
                    ON CONFLICT (tenant_id, created_at, session_id) DO NOTHING"
                )
                .bind(tenant_id.0)
                .bind(&session_id)
                .bind(&request_id)
                .bind(timestamp)
                .bind(&model_requested)
                .bind(&provider)
                .bind("default") // listener
                .bind(is_streaming)
                .bind(metadata.client_ip.as_ref().and_then(|ip| ip.parse::<std::net::IpAddr>().ok()))
                .bind(&metadata.user_agent)
                .execute(&self.pool)
                .await?;
            }

            SessionEvent::ResponseRecorded { session_id, response_text, model_used, stats, .. } => {
                sqlx::query(
                    "UPDATE sessions SET
                        response_text = $3,
                        output_tokens = $4,
                        thinking_tokens = $5,
                        model_used = $6,
                        provider_latency_ms = $7,
                        response_size_bytes = $8
                    WHERE tenant_id = $1 AND session_id = $2"
                )
                .bind(tenant_id.0)
                .bind(&session_id)
                .bind(&response_text)
                .bind(stats.tokens.output_tokens as i32)
                .bind(stats.tokens.thinking_tokens.map(|t| t as i32))
                .bind(&model_used)
                .bind(stats.provider_latency_ms as i64)
                .bind(stats.response_size_bytes as i64)
                .execute(&self.pool)
                .await?;

                // Record tool calls
                for tool_call in &stats.tool_calls {
                    sqlx::query(
                        "INSERT INTO tool_stats (
                            tenant_id, session_id, request_id, created_at,
                            tool_name, call_count, avg_execution_time_ms, error_count
                        ) VALUES ($1, $2, $3, NOW(), $4, 1, $5, $6)
                        ON CONFLICT (tenant_id, created_at, session_id, tool_name)
                        DO UPDATE SET
                            call_count = tool_stats.call_count + 1,
                            avg_execution_time_ms = (tool_stats.avg_execution_time_ms * tool_stats.call_count + $5) / (tool_stats.call_count + 1),
                            error_count = tool_stats.error_count + $6"
                    )
                    .bind(tenant_id.0)
                    .bind(&session_id)
                    .bind("")  // request_id from context
                    .bind(&tool_call.tool_name)
                    .bind(tool_call.execution_time_ms.unwrap_or(0) as i64)
                    .bind(if tool_call.success.unwrap_or(true) { 0 } else { 1 })
                    .execute(&self.pool)
                    .await?;
                }
            }

            SessionEvent::Completed { session_id, success, error, finish_reason, final_stats, .. } => {
                let mut tx = self.pool.begin().await?;

                // Update session completion
                sqlx::query(
                    "UPDATE sessions SET
                        completed_at = NOW(),
                        success = $3,
                        error_message = $4,
                        finish_reason = $5,
                        total_duration_ms = $6
                    WHERE tenant_id = $1 AND session_id = $2"
                )
                .bind(tenant_id.0)
                .bind(&session_id)
                .bind(success)
                .bind(&error)
                .bind(&finish_reason)
                .bind(final_stats.total_duration_ms as i64)
                .execute(&mut *tx)
                .await?;

                // Record streaming metrics if present
                if let Some(streaming_stats) = &final_stats.streaming_stats {
                    sqlx::query(
                        "INSERT INTO stream_metrics (
                            tenant_id, session_id, request_id, created_at,
                            time_to_first_token_ms, total_chunks, streaming_duration_ms,
                            avg_chunk_latency_ms, p50_chunk_latency_ms, p95_chunk_latency_ms,
                            p99_chunk_latency_ms, max_chunk_latency_ms, min_chunk_latency_ms
                        ) VALUES ($1, $2, $3, NOW(), $4, $5, $6, $7, $8, $9, $10, $11, $12)
                        ON CONFLICT (tenant_id, created_at, session_id) DO NOTHING"
                    )
                    .bind(tenant_id.0)
                    .bind(&session_id)
                    .bind("") // request_id
                    .bind(streaming_stats.time_to_first_token_ms as i64)
                    .bind(streaming_stats.total_chunks as i32)
                    .bind(streaming_stats.streaming_duration_ms as i64)
                    .bind(streaming_stats.avg_chunk_latency_ms)
                    .bind(streaming_stats.p50_chunk_latency_ms.map(|v| v as i64))
                    .bind(streaming_stats.p95_chunk_latency_ms.map(|v| v as i64))
                    .bind(streaming_stats.p99_chunk_latency_ms.map(|v| v as i64))
                    .bind(streaming_stats.max_chunk_latency_ms as i64)
                    .bind(streaming_stats.min_chunk_latency_ms as i64)
                    .execute(&mut *tx)
                    .await?;
                }

                tx.commit().await?;
            }

            _ => {
                // Handle other event types
            }
        }

        Ok(())
    }

    async fn search(&self, tenant_id: Option<TenantId>, query: SearchQuery) -> Result<SearchResults> {
        let tenant_id = tenant_id.ok_or(Error::TenantRequired)?;

        // Build dynamic query with filters
        let mut sql = "SELECT * FROM sessions WHERE tenant_id = $1".to_string();
        let mut params: Vec<Box<dyn sqlx::Encode<'_, sqlx::Postgres> + Send>> = vec![Box::new(tenant_id.0)];

        if let Some(time_range) = query.time_range {
            sql.push_str(" AND created_at >= $2 AND created_at <= $3");
            params.push(Box::new(time_range.start));
            params.push(Box::new(time_range.end));
        }

        // Execute query and map results
        // ...
    }
}
```

**Alternative: `lunaroute-session-clickhouse`** for massive scale:

```rust
// ClickHouse excels at:
// - 10-100x better compression than PostgreSQL
// - Faster analytics queries on billions of rows
// - Built-in time-series partitioning and TTL
// - Horizontal scaling via sharding
//
// Trade-offs:
// - Not OLTP-friendly (no transactions, eventual consistency)
// - Different SQL dialect
// - No foreign keys or complex joins
//
// Recommendation: Start with TimescaleDB for easier migration.
// Consider ClickHouse if you exceed:
// - 100M+ sessions/month
// - Complex analytics queries taking >1 second
// - Need for long-term data retention with compression
```

### 3. Crate Structure

```
lunaroute/
├── crates/
│   ├── lunaroute-core/              # NEW: Core traits and types
│   │   ├── src/
│   │   │   ├── config_store.rs      # ConfigStore trait
│   │   │   ├── session_store.rs     # SessionStore trait
│   │   │   ├── tenant.rs            # TenantContext, TenantId
│   │   │   ├── events.rs            # SessionEvent (moved from session)
│   │   │   └── types.rs             # Shared types
│   │   └── Cargo.toml
│   │
│   ├── lunaroute-config-file/       # REFACTORED: File-based config
│   │   ├── src/
│   │   │   └── file_store.rs        # Implements ConfigStore
│   │   └── Cargo.toml
│   │
│   ├── lunaroute-config-postgres/   # NEW: Multi-tenant config
│   │   ├── src/
│   │   │   └── postgres_store.rs    # Implements ConfigStore
│   │   └── Cargo.toml
│   │
│   ├── lunaroute-session-sqlite/    # REFACTORED: Local sessions
│   │   ├── src/
│   │   │   ├── sqlite_store.rs      # Implements SessionStore
│   │   │   └── jsonl_writer.rs      # Optional JSONL writer
│   │   └── Cargo.toml
│   │
│   ├── lunaroute-session-postgres/    # NEW: Multi-tenant sessions
│   │   ├── src/
│   │   │   ├── postgres_store.rs    # Implements SessionStore
│   │   │   └── migrations/          # SQL migration files
│   │   └── Cargo.toml
│   │
│   ├── lunaroute-session/           # KEPT: Middleware/coordination
│   │   ├── src/
│   │   │   ├── recorder.rs          # Uses SessionStore trait
│   │   │   ├── worker.rs            # Event batching (store-agnostic)
│   │   │   ├── search.rs            # Generic search interface
│   │   │   └── cleanup.rs           # Retention policies
│   │   └── Cargo.toml
│   │
│   ├── lunaroute-server/            # REFACTORED: Server entry point
│   │   ├── src/
│   │   │   ├── main.rs              # Local mode entry
│   │   │   └── app.rs               # Core app logic (store-agnostic)
│   │   └── Cargo.toml
│   │
│   ├── lunaroute-server-multitenant/# NEW: Multi-tenant server
│   │   ├── src/
│   │   │   ├── main.rs              # Multi-tenant entry
│   │   │   ├── tenant_middleware.rs # Extract tenant from request
│   │   │   ├── tenant_isolation.rs  # Rate limiting, quotas
│   │   │   └── auth.rs              # JWT/API key validation
│   │   └── Cargo.toml
│   │
│   ├── lunaroute-ingress/           # REFACTORED: Add tenant context
│   ├── lunaroute-egress/            # MINIMAL CHANGES
│   ├── lunaroute-routing/           # REFACTORED: Tenant-aware routing
│   ├── lunaroute-ui/                # REFACTORED: Tenant-aware UI
│   └── lunaroute-pii/               # NO CHANGES
│
└── bins/
    ├── lunaroute                    # Local single-tenant binary
    └── lunaroute-cloud              # Multi-tenant hosted binary
```

### 4. Dependency Injection

Refactor `lunaroute-server/src/app.rs` to accept trait-based stores:

```rust
pub struct LunaRouteApp {
    tenant_context: Arc<TenantContext>,
    router: Router,
}

impl LunaRouteApp {
    pub fn new(
        config_store: Arc<dyn ConfigStore>,
        session_store: Arc<dyn SessionStore>,
        tenant_id: Option<TenantId>,
    ) -> Self {
        let tenant_context = Arc::new(TenantContext {
            tenant_id,
            config_store,
            session_store,
        });

        let router = Self::build_router(tenant_context.clone());

        Self {
            tenant_context,
            router,
        }
    }

    fn build_router(ctx: Arc<TenantContext>) -> Router {
        Router::new()
            .route("/v1/messages", post(handle_messages))
            .route("/v1/chat/completions", post(handle_chat_completions))
            .layer(Extension(ctx))
    }

    pub async fn run(self, addr: SocketAddr) -> Result<()> {
        axum::Server::bind(&addr)
            .serve(self.router.into_make_service())
            .await?;
        Ok(())
    }
}

// Local mode (main.rs)
#[tokio::main]
async fn main() -> Result<()> {
    let config_store = Arc::new(FileConfigStore::new("config.yaml").await?);
    let session_store = Arc::new(SqliteSessionStore::new("~/.lunaroute/sessions.db").await?);

    let app = LunaRouteApp::new(
        config_store,
        session_store,
        None,  // No tenant_id for single-tenant mode
    );

    app.run("127.0.0.1:8081".parse()?).await
}
```

### 5. Multi-Tenant Server

**`lunaroute-server-multitenant/src/main.rs`**:

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let database_url = std::env::var("DATABASE_URL")?;

    let config_store = Arc::new(PostgresConfigStore::new(&database_url).await?);
    let session_store = Arc::new(PostgresSessionStore::new(&database_url).await?);

    // Multi-tenant router with tenant extraction middleware
    let app = Router::new()
        .route("/v1/messages", post(handle_messages))
        .route("/v1/chat/completions", post(handle_chat_completions))
        .layer(TenantExtractionLayer::new(
            TenantExtractionStrategy::Subdomain,
            config_store.clone(),
            session_store.clone(),
        ))
        .layer(RateLimitLayer::new())  // Per-tenant rate limiting
        .layer(AuthLayer::new());       // JWT/API key validation

    let addr = "0.0.0.0:8081".parse()?;
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;

    Ok(())
}
```

**Tenant Extraction Middleware**:

```rust
// lunaroute-server-multitenant/src/tenant_middleware.rs

#[derive(Clone)]
pub enum TenantExtractionStrategy {
    Subdomain,              // tenant1.lunaroute.com
    Header(String),         // X-Tenant-Id header
    JwtClaim(String),       // JWT with tenant_id claim
    PathPrefix,             // /tenant1/v1/messages
    ApiKey,                 // API key -> tenant mapping
}

pub struct TenantExtractionLayer {
    strategy: TenantExtractionStrategy,
    config_store: Arc<dyn ConfigStore>,
    session_store: Arc<dyn SessionStore>,
}

impl TenantExtractionLayer {
    pub fn new(
        strategy: TenantExtractionStrategy,
        config_store: Arc<dyn ConfigStore>,
        session_store: Arc<dyn SessionStore>,
    ) -> Self {
        Self {
            strategy,
            config_store,
            session_store,
        }
    }
}

impl<S> Layer<S> for TenantExtractionLayer {
    type Service = TenantExtractionMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        TenantExtractionMiddleware {
            inner,
            strategy: self.strategy.clone(),
            config_store: self.config_store.clone(),
            session_store: self.session_store.clone(),
        }
    }
}

#[derive(Clone)]
pub struct TenantExtractionMiddleware<S> {
    inner: S,
    strategy: TenantExtractionStrategy,
    config_store: Arc<dyn ConfigStore>,
    session_store: Arc<dyn SessionStore>,
}

impl<S, B> Service<Request<B>> for TenantExtractionMiddleware<S>
where
    S: Service<Request<B>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<B>) -> Self::Future {
        let strategy = self.strategy.clone();
        let config_store = self.config_store.clone();
        let session_store = self.session_store.clone();
        let mut inner = self.inner.clone();

        Box::pin(async move {
            // Extract tenant_id based on strategy
            let tenant_id = match &strategy {
                TenantExtractionStrategy::Subdomain => {
                    extract_from_subdomain(req.uri().host())?
                }
                TenantExtractionStrategy::Header(header_name) => {
                    extract_from_header(&req, header_name)?
                }
                TenantExtractionStrategy::JwtClaim(claim_name) => {
                    extract_from_jwt(&req, claim_name)?
                }
                TenantExtractionStrategy::PathPrefix => {
                    extract_from_path(req.uri().path())?
                }
                TenantExtractionStrategy::ApiKey => {
                    extract_from_api_key(&req, &config_store).await?
                }
            };

            // Create tenant context
            let tenant_context = Arc::new(TenantContext {
                tenant_id: Some(tenant_id),
                config_store: config_store.clone(),
                session_store: session_store.clone(),
            });

            // Inject into request extensions
            req.extensions_mut().insert(tenant_context);

            // Continue with request
            inner.call(req).await
        })
    }
}

fn extract_from_subdomain(host: Option<&str>) -> Result<TenantId> {
    let host = host.ok_or(Error::MissingHost)?;

    // Parse subdomain from host (e.g., "tenant1.lunaroute.com" -> "tenant1")
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() < 3 {
        return Err(Error::InvalidTenant);
    }

    let subdomain = parts[0];

    // Look up tenant_id from subdomain (cached in memory)
    // In production, maintain a subdomain -> tenant_id mapping
    let tenant_id = TenantId::from_subdomain(subdomain)?;

    Ok(tenant_id)
}

fn extract_from_header(req: &Request<B>, header_name: &str) -> Result<TenantId> {
    let header_value = req
        .headers()
        .get(header_name)
        .ok_or(Error::MissingTenantHeader)?
        .to_str()
        .map_err(|_| Error::InvalidTenantHeader)?;

    let tenant_id = TenantId::from_string(header_value)?;
    Ok(tenant_id)
}
```

### 6. Shared Business Logic

The following crates remain largely unchanged and work with both deployment models:

**Shared (no changes needed):**
- **`lunaroute-routing`**: Routing logic operates on `TenantContext`
- **`lunaroute-egress`**: Provider clients don't care about tenants
- **`lunaroute-ingress`**: Request parsing is tenant-independent
- **`lunaroute-pii`**: PII detection is tenant-independent

**Minimal changes (inject TenantContext):**
- **`lunaroute-server/app.rs`**: Accept stores as constructor parameters
- **Session recording**: Use `SessionStore` trait instead of concrete SQLite
- **Config loading**: Use `ConfigStore` trait instead of file reading

### 7. Feature Flags

Use Cargo features to compile different variants:

```toml
# Cargo.toml (workspace root)

[features]
default = ["local"]
local = ["lunaroute-config-file", "lunaroute-session-sqlite"]
multitenant = ["lunaroute-config-postgres", "lunaroute-session-postgres"]
all-stores = ["local", "multitenant"]  # For testing

[[bin]]
name = "lunaroute"
path = "crates/lunaroute-server/src/main.rs"
required-features = ["local"]

[[bin]]
name = "lunaroute-cloud"
path = "crates/lunaroute-server-multitenant/src/main.rs"
required-features = ["multitenant"]
```

Build commands:

```bash
# Build local version
cargo build --release --bin lunaroute

# Build multi-tenant version
cargo build --release --bin lunaroute-cloud --features multitenant

# Build both
cargo build --release --features all-stores
```

## Migration Path

### Phase 1: Abstraction (Non-Breaking)

1. Create `lunaroute-core` with traits
2. Move current implementations to implement traits with `tenant_id: Option<TenantId>` (always `None`)
3. Refactor `lunaroute-server` to use trait-based stores
4. **Result**: Code still works exactly the same, but now trait-based

**Changes:**
- No user-facing changes
- No config changes
- No API changes
- Tests continue to pass

### Phase 2: Multi-Tenant Implementations

1. Create `lunaroute-config-postgres` implementing `ConfigStore`
2. Create `lunaroute-session-postgres` implementing `SessionStore`
3. Add schema migration scripts for tenant tables
4. Write integration tests for multi-tenant stores

**Result**: Multi-tenant stores available but not yet used in production

### Phase 3: Multi-Tenant Server

1. Create `lunaroute-server-multitenant` crate
2. Implement tenant extraction middleware
3. Add tenant isolation (rate limiting, quotas)
4. Create separate binary: `lunaroute-cloud`
5. Deploy multi-tenant version alongside local version

**Result**: Both versions coexist, serving different use cases

### Phase 4: Production Rollout

1. Set up PostgreSQL + TimescaleDB cluster
2. Deploy `lunaroute-cloud` with tenant management UI
3. Migrate pilot customers to multi-tenant version
4. Monitor performance and iterate
5. Scale horizontally as needed

## Database Recommendations

### For Configuration: PostgreSQL with JSONB

**Why PostgreSQL:**
- JSONB column for flexible config schema
- Row-level security for tenant isolation
- Triggers for config versioning/audit log
- LISTEN/NOTIFY for real-time config updates
- Mature, battle-tested, excellent tooling

**Schema Design:**
```sql
-- Tenant configs with versioning
CREATE TABLE tenant_configs (
    tenant_id UUID PRIMARY KEY,
    config JSONB NOT NULL,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW(),
    version INT NOT NULL DEFAULT 1
);

-- Row-level security
ALTER TABLE tenant_configs ENABLE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON tenant_configs
    FOR ALL
    USING (tenant_id = current_setting('app.current_tenant_id')::UUID);
```

### For Session Data: TimescaleDB vs ClickHouse

#### TimescaleDB (Recommended Starting Point)

**Pros:**
- PostgreSQL extension (familiar SQL syntax)
- Automatic time-series partitioning (chunks)
- Continuous aggregates for dashboards
- Built-in retention policies
- Excellent query performance for time-based filters
- Easy migration from SQLite schema

**Cons:**
- Not as efficient as ClickHouse at extreme scale (1B+ rows)
- Higher storage costs than ClickHouse

**When to use:**
- Up to 100M sessions/month
- Need OLTP capabilities (transactions, updates)
- Team familiar with PostgreSQL
- Want simple deployment (single database for config + sessions)

#### ClickHouse (For Massive Scale)

**Pros:**
- 10-100x better compression than PostgreSQL
- Faster analytics queries on billions of rows
- Built-in time-series partitioning and TTL
- Excellent horizontal scaling via sharding
- Columnar storage optimized for analytics

**Cons:**
- Different SQL dialect (learning curve)
- No transactions or foreign keys
- Eventual consistency (not OLTP-friendly)
- More complex deployment and operations

**When to use:**
- 100M+ sessions/month
- Complex analytics queries taking >1 second on TimescaleDB
- Need for long-term data retention with compression
- Analytics workload dominates over OLTP

**Recommendation**: Start with TimescaleDB for easier migration from SQLite. Consider ClickHouse if you hit scale issues or need ultra-fast analytics on massive datasets.

### Hybrid Approach

For best of both worlds:

1. **PostgreSQL**: Configuration and tenant metadata
2. **PostgreSQL (with TimescaleDB)**: Recent session data (last 30 days) for real-time dashboards
3. **ClickHouse**: Historical session data (30+ days) for long-term analytics
4. **Automatic archival**: Move old data from PostgreSQL to ClickHouse

```rust
// Hybrid session store
pub struct HybridSessionStore {
    postgres: Arc<PostgresSessionStore>,
    clickhouse: Arc<ClickHouseSessionStore>,
    archive_threshold_days: u32,
}

#[async_trait]
impl SessionStore for HybridSessionStore {
    async fn write_event(&self, tenant_id: Option<TenantId>, event: SessionEvent) -> Result<()> {
        // Always write to PostgreSQL for recent data
        self.postgres.write_event(tenant_id, event).await
    }

    async fn search(&self, tenant_id: Option<TenantId>, query: SearchQuery) -> Result<SearchResults> {
        // Route query based on time range
        if query.is_recent(self.archive_threshold_days) {
            self.postgres.search(tenant_id, query).await
        } else {
            self.clickhouse.search(tenant_id, query).await
        }
    }
}
```

## Testing Strategy

### Unit Tests

Mock implementations of traits for isolated testing:

```rust
pub struct MockConfigStore {
    configs: Arc<Mutex<HashMap<Option<TenantId>, ServerConfig>>>,
}

#[async_trait]
impl ConfigStore for MockConfigStore {
    async fn get_config(&self, tenant_id: Option<TenantId>) -> Result<ServerConfig> {
        self.configs
            .lock()
            .await
            .get(&tenant_id)
            .cloned()
            .ok_or(Error::ConfigNotFound)
    }
}

// Use in tests
#[tokio::test]
async fn test_tenant_routing() {
    let mut mock_store = MockConfigStore::new();
    mock_store.insert(Some(tenant_a), config_a);
    mock_store.insert(Some(tenant_b), config_b);

    let app = LunaRouteApp::new(
        Arc::new(mock_store),
        Arc::new(MockSessionStore::new()),
        Some(tenant_a),
    );

    // Test that routing uses tenant_a's config
}
```

### Integration Tests

Test both local and multi-tenant modes:

```rust
#[cfg(feature = "local")]
mod local_tests {
    #[tokio::test]
    async fn test_file_config_store() {
        // Test FileConfigStore implementation
    }
}

#[cfg(feature = "multitenant")]
mod multitenant_tests {
    #[tokio::test]
    async fn test_postgres_config_store() {
        // Test PostgresConfigStore implementation
    }

    #[tokio::test]
    async fn test_tenant_isolation() {
        // Ensure tenant A cannot access tenant B's data
    }
}
```

### Property-Based Tests

Ensure both implementations behave identically:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn config_store_roundtrip(config: ServerConfig) {
        // Test that get_config(set_config(x)) == x
        // Works for both FileConfigStore and PostgresConfigStore
    }
}
```

## Performance Considerations

### TimescaleDB Optimization

```sql
-- Partition by tenant and time
SELECT create_hypertable('sessions', 'created_at',
    partitioning_column => 'tenant_id',
    number_partitions => 4,
    chunk_time_interval => INTERVAL '1 day'
);

-- Compression (reduce storage by 10-20x)
ALTER TABLE sessions SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'tenant_id,provider,model_used',
    timescaledb.compress_orderby = 'created_at DESC'
);

-- Auto-compress old chunks
SELECT add_compression_policy('sessions', INTERVAL '7 days');

-- Retention (auto-delete old data)
SELECT add_retention_policy('sessions', INTERVAL '90 days');

-- Continuous aggregates (pre-computed dashboards)
CREATE MATERIALIZED VIEW sessions_daily
WITH (timescaledb.continuous) AS
SELECT
    tenant_id,
    time_bucket('1 day', created_at) AS day,
    COUNT(*) as total_requests,
    SUM(input_tokens) as total_input_tokens,
    SUM(output_tokens) as total_output_tokens,
    AVG(provider_latency_ms) as avg_latency_ms
FROM sessions
GROUP BY tenant_id, day;
```

### Connection Pooling

```rust
// Configure appropriate pool sizes
let pool = PgPoolOptions::new()
    .max_connections(20)              // Per instance
    .min_connections(5)                // Keep warm connections
    .acquire_timeout(Duration::from_secs(5))
    .idle_timeout(Duration::from_secs(600))
    .max_lifetime(Duration::from_secs(1800))
    .connect(&database_url)
    .await?;
```

### Caching Strategy

```rust
// Cache tenant configs in memory with TTL
use moka::future::Cache;

pub struct CachedConfigStore {
    inner: Arc<dyn ConfigStore>,
    cache: Cache<TenantId, ServerConfig>,
}

impl CachedConfigStore {
    pub fn new(inner: Arc<dyn ConfigStore>) -> Self {
        let cache = Cache::builder()
            .max_capacity(10_000)                     // 10k tenants
            .time_to_live(Duration::from_secs(300))  // 5 min TTL
            .build();

        Self { inner, cache }
    }
}

#[async_trait]
impl ConfigStore for CachedConfigStore {
    async fn get_config(&self, tenant_id: Option<TenantId>) -> Result<ServerConfig> {
        if let Some(tenant_id) = tenant_id {
            // Try cache first
            if let Some(config) = self.cache.get(&tenant_id) {
                return Ok(config);
            }
        }

        // Cache miss - fetch from database
        let config = self.inner.get_config(tenant_id).await?;

        if let Some(tenant_id) = tenant_id {
            self.cache.insert(tenant_id, config.clone()).await;
        }

        Ok(config)
    }
}
```

## Security Considerations

### Tenant Isolation

**Row-Level Security (PostgreSQL):**
```sql
ALTER TABLE tenant_configs ENABLE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON tenant_configs
    FOR ALL
    USING (tenant_id = current_setting('app.current_tenant_id')::UUID);

-- Set tenant context for connection
SET app.current_tenant_id = '550e8400-e29b-41d4-a716-446655440000';
```

**Application-Level Isolation:**
```rust
// Always inject tenant_id into queries
let sessions = sqlx::query_as::<_, Session>(
    "SELECT * FROM sessions WHERE tenant_id = $1 AND session_id = $2"
)
.bind(tenant_id.0)
.bind(session_id)
.fetch_one(&pool)
.await?;
```

### Rate Limiting

Per-tenant rate limiting with Redis:

```rust
use tower_governor::{Governor, GovernorConfigBuilder};

pub struct TenantRateLimiter {
    redis: Arc<redis::Client>,
}

impl TenantRateLimiter {
    pub async fn check_limit(&self, tenant_id: TenantId) -> Result<bool> {
        let key = format!("rate_limit:{}", tenant_id);

        // Token bucket algorithm
        let allowed: bool = redis::Script::new(RATE_LIMIT_SCRIPT)
            .key(&key)
            .arg(100)  // Max requests
            .arg(60)   // Per 60 seconds
            .invoke_async(&mut self.redis.get_async_connection().await?)
            .await?;

        Ok(allowed)
    }
}
```

### Authentication

JWT-based authentication with tenant claim:

```rust
pub struct JwtAuth {
    decoding_key: DecodingKey,
}

#[derive(Deserialize)]
struct Claims {
    tenant_id: TenantId,
    sub: String,  // User ID
    exp: usize,   // Expiry
}

impl JwtAuth {
    pub fn validate_token(&self, token: &str) -> Result<TenantId> {
        let token_data = decode::<Claims>(
            token,
            &self.decoding_key,
            &Validation::default(),
        )?;

        Ok(token_data.claims.tenant_id)
    }
}
```

## Benefits Summary

### ✅ Code Reuse
- **95%+ shared** between local and hosted versions
- Single codebase, different entry points
- Shared business logic, routing, PII detection

### ✅ Zero Breaking Changes
- Local users unaffected
- Same file-based config and SQLite
- Same CLI experience

### ✅ Testability
- Mock implementations of traits
- Property-based tests for both stores
- Integration tests for tenant isolation

### ✅ Flexibility
- Swap stores without touching business logic
- Add new storage backends (e.g., MongoDB, DynamoDB)
- Mix and match implementations

### ✅ Performance
- Optimized stores for each use case
- TimescaleDB continuous aggregates
- Connection pooling and caching

### ✅ Scalability
- Horizontal scaling for multi-tenant version
- Time-series partitioning
- Auto-archival to cold storage

### ✅ Maintainability
- Single codebase to maintain
- Clear separation of concerns
- Trait-based design for extensibility

## Next Steps

1. **Create `lunaroute-core` crate** with traits (2-3 days)
2. **Refactor existing code** to implement traits (3-5 days)
3. **Implement PostgreSQL config store** (2-3 days)
4. **Implement PostgreSQL session store** (5-7 days)
5. **Create multi-tenant server** with tenant middleware (3-5 days)
6. **Write integration tests** (3-5 days)
7. **Deploy pilot** multi-tenant version (1-2 weeks)

**Total estimated effort**: 4-6 weeks for full implementation and testing.
