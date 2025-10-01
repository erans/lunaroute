//! File-based session storage with compression

use crate::atomic_writer::AtomicWriter;
use crate::compression::{compress, decompress, CompressionAlgorithm};
use crate::rolling_writer::RollingWriter;
use crate::session_index::SessionIndex;
use crate::traits::{
    RetentionPolicy, SessionData, SessionFilter, SessionInfo, SessionMetadata, SessionStore,
    StorageError, StorageResult,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// File-based session store
pub struct FileSessionStore {
    base_path: PathBuf,
    compression: CompressionAlgorithm,
    index: Arc<SessionIndex>,
    max_stream_file_size: u64,
}

impl FileSessionStore {
    /// Validate session ID to prevent path traversal attacks
    fn validate_session_id(id: &str) -> StorageResult<()> {
        // Check for empty or too long IDs
        if id.is_empty() {
            return Err(StorageError::InvalidData("Session ID cannot be empty".into()));
        }

        if id.len() > 255 {
            return Err(StorageError::InvalidData(format!(
                "Session ID too long: {} chars (max 255)",
                id.len()
            )));
        }

        // Check for path traversal characters
        if id.contains("..") || id.contains('/') || id.contains('\\') {
            return Err(StorageError::InvalidData(format!(
                "Invalid session ID '{}': contains path traversal characters",
                id
            )));
        }

        // Only allow alphanumeric, dash, underscore
        let is_valid = id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_');
        if !is_valid {
            return Err(StorageError::InvalidData(format!(
                "Invalid session ID '{}': only alphanumeric, dash, and underscore allowed",
                id
            )));
        }

        Ok(())
    }

    /// Create a new file session store
    pub fn new<P: AsRef<Path>>(base_path: P) -> StorageResult<Self> {
        let base_path = base_path.as_ref().to_path_buf();
        let index_path = base_path.join(".index.json");
        let index = Arc::new(SessionIndex::new(&index_path)?);

        Ok(Self {
            base_path,
            compression: CompressionAlgorithm::Zstd,
            index,
            max_stream_file_size: 10 * 1024 * 1024, // 10MB default
        })
    }

    /// Create with specific compression algorithm
    pub fn with_compression<P: AsRef<Path>>(
        base_path: P,
        compression: CompressionAlgorithm,
    ) -> StorageResult<Self> {
        let mut store = Self::new(base_path)?;
        store.compression = compression;
        Ok(store)
    }

    /// Create with custom stream file size
    pub fn with_stream_file_size<P: AsRef<Path>>(
        base_path: P,
        max_stream_file_size: u64,
    ) -> StorageResult<Self> {
        let mut store = Self::new(base_path)?;
        store.max_stream_file_size = max_stream_file_size;
        Ok(store)
    }

    /// Get path for session directory
    fn session_dir(&self, session_id: &str) -> PathBuf {
        self.base_path.join(session_id)
    }

    /// Get path for metadata file
    fn metadata_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("metadata.json")
    }

    /// Get path for request file
    fn request_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("request.bin")
    }

    /// Get path for response file
    fn response_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("response.bin")
    }

    /// Get path for stream events file
    fn stream_events_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("stream_events.ndjson")
    }

    /// Write compressed data to file
    fn write_compressed(&self, path: &Path, data: &[u8]) -> StorageResult<()> {
        let compressed = compress(data, self.compression)?;
        let mut writer = AtomicWriter::new(path)?;
        writer.write(&compressed)?;
        writer.commit()?;
        Ok(())
    }

    /// Read and decompress data from file
    fn read_compressed(&self, path: &Path) -> StorageResult<Vec<u8>> {
        let compressed = fs::read(path)?;
        decompress(&compressed, self.compression)
    }

    /// Get session size in bytes
    fn session_size(&self, session_id: &str) -> StorageResult<u64> {
        let dir = self.session_dir(session_id);
        if !dir.exists() {
            return Ok(0);
        }

        let mut total = 0;
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                total += entry.metadata()?.len();
            }
        }
        Ok(total)
    }
}

