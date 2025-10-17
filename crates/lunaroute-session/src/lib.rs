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

pub mod pii_redaction;
pub mod recorder;
pub mod recording_provider;
pub mod session;
pub mod tool_mapper;

// V2 async recording system
pub mod cleanup;
pub mod config;
pub mod events;
pub mod jsonl_writer;
pub mod search;
pub mod writer;

#[cfg(feature = "sqlite-writer")]
pub mod sqlite_writer;

#[cfg(feature = "sqlite-writer")]
pub mod import;

pub use recorder::{FileSessionRecorder, RecordedSession, SessionRecorder};
pub use recording_provider::RecordingProvider;
pub use session::{SessionId, SessionMetadata, SessionQuery};
pub use tool_mapper::ToolCallMapper;

// V2 exports
pub use cleanup::{
    CleanupError, CleanupResult, CleanupStats, CleanupTask, DiskUsage, calculate_disk_usage,
    compress_session_file, delete_session_file, execute_cleanup, spawn_cleanup_task,
};
pub use config::{
    CustomPatternConfig, JsonlConfig, PIIConfig, PostgresConfig, RetentionPolicy,
    SessionRecordingConfig, SqliteConfig, WorkerConfig,
};
pub use events::{FinalSessionStats, SessionEvent, SessionStats};
pub use jsonl_writer::{JsonlConfig as JsonlWriterConfig, JsonlWriter};
pub use pii_redaction::SessionPIIRedactor;
pub use search::{
    SearchResults, SessionAggregates, SessionFilter, SessionFilterBuilder, SessionRecord,
    SortOrder, TimeRange,
};
pub use writer::{
    MultiWriterRecorder, RecorderBuilder, RecorderConfig, SessionWriter, WriterError, WriterResult,
    build_from_config,
};

#[cfg(feature = "sqlite-writer")]
pub use sqlite_writer::SqliteWriter;

#[cfg(feature = "sqlite-writer")]
pub use import::{ImportConfig, ImportResult, SessionFile, import_sessions, scan_sessions};
