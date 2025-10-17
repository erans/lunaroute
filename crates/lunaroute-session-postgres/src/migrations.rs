//! Database migration system for PostgreSQL session store
//!
//! Provides versioned schema migrations with tracking to ensure migrations
//! are applied exactly once and in the correct order.

use lunaroute_core::{Error, Result};
use sqlx::PgPool;
use tracing::{debug, info};

/// Represents a single database migration
#[derive(Debug, Clone)]
pub struct Migration {
    /// Unique version number (must be sequential)
    pub version: i32,
    /// Description of what this migration does
    pub description: &'static str,
    /// SQL to execute for this migration
    pub up_sql: &'static str,
}

/// All migrations in order
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        description: "Create sessions table",
        up_sql: r#"
            CREATE TABLE IF NOT EXISTS sessions (
                tenant_id UUID NOT NULL,
                session_id TEXT NOT NULL,
                request_id TEXT,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

                -- Session metadata
                provider TEXT NOT NULL,
                listener TEXT NOT NULL,
                model_requested TEXT NOT NULL,
                model_used TEXT,

                -- Timing
                started_at TIMESTAMPTZ NOT NULL,
                completed_at TIMESTAMPTZ,
                total_duration_ms BIGINT,
                provider_latency_ms BIGINT,

                -- Token usage
                input_tokens INTEGER DEFAULT 0,
                output_tokens INTEGER DEFAULT 0,
                thinking_tokens INTEGER,
                reasoning_tokens INTEGER,
                cache_read_tokens INTEGER,
                cache_creation_tokens INTEGER,
                audio_input_tokens INTEGER,
                audio_output_tokens INTEGER,
                total_tokens INTEGER DEFAULT 0,

                -- Status
                success BOOLEAN,
                error_message TEXT,
                finish_reason TEXT,

                -- Content
                request_text TEXT,
                response_text TEXT,

                -- Client metadata
                client_ip INET,
                user_agent TEXT,

                -- Streaming metadata
                is_streaming BOOLEAN DEFAULT FALSE,

                PRIMARY KEY (tenant_id, created_at, session_id)
            )
        "#,
    },
    Migration {
        version: 2,
        description: "Create sessions indexes",
        up_sql: r#"
            CREATE INDEX IF NOT EXISTS idx_sessions_tenant_time
            ON sessions(tenant_id, created_at DESC);

            CREATE INDEX IF NOT EXISTS idx_sessions_provider
            ON sessions(tenant_id, provider, created_at DESC);

            CREATE INDEX IF NOT EXISTS idx_sessions_model
            ON sessions(tenant_id, model_used, created_at DESC);

            CREATE INDEX IF NOT EXISTS idx_sessions_session_id
            ON sessions(tenant_id, session_id)
        "#,
    },
    Migration {
        version: 3,
        description: "Create tool_stats table",
        up_sql: r#"
            CREATE TABLE IF NOT EXISTS tool_stats (
                tenant_id UUID NOT NULL,
                session_id TEXT NOT NULL,
                request_id TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

                tool_name TEXT NOT NULL,
                tool_call_id TEXT,
                execution_time_ms BIGINT,
                input_size_bytes BIGINT,
                output_size_bytes BIGINT,
                success BOOLEAN,
                tool_arguments TEXT,

                PRIMARY KEY (tenant_id, created_at, session_id, tool_call_id)
            )
        "#,
    },
    Migration {
        version: 4,
        description: "Create tool_stats indexes",
        up_sql: r#"
            CREATE INDEX IF NOT EXISTS idx_tool_stats_tenant_tool
            ON tool_stats(tenant_id, tool_name, created_at DESC)
        "#,
    },
    Migration {
        version: 5,
        description: "Create stream_metrics table",
        up_sql: r#"
            CREATE TABLE IF NOT EXISTS stream_metrics (
                tenant_id UUID NOT NULL,
                session_id TEXT NOT NULL,
                request_id TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

                time_to_first_token_ms BIGINT NOT NULL,
                total_chunks INTEGER,
                streaming_duration_ms BIGINT,
                avg_chunk_latency_ms DOUBLE PRECISION,
                p50_chunk_latency_ms BIGINT,
                p95_chunk_latency_ms BIGINT,
                p99_chunk_latency_ms BIGINT,
                max_chunk_latency_ms BIGINT,
                min_chunk_latency_ms BIGINT,

                PRIMARY KEY (tenant_id, created_at, session_id)
            )
        "#,
    },
    Migration {
        version: 6,
        description: "Create session_stats table",
        up_sql: r#"
            CREATE TABLE IF NOT EXISTS session_stats (
                id BIGSERIAL PRIMARY KEY,
                tenant_id UUID NOT NULL,
                session_id TEXT NOT NULL,
                request_id TEXT,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

                model_name TEXT NOT NULL,
                pre_processing_ms DOUBLE PRECISION,
                post_processing_ms DOUBLE PRECISION,
                proxy_overhead_ms DOUBLE PRECISION,

                input_tokens INTEGER DEFAULT 0,
                output_tokens INTEGER DEFAULT 0,
                thinking_tokens INTEGER DEFAULT 0,
                reasoning_tokens INTEGER DEFAULT 0,
                cache_read_tokens INTEGER DEFAULT 0,
                cache_creation_tokens INTEGER DEFAULT 0,
                audio_input_tokens INTEGER DEFAULT 0,
                audio_output_tokens INTEGER DEFAULT 0,

                tokens_per_second DOUBLE PRECISION,
                thinking_percentage DOUBLE PRECISION,

                request_size_bytes BIGINT,
                response_size_bytes BIGINT,
                message_count INTEGER,
                content_blocks INTEGER,
                has_tools BOOLEAN DEFAULT FALSE,
                has_refusal BOOLEAN DEFAULT FALSE,
                user_agent TEXT
            )
        "#,
    },
    Migration {
        version: 7,
        description: "Create session_stats indexes",
        up_sql: r#"
            CREATE INDEX IF NOT EXISTS idx_session_stats_tenant_session
            ON session_stats(tenant_id, session_id);

            CREATE INDEX IF NOT EXISTS idx_session_stats_tenant_model
            ON session_stats(tenant_id, model_name, created_at DESC);

            CREATE INDEX IF NOT EXISTS idx_session_stats_tenant_time
            ON session_stats(tenant_id, created_at DESC);

            CREATE INDEX IF NOT EXISTS idx_session_stats_user_agent
            ON session_stats(user_agent)
        "#,
    },
    Migration {
        version: 8,
        description: "Create tool_call_executions table",
        up_sql: r#"
            CREATE TABLE IF NOT EXISTS tool_call_executions (
                id BIGSERIAL PRIMARY KEY,
                tenant_id UUID NOT NULL,
                session_id TEXT NOT NULL,
                request_id TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

                tool_call_id TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                tool_arguments TEXT,
                execution_time_ms BIGINT,
                input_size_bytes BIGINT,
                output_size_bytes BIGINT,
                success BOOLEAN
            )
        "#,
    },
    Migration {
        version: 9,
        description: "Create tool_call_executions indexes",
        up_sql: r#"
            CREATE INDEX IF NOT EXISTS idx_tool_call_executions_tenant_session
            ON tool_call_executions(tenant_id, session_id, created_at DESC);

            CREATE INDEX IF NOT EXISTS idx_tool_call_executions_tenant_tool
            ON tool_call_executions(tenant_id, tool_name, created_at DESC);

            CREATE INDEX IF NOT EXISTS idx_tool_call_executions_request
            ON tool_call_executions(request_id);

            CREATE UNIQUE INDEX IF NOT EXISTS idx_tool_call_executions_unique
            ON tool_call_executions(tenant_id, created_at, session_id, tool_call_id)
        "#,
    },
];