#[async_trait::async_trait]
impl SessionStore for FileSessionStore {
    async fn create_session(&self, id: &str, metadata: SessionMetadata) -> StorageResult<()> {
        // Validate session ID to prevent path traversal
        Self::validate_session_id(id)?;

        let session_dir = self.session_dir(id);
        fs::create_dir_all(&session_dir)?;

        // Write metadata
        let metadata_json = serde_json::to_string_pretty(&metadata)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;

        let mut writer = AtomicWriter::new(self.metadata_path(id))?;
        writer.write(metadata_json.as_bytes())?;
        writer.commit()?;

        // Update index
        self.index.upsert(id, metadata, 0)?;

        Ok(())
    }

    async fn append_request(&self, session_id: &str, request: &[u8]) -> StorageResult<()> {
        // Validate session ID to prevent path traversal
        Self::validate_session_id(session_id)?;

        self.write_compressed(&self.request_path(session_id), request)?;
        Ok(())
    }

    async fn append_response(&self, session_id: &str, response: &[u8]) -> StorageResult<()> {
        // Validate session ID to prevent path traversal
        Self::validate_session_id(session_id)?;

        self.write_compressed(&self.response_path(session_id), response)?;
        Ok(())
    }

    async fn append_stream_event(&self, session_id: &str, event: &[u8]) -> StorageResult<()> {
        // Validate session ID to prevent path traversal
        Self::validate_session_id(session_id)?;

        let path = self.stream_events_path(session_id);

        // Use rolling writer for stream events
        let mut writer = RollingWriter::new(&path, self.max_stream_file_size);
        writer.write(event)?;
        writer.write(b"\n")?;
        writer.flush()?;

        Ok(())
    }

    async fn get_metadata(&self, session_id: &str) -> StorageResult<SessionMetadata> {
        // Validate session ID to prevent path traversal
        Self::validate_session_id(session_id)?;

        // Try index first (O(1) lookup)
        if let Some(metadata) = self.index.get(session_id) {
            return Ok(metadata);
        }

        // Fall back to disk if not in index
        let path = self.metadata_path(session_id);
        if !path.exists() {
            return Err(StorageError::NotFound(session_id.to_string()));
        }

        let content = fs::read_to_string(&path)?;
        let metadata: SessionMetadata = serde_json::from_str(&content)
            .map_err(|e| StorageError::Serialization(format!("Failed to parse metadata: {}", e)))?;

        // Update index for future lookups
        let size = self.session_size(session_id)?;
        self.index.upsert(session_id, metadata.clone(), size)?;

        Ok(metadata)
    }

    async fn list_sessions(&self, filter: SessionFilter) -> StorageResult<Vec<SessionInfo>> {
        // Use index for O(1) lookups instead of directory scanning
        let all_sessions = self.index.list_all();
        let mut sessions = Vec::new();

        for session in all_sessions {
            let metadata = &session.metadata;

            // Apply filters
            if let Some(ref user_id) = filter.user_id
                && metadata.user_id.as_ref() != Some(user_id)
            {
                continue;
            }

            if let Some(ref provider) = filter.provider
                && &metadata.provider != provider
            {
                continue;
            }

            if let Some(ref model) = filter.model
                && &metadata.model != model
            {
                continue;
            }

            if let Some(start_time) = filter.start_time
                && metadata.created_at < start_time
            {
                continue;
            }

            if let Some(end_time) = filter.end_time
                && metadata.created_at > end_time
            {
                continue;
            }

            if !filter.tags.is_empty() {
                let has_all_tags = filter.tags.iter().all(|t| metadata.tags.contains(t));
                if !has_all_tags {
                    continue;
                }
            }

            sessions.push(session);
        }

        // Apply limit
        if let Some(limit) = filter.limit {
            sessions.truncate(limit);
        }

        Ok(sessions)
    }

    async fn read_session(&self, session_id: &str) -> StorageResult<SessionData> {
        // Validate session ID to prevent path traversal
        Self::validate_session_id(session_id)?;

        let metadata = self.get_metadata(session_id).await?;

        let request = self.read_compressed(&self.request_path(session_id))?;

        let response = if self.response_path(session_id).exists() {
            Some(self.read_compressed(&self.response_path(session_id))?)
        } else {
            None
        };

        // Read stream events from all rolling files
        let stream_events_path = self.stream_events_path(session_id);
        let stream_events = if stream_events_path.with_extension("ndjson.0").exists()
            || stream_events_path.exists() {
            RollingWriter::read_all(&stream_events_path)?
        } else {
            Vec::new()
        };

        Ok(SessionData {
            metadata,
            request,
            response,
            stream_events,
        })
    }

