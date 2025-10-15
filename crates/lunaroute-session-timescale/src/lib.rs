//! TimescaleDB-backed session storage for multi-tenant LunaRoute deployments
//!
//! This crate implements the `SessionStore` trait using TimescaleDB (PostgreSQL extension)
//! for multi-tenant session storage with time-series optimization.
//!
//! # Features
//! - Hypertable partitioning by tenant_id and time
//! - Automatic data compression for old sessions
//! - Built-in retention policies
//! - Continuous aggregates for dashboards
//! - High-performance time-series queries
//!
//! # Example
//! ```no_run
//! # use lunaroute_session_timescale::TimescaleSessionStore;
//! # use lunaroute_core::SessionStore;
//! # async fn example() -> lunaroute_core::Result<()> {
//! let store = TimescaleSessionStore::new("postgres://localhost/lunaroute").await?;
//! # Ok(())
//! # }
//! ```

mod timescale_session_store;

pub use timescale_session_store::TimescaleSessionStore;
