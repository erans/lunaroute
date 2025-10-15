//! Session store factory
//!
//! Factory functions to create SessionStore implementations based on configuration.

use lunaroute_core::{Result, session_store::SessionStore};
use lunaroute_session::config::SessionRecordingConfig;
use std::sync::Arc;

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
    #[cfg(feature = "postgres")]
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

    // Fall back to SQLite + JSONL (single-tenant mode)
    if config.is_sqlite_enabled() || config.is_jsonl_enabled() {
        // Determine paths from config
        let db_path = config
            .sqlite
            .as_ref()
            .map(|s| s.path.clone())
            .unwrap_or_else(|| std::path::PathBuf::from("~/.lunaroute/sessions.db"));

        let jsonl_dir = config
            .jsonl
            .as_ref()
            .map(|j| j.directory.clone())
            .unwrap_or_else(|| std::path::PathBuf::from("~/.lunaroute/sessions"));

        tracing::info!(
            "Initializing SQLite session store (db={:?}, jsonl={:?})",
            db_path,
            jsonl_dir
        );

        let store = lunaroute_session_sqlite::SqliteSessionStore::new(&db_path, &jsonl_dir).await?;

        return Ok(Arc::new(store));
    }

    #[cfg(not(feature = "postgres"))]
    {
        Err(lunaroute_core::Error::Config(
            "No session store writer enabled in configuration. \
            Enable postgres feature for multi-tenant mode or configure sqlite/jsonl for single-tenant mode."
                .to_string(),
        ))
    }

    #[cfg(feature = "postgres")]
    {
        Err(lunaroute_core::Error::Config(
            "No session store writer enabled in configuration".to_string(),
        ))
    }
}
