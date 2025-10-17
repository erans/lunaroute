//! PostgreSQL-backed session storage for multi-tenant LunaRoute deployments
//!
//! This crate implements the `SessionStore` trait using PostgreSQL for multi-tenant
//! session storage. TimescaleDB extension support is optional - if available, the store
//! will automatically enable hypertable partitioning for improved time-series performance.
//!
//! # Features
//! - Works with vanilla PostgreSQL (no extensions required)
//! - Optional TimescaleDB hypertable partitioning by tenant_id and time
//! - Optional automatic data compression for old sessions (TimescaleDB only)
//! - Optional built-in retention policies (TimescaleDB only)
//! - Optional continuous aggregates for dashboards (TimescaleDB only)
//! - High-performance queries with or without TimescaleDB
//! - Configurable connection pool settings
//! - Prometheus metrics for monitoring database operations, connection pool health, and migration status
//!
//! # Example
//! ```no_run
//! # use lunaroute_session_postgres::{PostgresSessionStore, PostgresSessionStoreConfig};
//! # use lunaroute_core::SessionStore;
//! # async fn example() -> lunaroute_core::Result<()> {
//! // Works with vanilla PostgreSQL or TimescaleDB
//! let store = PostgresSessionStore::new("postgres://localhost/lunaroute").await?;
//!
//! // Or with custom configuration
//! let config = PostgresSessionStoreConfig::default()
//!     .with_max_connections(50)
//!     .with_min_connections(10);
//! let store = PostgresSessionStore::with_config("postgres://localhost/lunaroute", config).await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Observability
//!
//! The session store provides comprehensive Prometheus metrics for production monitoring:
//!
//! ```no_run
//! # use lunaroute_session_postgres::{PostgresSessionStore, PostgresSessionStoreConfig, SessionStoreMetrics};
//! # async fn example() -> lunaroute_core::Result<()> {
//! // Create metrics collector
//! let metrics = SessionStoreMetrics::new().expect("Failed to create metrics");
//!
//! // Create store with metrics enabled
//! let config = PostgresSessionStoreConfig::default();
//! let store = PostgresSessionStore::with_config_and_metrics(
//!     "postgres://localhost/lunaroute",
//!     config,
//!     Some(metrics.clone())
//! ).await?;
//!
//! // Metrics are automatically recorded for all operations:
//! // - Event writes (by event type, with latency)
//! // - Session retrievals (with success/failure tracking)
//! // - Search and list operations (with latency)
//! // - Connection pool health (total, idle, active connections)
//! // - Migration status (applied count, current version)
//! // - TimescaleDB availability
//!
//! // Export metrics to Prometheus
//! let registry = metrics.registry();
//! // ... integrate with Prometheus exporter
//! # Ok(())
//! # }
//! ```

mod config;
mod metrics;
mod migrations;
mod postgres_session_store;

pub use config::PostgresSessionStoreConfig;
pub use metrics::SessionStoreMetrics;
pub use migrations::{MIGRATIONS, Migration, get_current_version, run_migrations};
pub use postgres_session_store::PostgresSessionStore;
