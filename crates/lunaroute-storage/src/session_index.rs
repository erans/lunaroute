//! Session index for fast lookups

use crate::atomic_writer::AtomicWriter;
use crate::traits::{SessionInfo, SessionMetadata, StorageError, StorageResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Session index for O(1) lookups
#[derive(Clone)]
pub struct SessionIndex {
    path: PathBuf,
    index: Arc<RwLock<HashMap<String, IndexEntry>>>,
}

/// Index entry for a session
#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexEntry {
    metadata: SessionMetadata,
    size_bytes: u64,
}

impl SessionIndex {
    /// Create a new session index
    pub fn new<P: AsRef<Path>>(path: P) -> StorageResult<Self> {
        let path = path.as_ref().to_path_buf();
        let index = if path.exists() {
            let content = fs::read_to_string(&path)?;
            serde_json::from_str(&content)
                .map_err(|e| StorageError::Serialization(format!("Failed to load index: {}", e)))?
        } else {
            HashMap::new()
        };

        Ok(Self {
            path,
            index: Arc::new(RwLock::new(index)),
        })
    }

    /// Add or update a session in the index
    pub fn upsert(&self, session_id: &str, metadata: SessionMetadata, size_bytes: u64) -> StorageResult<()> {
        let mut index = self.index.write().unwrap();
        index.insert(
            session_id.to_string(),
            IndexEntry {
                metadata,
                size_bytes,
            },
        );
        drop(index); // Release lock before persist
        self.persist()
    }

    /// Remove a session from the index
    pub fn remove(&self, session_id: &str) -> StorageResult<()> {
        let mut index = self.index.write().unwrap();
        index.remove(session_id);
        drop(index); // Release lock before persist
        self.persist()
    }

    /// Get session metadata by ID (O(1))
    pub fn get(&self, session_id: &str) -> Option<SessionMetadata> {
        let index = self.index.read().unwrap();
        index.get(session_id).map(|entry| entry.metadata.clone())
    }

    /// Get session info by ID (O(1))
    pub fn get_info(&self, session_id: &str) -> Option<SessionInfo> {
        let index = self.index.read().unwrap();
        index.get(session_id).map(|entry| SessionInfo {
            id: session_id.to_string(),
            metadata: entry.metadata.clone(),
            size_bytes: entry.size_bytes,
        })
    }

    /// Check if a session exists (O(1))
    pub fn exists(&self, session_id: &str) -> bool {
        let index = self.index.read().unwrap();
        index.contains_key(session_id)
    }

    /// List all session IDs
    pub fn list_ids(&self) -> Vec<String> {
        let index = self.index.read().unwrap();
        index.keys().cloned().collect()
    }

    /// List all sessions
    pub fn list_all(&self) -> Vec<SessionInfo> {
        let index = self.index.read().unwrap();
        index
            .iter()
            .map(|(id, entry)| SessionInfo {
                id: id.clone(),
                metadata: entry.metadata.clone(),
                size_bytes: entry.size_bytes,
            })
            .collect()
    }

    /// Get the number of sessions in the index
    pub fn len(&self) -> usize {
        let index = self.index.read().unwrap();
        index.len()
    }

    /// Check if the index is empty
    pub fn is_empty(&self) -> bool {
        let index = self.index.read().unwrap();
        index.is_empty()
    }

    /// Persist the index to disk
    fn persist(&self) -> StorageResult<()> {
        let index = self.index.read().unwrap();
        let content = serde_json::to_string_pretty(&*index)
            .map_err(|e| StorageError::Serialization(format!("Failed to serialize index: {}", e)))?;

        // Create parent directory if needed
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut writer = AtomicWriter::new(&self.path)?;
        writer.write(content.as_bytes())?;
        writer.commit()?;

        Ok(())
    }

