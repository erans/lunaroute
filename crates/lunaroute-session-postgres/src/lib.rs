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

mod config;
mod postgres_session_store;

pub use config::PostgresSessionStoreConfig;
pub use postgres_session_store::PostgresSessionStore;
