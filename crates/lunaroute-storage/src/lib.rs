//! LunaRoute Storage Abstraction
//!
//! This crate provides storage abstractions and implementations:
//! - Config store (file-based with hot-reload)
//! - Session store (file-based with compression)
//! - State store (in-memory with persistence)

pub mod atomic_writer;
pub mod compression;
pub mod config;
pub mod encryption;
pub mod session;
pub mod state;
pub mod traits;

pub use atomic_writer::AtomicWriter;
pub use compression::{compress, decompress, CompressionAlgorithm};
pub use encryption::{decrypt, encrypt, generate_key};
pub use traits::{
    ConfigStore, RetentionPolicy, SessionData, SessionFilter, SessionInfo, SessionMetadata,
    SessionStore, StateStore, StorageError, StorageResult,
};
