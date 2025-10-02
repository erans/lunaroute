//! LunaRoute Session Recording
//!
//! This crate provides session recording capabilities:
//! - Request/response recording
//! - Stream event recording
//! - Session management
//! - Async multi-writer recording (v2)
//! - Disk space management with retention policies
//! - Automatic cleanup and compression

pub mod recorder;
pub mod session;
pub mod recording_provider;

// V2 async recording system
pub mod events;
pub mod writer;
pub mod jsonl_writer;
pub mod config;
pub mod cleanup;

#[cfg(feature = "sqlite-writer")]
pub mod sqlite_writer;

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
pub use config::{SessionRecordingConfig, JsonlConfig, SqliteConfig, WorkerConfig, RetentionPolicy};
pub use cleanup::{
    CleanupError, CleanupResult, CleanupStats, CleanupTask, DiskUsage,
    calculate_disk_usage, execute_cleanup, compress_session_file, delete_session_file,
    spawn_cleanup_task,
};

#[cfg(feature = "sqlite-writer")]
pub use sqlite_writer::SqliteWriter;
