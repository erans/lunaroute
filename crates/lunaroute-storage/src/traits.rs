//! Storage trait definitions

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Invalid data: {0}")]
    InvalidData(String),

    #[error("Configuration error: {0}")]
    Config(String),
}

pub type StorageResult<T> = Result<T, StorageError>;

/// Configuration store trait for loading and managing configuration
#[async_trait::async_trait]
pub trait ConfigStore: Send + Sync {
    /// Load configuration from storage
    async fn load<T>(&self) -> StorageResult<T>
    where
        T: for<'de> Deserialize<'de> + Send;

    /// Save configuration to storage
    async fn save<T>(&self, config: &T) -> StorageResult<()>
    where
        T: Serialize + Send + Sync;

    /// Watch for configuration changes and call the callback
    async fn watch<F>(&self, callback: F) -> StorageResult<()>
    where
        F: Fn() + Send + Sync + 'static;

    /// Validate configuration without loading
    async fn validate(&self) -> StorageResult<()>;
}

/// Session store trait for storing LLM request/response sessions
#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    /// Create a new session
    async fn create_session(&self, id: &str, metadata: SessionMetadata) -> StorageResult<()>;

    /// Append a request to a session
    async fn append_request(&self, session_id: &str, request: &[u8]) -> StorageResult<()>;

    /// Append a response to a session
    async fn append_response(&self, session_id: &str, response: &[u8]) -> StorageResult<()>;

    /// Append a stream event to a session
    async fn append_stream_event(&self, session_id: &str, event: &[u8]) -> StorageResult<()>;

    /// Get session metadata
    async fn get_metadata(&self, session_id: &str) -> StorageResult<SessionMetadata>;

    /// List sessions with optional filtering
    async fn list_sessions(&self, filter: SessionFilter) -> StorageResult<Vec<SessionInfo>>;

    /// Read a complete session
    async fn read_session(&self, session_id: &str) -> StorageResult<SessionData>;

    /// Delete a session
    async fn delete_session(&self, session_id: &str) -> StorageResult<()>;

    /// Prune old sessions based on retention policy
    async fn prune(&self, retention: RetentionPolicy) -> StorageResult<u64>;
}

/// State store trait for managing runtime state
#[async_trait::async_trait]
pub trait StateStore: Send + Sync {
    /// Get a value from the store
    async fn get(&self, key: &str) -> StorageResult<Option<Vec<u8>>>;

    /// Set a value in the store
    async fn set(&self, key: &str, value: Vec<u8>) -> StorageResult<()>;

    /// Delete a value from the store
    async fn delete(&self, key: &str) -> StorageResult<()>;

    /// Check if a key exists
    async fn exists(&self, key: &str) -> StorageResult<bool>;

    /// List keys matching a prefix
    async fn list_keys(&self, prefix: &str) -> StorageResult<Vec<String>>;

    /// Atomically increment a counter
    async fn increment(&self, key: &str, delta: i64) -> StorageResult<i64>;

    /// Get multiple values at once
    async fn get_many(&self, keys: &[String]) -> StorageResult<Vec<Option<Vec<u8>>>>;

    /// Set multiple values at once
    async fn set_many(&self, items: Vec<(String, Vec<u8>)>) -> StorageResult<()>;

    /// Persist state to durable storage
    async fn persist(&self) -> StorageResult<()>;
}

/// Metadata for a session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// Unique session ID
    pub id: String,

    /// Timestamp when session was created
    pub created_at: i64,

    /// Model used in this session
    pub model: String,

    /// Provider used
    pub provider: String,

    /// User/tenant ID
    pub user_id: Option<String>,

    /// Request tags
    pub tags: Vec<String>,

    /// Total tokens used
    pub total_tokens: u32,

    /// Total cost (if known)
    pub total_cost: Option<f64>,
}

/// Filter for querying sessions
#[derive(Debug, Clone, Default)]
pub struct SessionFilter {
    /// Filter by user ID
    pub user_id: Option<String>,

    /// Filter by provider
    pub provider: Option<String>,

    /// Filter by model
    pub model: Option<String>,

    /// Start time (Unix timestamp)
    pub start_time: Option<i64>,

    /// End time (Unix timestamp)
    pub end_time: Option<i64>,

    /// Filter by tags
    pub tags: Vec<String>,

    /// Maximum number of results
    pub limit: Option<usize>,
}

/// Session information for listing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    /// Session ID
    pub id: String,

    /// Session metadata
    pub metadata: SessionMetadata,

    /// Size of session data in bytes
    pub size_bytes: u64,
}

/// Complete session data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    /// Session metadata
    pub metadata: SessionMetadata,

    /// Request data
    pub request: Vec<u8>,

    /// Response data (if completed)
    pub response: Option<Vec<u8>>,

    /// Stream events (if streaming)
    pub stream_events: Vec<Vec<u8>>,
}

/// Retention policy for pruning sessions
#[derive(Debug, Clone)]
pub struct RetentionPolicy {
    /// Maximum age in seconds
    pub max_age_seconds: u64,

    /// Maximum number of sessions to keep
    pub max_sessions: Option<usize>,

    /// Maximum total size in bytes
    pub max_total_size_bytes: Option<u64>,
}

#[cfg(test)]
mod tests;