    async fn delete_session(&self, session_id: &str) -> StorageResult<()> {
        // Validate session ID to prevent path traversal
        Self::validate_session_id(session_id)?;

        let session_dir = self.session_dir(session_id);
        if session_dir.exists() {
            fs::remove_dir_all(&session_dir)?;
        }

        // Remove from index
        self.index.remove(session_id)?;

        Ok(())
    }

    async fn prune(&self, retention: RetentionPolicy) -> StorageResult<u64> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let max_age = now - retention.max_age_seconds as i64;
        let mut deleted_count = 0;

        let sessions = self.list_sessions(SessionFilter::default()).await?;

        for session in sessions {
            let should_delete = session.metadata.created_at < max_age;

            if should_delete {
                self.delete_session(&session.id).await?;
                deleted_count += 1;
            }
        }

        Ok(deleted_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_metadata(id: &str) -> SessionMetadata {
        SessionMetadata {
            id: id.to_string(),
            created_at: 1000000,
            model: "gpt-4".to_string(),
            provider: "openai".to_string(),
            user_id: Some("user_123".to_string()),
            tags: vec!["test".to_string()],
            total_tokens: 100,
            total_cost: Some(0.01),
        }
    }

    #[tokio::test]
    async fn test_create_and_get_session() {
        let temp_dir = TempDir::new().unwrap();
        let store = FileSessionStore::new(temp_dir.path()).unwrap();

        let metadata = create_test_metadata("session_1");
        store.create_session("session_1", metadata.clone()).await.unwrap();

        let loaded = store.get_metadata("session_1").await.unwrap();
        assert_eq!(loaded.id, "session_1");
        assert_eq!(loaded.model, "gpt-4");
    }

    #[tokio::test]
    async fn test_append_request_and_response() {
        let temp_dir = TempDir::new().unwrap();
        let store = FileSessionStore::new(temp_dir.path()).unwrap();

        let metadata = create_test_metadata("session_2");
        store.create_session("session_2", metadata).await.unwrap();

        let request_data = b"test request data";
        let response_data = b"test response data";

        store.append_request("session_2", request_data).await.unwrap();
        store.append_response("session_2", response_data).await.unwrap();

        let session = store.read_session("session_2").await.unwrap();
        assert_eq!(session.request, request_data);
        assert_eq!(session.response.unwrap(), response_data);
    }

    #[tokio::test]
    async fn test_append_stream_events() {
        let temp_dir = TempDir::new().unwrap();
        let store = FileSessionStore::new(temp_dir.path()).unwrap();

        let metadata = create_test_metadata("session_3");
        store.create_session("session_3", metadata).await.unwrap();

        // Need a request for read_session to work
        store.append_request("session_3", b"request").await.unwrap();

        store.append_stream_event("session_3", b"event1").await.unwrap();
        store.append_stream_event("session_3", b"event2").await.unwrap();
        store.append_stream_event("session_3", b"event3").await.unwrap();

        let session = store.read_session("session_3").await.unwrap();
        assert_eq!(session.stream_events.len(), 3);
        assert_eq!(session.stream_events[0], b"event1");
        assert_eq!(session.stream_events[1], b"event2");
    }

    #[tokio::test]
    async fn test_list_sessions_with_filter() {
        let temp_dir = TempDir::new().unwrap();
        let store = FileSessionStore::new(temp_dir.path()).unwrap();

        // Create multiple sessions
        for i in 1..=3 {
            let mut metadata = create_test_metadata(&format!("session_{}", i));
            metadata.provider = if i == 1 { "openai".to_string() } else { "anthropic".to_string() };
            store.create_session(&format!("session_{}", i), metadata).await.unwrap();
        }

        // Filter by provider
        let filter = SessionFilter {
            provider: Some("openai".to_string()),
            ..Default::default()
        };

        let sessions = store.list_sessions(filter).await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "session_1");
    }

    #[tokio::test]
    async fn test_delete_session() {
        let temp_dir = TempDir::new().unwrap();
        let store = FileSessionStore::new(temp_dir.path()).unwrap();

        let metadata = create_test_metadata("session_4");
        store.create_session("session_4", metadata).await.unwrap();

        assert!(store.get_metadata("session_4").await.is_ok());

        store.delete_session("session_4").await.unwrap();

        assert!(store.get_metadata("session_4").await.is_err());
    }

