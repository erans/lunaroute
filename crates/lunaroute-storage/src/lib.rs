//! LunaRoute Storage Abstraction
//!
//! This crate provides storage abstractions and implementations:
//! - Config store (file-based with hot-reload)
//! - Session store (file-based with compression)
//! - State store (in-memory with persistence)
//! - Buffer pool for memory efficiency
//! - Rolling file writer for streams
//! - Session indexing for fast lookups

pub mod atomic_writer;
pub mod buffer_pool;
pub mod compression;
pub mod config;
pub mod encryption;
pub mod rolling_writer;
pub mod session;
pub mod session_index;
pub mod state;
pub mod traits;

pub use atomic_writer::AtomicWriter;
pub use buffer_pool::BufferPool;
pub use compression::{compress, decompress, CompressionAlgorithm};
pub use config::{ConfigValidator, FileConfigStore, ValidatedConfigStore};
pub use encryption::{decrypt, encrypt, generate_key};
pub use rolling_writer::RollingWriter;
pub use session::FileSessionStore;
pub use session_index::SessionIndex;
pub use state::FileStateStore;
pub use traits::{
    ConfigStore, RetentionPolicy, SessionData, SessionFilter, SessionInfo, SessionMetadata,
    SessionStore, StateStore, StorageError, StorageResult,
};
