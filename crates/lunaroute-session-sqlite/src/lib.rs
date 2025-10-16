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
//! - **Custom writer support** - Implement your own storage backends (S3, CloudWatch, etc.)
//!
//! # Basic Usage
//! ```no_run
//! # use lunaroute_session_sqlite::SqliteSessionStore;
//! # use lunaroute_core::SessionStore;
//! # async fn example() -> lunaroute_core::Result<()> {
//! // Both SQLite and JSONL enabled
//! let store = SqliteSessionStore::new(
//!     Some("~/.lunaroute/sessions.db"),
//!     Some("~/.lunaroute/sessions")
//! ).await?;
//!
//! // Only SQLite enabled (no JSONL files)
//! let store = SqliteSessionStore::new(
//!     Some("~/.lunaroute/sessions.db"),
//!     None::<&str>
//! ).await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Custom Writers
//!
//! You can implement custom storage backends by implementing the `SessionWriter` trait
//! and using the `with_writers()` constructor:
//!
//! ```no_run
//! # use lunaroute_session_sqlite::{SqliteSessionStore, SessionWriter, SessionEvent, RecorderConfig, WriterResult};
//! # use lunaroute_session::sqlite_writer::SqliteWriter;
//! # use std::sync::Arc;
//! # use std::path::Path;
//! # use async_trait::async_trait;
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Example: Custom S3 writer (implement in your private crate)
//! struct S3SessionWriter {
//!     bucket: String,
//! }
//!
//! #[async_trait]
//! impl SessionWriter for S3SessionWriter {
//!     async fn write_event(&self, event: &SessionEvent) -> WriterResult<()> {
//!         // Upload event to S3 bucket
//!         // let json = serde_json::to_string(event)?;
//!         // s3_client.put_object(...).await?;
//!         Ok(())
//!     }
//! }
//!
//! // Create SQLite writer for stats/queries
//! let sqlite = SqliteWriter::new(Path::new("~/.lunaroute/sessions.db")).await?;
//!
//! // Create custom S3 writer for raw event storage
//! let s3_writer = Arc::new(S3SessionWriter {
//!     bucket: "my-lunaroute-sessions".to_string(),
//! });
//!
//! // Combine SQLite (for queries) + S3 (for raw events)
//! let store = SqliteSessionStore::with_writers(
//!     Some(Arc::new(sqlite)),
//!     vec![s3_writer],
//!     RecorderConfig::default(),
//! )?;
//! # Ok(())
//! # }
//! ```

mod sqlite_session_store;

pub use sqlite_session_store::SqliteSessionStore;

// Re-export for convenience
pub use lunaroute_session::{jsonl_writer::JsonlWriter, sqlite_writer::SqliteWriter};

// Re-export for custom writer implementations
pub use lunaroute_session::{
    events::SessionEvent,
    writer::{RecorderConfig, SessionWriter, WriterError, WriterResult},
};