    #[tokio::test]
    async fn test_prune_old_sessions() {
        let temp_dir = TempDir::new().unwrap();
        let store = FileSessionStore::new(temp_dir.path()).unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // Create old session
        let mut old_metadata = create_test_metadata("old_session");
        old_metadata.created_at = now - 100000;
        store.create_session("old_session", old_metadata).await.unwrap();

        // Create recent session
        let mut recent_metadata = create_test_metadata("recent_session");
        recent_metadata.created_at = now - 100;
        store.create_session("recent_session", recent_metadata).await.unwrap();

        // Prune sessions older than 1000 seconds
        let retention = RetentionPolicy {
            max_age_seconds: 1000,
            max_sessions: None,
            max_total_size_bytes: None,
        };

        let deleted = store.prune(retention).await.unwrap();
        assert_eq!(deleted, 1);

        // Old session should be gone
        assert!(store.get_metadata("old_session").await.is_err());
        // Recent session should still exist
        assert!(store.get_metadata("recent_session").await.is_ok());
    }

    #[tokio::test]
    async fn test_compression_algorithms() {
        let temp_dir = TempDir::new().unwrap();

        // Test with different compression algorithms
        let algorithms = vec![
            CompressionAlgorithm::Zstd,
            CompressionAlgorithm::Lz4,
            CompressionAlgorithm::None,
        ];

        for algo in algorithms {
            let store = FileSessionStore::with_compression(temp_dir.path(), algo).unwrap();
            let metadata = create_test_metadata("test_compression");
            store.create_session("test_compression", metadata).await.unwrap();

            let data = b"Test data for compression";
            store.append_request("test_compression", data).await.unwrap();

            let session = store.read_session("test_compression").await.unwrap();
            assert_eq!(session.request, data);

            store.delete_session("test_compression").await.unwrap();
        }
    }

    #[tokio::test]
    async fn test_session_id_validation_path_traversal() {
        let temp_dir = TempDir::new().unwrap();
        let store = FileSessionStore::new(temp_dir.path()).unwrap();
        let metadata = create_test_metadata("invalid");

        // Test path traversal attempts
        let invalid_ids = vec![
            "../etc/passwd",
            "../../secret",
            "session/../../../etc",
            "session/../../test",
            "./test",
        ];

        for id in invalid_ids {
            let result = store.create_session(id, metadata.clone()).await;
            assert!(result.is_err(), "Should reject path traversal ID: {}", id);
        }
    }

    #[tokio::test]
    async fn test_session_id_validation_invalid_chars() {
        let temp_dir = TempDir::new().unwrap();
        let store = FileSessionStore::new(temp_dir.path()).unwrap();
        let metadata = create_test_metadata("invalid");

        // Test invalid characters
        let invalid_ids = vec![
            "session/id",
            "session\\id",
            "session id",
            "session@id",
            "session#id",
        ];

        for id in invalid_ids {
            let result = store.create_session(id, metadata.clone()).await;
            assert!(result.is_err(), "Should reject invalid chars in ID: {}", id);
        }
    }

    #[tokio::test]
    async fn test_session_id_validation_valid() {
        let temp_dir = TempDir::new().unwrap();
        let store = FileSessionStore::new(temp_dir.path()).unwrap();
        let metadata = create_test_metadata("valid_123");

        // Test valid IDs
        let valid_ids = vec![
            "session_123",
            "session-456",
            "ABC_xyz_123",
            "test-session_001",
        ];

        for id in valid_ids {
            let mut meta = metadata.clone();
            meta.id = id.to_string();
            let result = store.create_session(id, meta).await;
            assert!(result.is_ok(), "Should accept valid ID: {}", id);
            store.delete_session(id).await.unwrap();
        }
    }

    #[tokio::test]
    async fn test_session_id_validation_empty() {
        let temp_dir = TempDir::new().unwrap();
        let store = FileSessionStore::new(temp_dir.path()).unwrap();
        let metadata = create_test_metadata("empty");

        let result = store.create_session("", metadata).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_id_validation_too_long() {
        let temp_dir = TempDir::new().unwrap();
        let store = FileSessionStore::new(temp_dir.path()).unwrap();
        let metadata = create_test_metadata("toolong");

        let long_id = "a".repeat(300);
        let result = store.create_session(&long_id, metadata).await;
        assert!(result.is_err());
    }
}