    /// Rebuild the index from session directories
    pub fn rebuild<F>(&self, sessions: Vec<SessionInfo>, _on_error: F) -> StorageResult<()>
    where
        F: Fn(&str, &StorageError),
    {
        let mut index = self.index.write().unwrap();
        index.clear();

        for session in sessions {
            index.insert(
                session.id.clone(),
                IndexEntry {
                    metadata: session.metadata,
                    size_bytes: session.size_bytes,
                },
            );
        }

        drop(index); // Release lock before persist
        self.persist()
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

    #[test]
    fn test_session_index_upsert_and_get() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path().join("index.json");
        let index = SessionIndex::new(&index_path).unwrap();

        let metadata = create_test_metadata("session_1");
        index.upsert("session_1", metadata.clone(), 1024).unwrap();

        let retrieved = index.get("session_1").unwrap();
        assert_eq!(retrieved.id, "session_1");
        assert_eq!(retrieved.model, "gpt-4");
    }

    #[test]
    fn test_session_index_remove() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path().join("index.json");
        let index = SessionIndex::new(&index_path).unwrap();

        let metadata = create_test_metadata("session_1");
        index.upsert("session_1", metadata, 1024).unwrap();
        assert!(index.exists("session_1"));

        index.remove("session_1").unwrap();
        assert!(!index.exists("session_1"));
    }

    #[test]
    fn test_session_index_list_all() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path().join("index.json");
        let index = SessionIndex::new(&index_path).unwrap();

        for i in 1..=3 {
            let metadata = create_test_metadata(&format!("session_{}", i));
            index.upsert(&format!("session_{}", i), metadata, 1024).unwrap();
        }

        let sessions = index.list_all();
        assert_eq!(sessions.len(), 3);
    }

    #[test]
    fn test_session_index_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path().join("index.json");

        {
            let index = SessionIndex::new(&index_path).unwrap();
            let metadata = create_test_metadata("session_1");
            index.upsert("session_1", metadata, 1024).unwrap();
        }

        // Reload from disk
        let index2 = SessionIndex::new(&index_path).unwrap();
        assert!(index2.exists("session_1"));
        assert_eq!(index2.len(), 1);
    }

    #[test]
    fn test_session_index_get_info() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path().join("index.json");
        let index = SessionIndex::new(&index_path).unwrap();

        let metadata = create_test_metadata("session_1");
        index.upsert("session_1", metadata, 2048).unwrap();

        let info = index.get_info("session_1").unwrap();
        assert_eq!(info.id, "session_1");
        assert_eq!(info.size_bytes, 2048);
    }

    #[test]
    fn test_session_index_rebuild() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path().join("index.json");
        let index = SessionIndex::new(&index_path).unwrap();

        // Create some sessions
        let sessions = vec![
            SessionInfo {
                id: "session_1".to_string(),
                metadata: create_test_metadata("session_1"),
                size_bytes: 1024,
            },
            SessionInfo {
                id: "session_2".to_string(),
                metadata: create_test_metadata("session_2"),
                size_bytes: 2048,
            },
        ];

        index.rebuild(sessions, |_, _| {}).unwrap();

        assert_eq!(index.len(), 2);
        assert!(index.exists("session_1"));
        assert!(index.exists("session_2"));
    }

    #[test]
    fn test_session_index_list_ids() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path().join("index.json");
        let index = SessionIndex::new(&index_path).unwrap();

        for i in 1..=3 {
            let metadata = create_test_metadata(&format!("session_{}", i));
            index.upsert(&format!("session_{}", i), metadata, 1024).unwrap();
        }

        let ids = index.list_ids();
        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&"session_1".to_string()));
        assert!(ids.contains(&"session_2".to_string()));
        assert!(ids.contains(&"session_3".to_string()));
    }

    #[test]
    fn test_session_index_is_empty() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path().join("index.json");
        let index = SessionIndex::new(&index_path).unwrap();

        assert!(index.is_empty());

        let metadata = create_test_metadata("session_1");
        index.upsert("session_1", metadata, 1024).unwrap();

        assert!(!index.is_empty());
    }
}
