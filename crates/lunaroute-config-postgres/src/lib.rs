//! PostgreSQL-backed configuration storage for multi-tenant LunaRoute deployments
//!
//! This crate implements the `ConfigStore` trait using PostgreSQL for multi-tenant
//! configuration storage with JSONB columns and audit history.
//!
//! # Features
//! - JSONB column for flexible config schema
//! - Version tracking for optimistic concurrency
//! - Audit history for all config changes
//! - PostgreSQL LISTEN/NOTIFY for real-time updates
//! - Automatic schema migrations
//!
//! # Example
//! ```no_run
//! # use lunaroute_config_postgres::PostgresConfigStore;
//! # use lunaroute_core::ConfigStore;
//! # async fn example() -> lunaroute_core::Result<()> {
//! let store = PostgresConfigStore::new("postgres://localhost/lunaroute").await?;
//! # Ok(())
//! # }
//! ```

mod postgres_config_store;

pub use postgres_config_store::PostgresConfigStore;
