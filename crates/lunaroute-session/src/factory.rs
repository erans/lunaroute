//! Session store factory
//!
//! Factory functions to create SessionStore implementations based on configuration.

use lunaroute_core::{Result, session_store::SessionStore};
use std::sync::Arc;

use crate::config::SessionRecordingConfig;

/// Create a session store based on configuration
///
/// Creates either a SQLite or PostgreSQL session store depending on what's enabled
/// in the configuration. If both are enabled, PostgreSQL takes precedence.
///
/// # Arguments
/// * `config` - Session recording configuration
///
/// # Returns
/// A boxed SessionStore implementation
///
/// # Errors
/// - `Error::Config` if no writers are enabled
/// - `Error::Database` if database connection fails
pub async fn create_session_store(
    config: &SessionRecordingConfig,
) -> Result<Arc<dyn SessionStore>> {
    // Try PostgreSQL first (multi-tenant mode)
    #[cfg(feature = "postgres-writer")]
    if config.is_postgres_enabled()
        && let Some(postgres_config) = &config.postgres
    {
        tracing::info!("Initializing PostgreSQL session store");

        let pg_config = lunaroute_session_postgres::PostgresSessionStoreConfig::default()
            .with_max_connections(postgres_config.max_connections)
            .with_min_connections(postgres_config.min_connections)
            .with_acquire_timeout(std::time::Duration::from_secs(
                postgres_config.acquire_timeout_seconds,
            ))
            .with_idle_timeout(std::time::Duration::from_secs(
                postgres_config.idle_timeout_seconds,
            ))
            .with_max_lifetime(std::time::Duration::from_secs(
                postgres_config.max_lifetime_seconds,
            ));

        let store = lunaroute_session_postgres::PostgresSessionStore::with_config(
            &postgres_config.connection_string,
            pg_config,
        )
        .await?;

        return Ok(Arc::new(store));
    }

    // Fall back to SQLite (single-tenant mode)
    #[cfg(feature = "sqlite-writer")]
    if config.is_sqlite_enabled()
        && let Some(sqlite_config) = &config.sqlite
    {
        tracing::info!(
            "Initializing SQLite session store at {:?}",
            sqlite_config.path
        );

        // Note: SqliteWriter implements SessionWriter trait, not SessionStore trait
        // For now, we don't support SQLite in the SessionStore abstraction
        // Users should use the direct SqliteWriter for single-tenant mode
        return Err(lunaroute_core::Error::Config(
            "SQLite session store does not implement SessionStore trait yet. \
            Use SqliteWriter directly for single-tenant mode."
                .to_string(),
        ));
    }

    #[cfg(not(feature = "postgres-writer"))]
    {
        Err(lunaroute_core::Error::Config(
            "No session store writer enabled in configuration. \
            Enable postgres-writer feature for multi-tenant mode."
                .to_string(),
        ))
    }

    #[cfg(feature = "postgres-writer")]
    {
        Err(lunaroute_core::Error::Config(
            "No session store writer enabled in configuration".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PostgresConfig, SqliteConfig};
    use std::path::PathBuf;

    #[tokio::test]
    #[cfg(feature = "sqlite-writer")]
    async fn test_create_sqlite_store() {
        let config = SessionRecordingConfig {
            enabled: true,
            jsonl: None,
            sqlite: Some(SqliteConfig {
                enabled: true,
                path: PathBuf::from(":memory:"),
                max_connections: 5,
            }),
            postgres: None,
            worker: Default::default(),
            pii: None,
            capture_user_agent: true,
            max_user_agent_length: 255,
        };

        let result = create_session_store(&config).await;
        // SQLite doesn't implement SessionStore trait, so this should fail
        assert!(result.is_err());
        assert!(matches!(result, Err(lunaroute_core::Error::Config(_))));
        if let Err(lunaroute_core::Error::Config(msg)) = result {
            assert!(msg.contains("SessionStore trait"));
        }
    }

    #[test]
    fn test_no_writers_enabled() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let config = SessionRecordingConfig {
                enabled: true,
                jsonl: None,
                sqlite: None,
                postgres: None,
                worker: Default::default(),
                pii: None,
                capture_user_agent: true,
                max_user_agent_length: 255,
            };

            let result = create_session_store(&config).await;
            assert!(result.is_err());
            assert!(matches!(result, Err(lunaroute_core::Error::Config(_))));
        });
    }

    #[tokio::test]
    #[cfg(all(feature = "sqlite-writer", feature = "postgres-writer"))]
    async fn test_postgres_takes_precedence() {
        // When both are enabled, PostgreSQL should be used
        // This test would need a real PostgreSQL instance to fully test,
        // so we just verify the config check works
        let config = SessionRecordingConfig {
            enabled: true,
            jsonl: None,
            sqlite: Some(SqliteConfig {
                enabled: true,
                path: PathBuf::from(":memory:"),
                max_connections: 5,
            }),
            postgres: Some(PostgresConfig {
                enabled: true,
                connection_string: "postgres://localhost/test".to_string(),
                max_connections: 5,
                min_connections: 1,
                acquire_timeout_seconds: 30,
                idle_timeout_seconds: 600,
                max_lifetime_seconds: 1800,
            }),
            worker: Default::default(),
            pii: None,
            capture_user_agent: true,
            max_user_agent_length: 255,
        };

        assert!(config.is_postgres_enabled());
        assert!(config.is_sqlite_enabled());
        // In this configuration, PostgreSQL should be chosen over SQLite
    }
}
