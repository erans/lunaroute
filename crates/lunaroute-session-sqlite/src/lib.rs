//! SQLite and JSONL session storage for single-tenant LunaRoute deployments
//!
//! This crate implements the `SessionStore` trait using SQLite for metadata
//! and searchable data, combined with JSONL files for complete session events.
//!
//! # Features
//! - SQLite database for fast querying and aggregations
//! - JSONL files for complete event history
//! - Optional encryption at rest (AES-256-GCM)
//! - LRU caching for file handles
//! - Batched writes for performance
//! - Automatic schema migrations
//!
//! # Example
//! ```no_run
//! # use lunaroute_session_sqlite::SqliteSessionStore;
//! # use lunaroute_core::SessionStore;
//! # async fn example() -> lunaroute_core::Result<()> {
//! let store = SqliteSessionStore::new("~/.lunaroute/sessions.db", "~/.lunaroute/sessions").await?;
//! # Ok(())
//! # }
//! ```

mod sqlite_session_store;

pub use sqlite_session_store::SqliteSessionStore;

// Re-export for convenience
pub use lunaroute_session::{
    sqlite_writer::SqliteWriter,
    jsonl_writer::JsonlWriter,
};
