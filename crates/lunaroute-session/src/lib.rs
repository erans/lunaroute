//! LunaRoute Session Recording
//!
//! This crate provides session recording capabilities:
//! - Request/response recording
//! - Stream event recording
//! - Session management
//! - Async multi-writer recording (v2)
//! - Disk space management with retention policies
//! - Automatic cleanup and compression
//! - Advanced session search and filtering (SQLite)
//! - Session analytics and aggregation

pub mod recorder;
pub mod session;
pub mod recording_provider;
pub mod pii_redaction;

// V2 async recording system
pub mod events;
pub mod writer;
pub mod jsonl_writer;
pub mod config;
pub mod cleanup;
pub mod search;

#[cfg(feature = "sqlite-writer")]
pub mod sqlite_writer;

#[cfg(feature = "sqlite-writer")]
pub mod import;

pub use recorder::{FileSessionRecorder, RecordedSession, SessionRecorder};
pub use session::{SessionId, SessionMetadata, SessionQuery};
pub use recording_provider::RecordingProvider;

// V2 exports
pub use events::{SessionEvent, SessionStats, FinalSessionStats};
pub use writer::{
    build_from_config, MultiWriterRecorder, RecorderBuilder, RecorderConfig, SessionWriter,
    WriterError, WriterResult,
};
pub use jsonl_writer::{JsonlConfig as JsonlWriterConfig, JsonlWriter};
pub use config::{SessionRecordingConfig, JsonlConfig, SqliteConfig, WorkerConfig, RetentionPolicy, PIIConfig, CustomPatternConfig};
pub use pii_redaction::SessionPIIRedactor;
pub use cleanup::{
    CleanupError, CleanupResult, CleanupStats, CleanupTask, DiskUsage,
    calculate_disk_usage, execute_cleanup, compress_session_file, delete_session_file,
    spawn_cleanup_task,
};
pub use search::{
    SearchResults, SessionAggregates, SessionFilter, SessionFilterBuilder, SessionRecord,
    SortOrder, TimeRange,
};

#[cfg(feature = "sqlite-writer")]
pub use sqlite_writer::SqliteWriter;

#[cfg(feature = "sqlite-writer")]
pub use import::{ImportConfig, ImportResult, SessionFile, import_sessions, scan_sessions};