/// Run all pending migrations
///
/// Creates a `schema_migrations` table to track which migrations have been applied,
/// then runs any migrations that haven't been applied yet.
pub async fn run_migrations(pool: &PgPool, timescale_available: bool) -> Result<()> {
    // Create schema_migrations table if it doesn't exist
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            description TEXT NOT NULL,
            applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        "#,
    )
    .execute(pool)
    .await
    .map_err(|e| Error::Database(format!("Failed to create schema_migrations table: {}", e)))?;

    // Get list of applied migrations
    let applied_versions: Vec<i32> = sqlx::query_scalar("SELECT version FROM schema_migrations")
        .fetch_all(pool)
        .await
        .map_err(|e| Error::Database(format!("Failed to fetch applied migrations: {}", e)))?;

    debug!(
        "Found {} applied migrations: {:?}",
        applied_versions.len(),
        applied_versions
    );

    // Run pending migrations in order
    for migration in MIGRATIONS {
        if applied_versions.contains(&migration.version) {
            debug!(
                "Skipping migration {}: {} (already applied)",
                migration.version, migration.description
            );
            continue;
        }

        info!(
            "Applying migration {}: {}",
            migration.version, migration.description
        );

        // Execute the migration
        sqlx::query(migration.up_sql)
            .execute(pool)
            .await
            .map_err(|e| {
                Error::Database(format!(
                    "Failed to apply migration {}: {}",
                    migration.version, e
                ))
            })?;

        // Record that this migration was applied
        sqlx::query(
            "INSERT INTO schema_migrations (version, description) VALUES ($1, $2)
                ON CONFLICT (version) DO NOTHING",
        )
        .bind(migration.version)
        .bind(migration.description)
        .execute(pool)
        .await
        .map_err(|e| {
            Error::Database(format!(
                "Failed to record migration {}: {}",
                migration.version, e
            ))
        })?;

        info!(
            "Successfully applied migration {}: {}",
            migration.version, migration.description
        );
    }

    // After all table migrations, apply TimescaleDB hypertable conversions
    if timescale_available {
        apply_timescaledb_features(pool).await?;
    }

    Ok(())
}

/// Apply TimescaleDB-specific features (hypertables, etc.)
async fn apply_timescaledb_features(pool: &PgPool) -> Result<()> {
    debug!("Applying TimescaleDB features");

    // Convert sessions table to hypertable
    let is_sessions_hypertable: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM timescaledb_information.hypertables WHERE hypertable_name = 'sessions')"
    )
    .fetch_one(pool)
    .await
    .unwrap_or(false);

    if !is_sessions_hypertable {
        info!("Converting sessions table to TimescaleDB hypertable");
        sqlx::query(
            r#"
            SELECT create_hypertable('sessions', 'created_at',
                partitioning_column => 'tenant_id',
                number_partitions => 4,
                chunk_time_interval => INTERVAL '1 day',
                if_not_exists => TRUE
            )
            "#,
        )
        .execute(pool)
        .await
        .map_err(|e| Error::Database(format!("Failed to create sessions hypertable: {}", e)))?;
    }

    // Convert tool_stats table to hypertable
    let is_tool_stats_hypertable: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM timescaledb_information.hypertables WHERE hypertable_name = 'tool_stats')"
    )
    .fetch_one(pool)
    .await
    .unwrap_or(false);

    if !is_tool_stats_hypertable {
        info!("Converting tool_stats table to TimescaleDB hypertable");
        sqlx::query(
            r#"
            SELECT create_hypertable('tool_stats', 'created_at',
                partitioning_column => 'tenant_id',
                number_partitions => 4,
                if_not_exists => TRUE
            )
            "#,
        )
        .execute(pool)
        .await
        .ok();
    }

    // Convert stream_metrics table to hypertable
    let is_stream_metrics_hypertable: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM timescaledb_information.hypertables WHERE hypertable_name = 'stream_metrics')"
    )
    .fetch_one(pool)
    .await
    .unwrap_or(false);

    if !is_stream_metrics_hypertable {
        info!("Converting stream_metrics table to TimescaleDB hypertable");
        sqlx::query(
            r#"
            SELECT create_hypertable('stream_metrics', 'created_at',
                partitioning_column => 'tenant_id',
                number_partitions => 4,
                if_not_exists => TRUE
            )
            "#,
        )
        .execute(pool)
        .await
        .ok();
    }

    // Convert session_stats table to hypertable
    let is_session_stats_hypertable: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM timescaledb_information.hypertables WHERE hypertable_name = 'session_stats')"
    )
    .fetch_one(pool)
    .await
    .unwrap_or(false);

    if !is_session_stats_hypertable {
        info!("Converting session_stats table to TimescaleDB hypertable");
        sqlx::query(
            r#"
            SELECT create_hypertable('session_stats', 'created_at',
                partitioning_column => 'tenant_id',
                number_partitions => 4,
                if_not_exists => TRUE
            )
            "#,
        )
        .execute(pool)
        .await
        .ok();
    }

    // Convert tool_call_executions table to hypertable
    let is_tool_call_executions_hypertable: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM timescaledb_information.hypertables WHERE hypertable_name = 'tool_call_executions')"
    )
    .fetch_one(pool)
    .await
    .unwrap_or(false);

    if !is_tool_call_executions_hypertable {
        info!("Converting tool_call_executions table to TimescaleDB hypertable");
        sqlx::query(
            r#"
            SELECT create_hypertable('tool_call_executions', 'created_at',
                partitioning_column => 'tenant_id',
                number_partitions => 4,
                if_not_exists => TRUE
            )
            "#,
        )
        .execute(pool)
        .await
        .ok();
    }

    Ok(())
}

/// Get the current schema version
pub async fn get_current_version(pool: &PgPool) -> Result<Option<i32>> {
    // Check if schema_migrations table exists
    let table_exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_name = 'schema_migrations'
        )
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(|e| {
        Error::Database(format!(
            "Failed to check for schema_migrations table: {}",
            e
        ))
    })?;

    if !table_exists {
        return Ok(None);
    }

    // Get the highest applied version
    let version: Option<i32> = sqlx::query_scalar("SELECT MAX(version) FROM schema_migrations")
        .fetch_one(pool)
        .await
        .map_err(|e| Error::Database(format!("Failed to get current schema version: {}", e)))?;

    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migrations_are_sequential() {
        let mut expected_version = 1;
        for migration in MIGRATIONS {
            assert_eq!(
                migration.version, expected_version,
                "Migration versions must be sequential"
            );
            expected_version += 1;
        }
    }

    #[test]
    fn test_migrations_have_descriptions() {
        for migration in MIGRATIONS {
            assert!(
                !migration.description.is_empty(),
                "Migration {} must have a description",
                migration.version
            );
        }
    }

    #[test]
    fn test_migrations_have_sql() {
        for migration in MIGRATIONS {
            assert!(
                !migration.up_sql.is_empty(),
                "Migration {} must have SQL",
                migration.version
            );
        }
    }
}
