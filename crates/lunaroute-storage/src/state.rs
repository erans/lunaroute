//! In-memory state storage with periodic persistence

use crate::atomic_writer::AtomicWriter;
use crate::traits::{StateStore, StorageError, StorageResult};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};

/// In-memory state store with periodic persistence
pub struct FileStateStore {
    path: PathBuf,
    state: Arc<RwLock<HashMap<String, Vec<u8>>>>,
    persist_interval: Duration,
}

impl FileStateStore {
    /// Create a new file state store
    pub async fn new<P: AsRef<Path>>(path: P) -> StorageResult<Self> {
        let path = path.as_ref().to_path_buf();
        let state = Arc::new(RwLock::new(HashMap::new()));

        // Load existing state if present
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let loaded: HashMap<String, Vec<u8>> = serde_json::from_str(&content)
                .map_err(|e| StorageError::Serialization(format!("Failed to load state: {}", e)))?;
            *state.write().await = loaded;
        }

        Ok(Self {
            path,
            state,
            persist_interval: Duration::from_secs(60), // Default: persist every minute
        })
    }

    /// Create with custom persist interval
    pub async fn with_persist_interval<P: AsRef<Path>>(
        path: P,
        persist_interval: Duration,
    ) -> StorageResult<Self> {
        let mut store = Self::new(path).await?;
        store.persist_interval = persist_interval;
        Ok(store)
    }

    /// Start background persistence task
    pub fn start_auto_persist(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let store = Arc::clone(&self);
        let interval_duration = self.persist_interval;

        tokio::spawn(async move {
            let mut ticker = interval(interval_duration);
            loop {
                ticker.tick().await;
                if let Err(e) = store.persist().await {
                    tracing::error!("Failed to persist state: {}", e);
                }
            }
        })
    }

    /// Serialize state for persistence
    async fn serialize_state(&self) -> StorageResult<String> {
        let state = self.state.read().await;
        serde_json::to_string_pretty(&*state)
            .map_err(|e| StorageError::Serialization(format!("Failed to serialize state: {}", e)))
    }
}

#[async_trait::async_trait]
impl StateStore for FileStateStore {
    async fn get(&self, key: &str) -> StorageResult<Option<Vec<u8>>> {
        let state = self.state.read().await;
        Ok(state.get(key).cloned())
    }

    async fn set(&self, key: &str, value: Vec<u8>) -> StorageResult<()> {
        let mut state = self.state.write().await;
        state.insert(key.to_string(), value);
        Ok(())
    }

    async fn delete(&self, key: &str) -> StorageResult<()> {
        let mut state = self.state.write().await;
        state.remove(key);
        Ok(())
    }

    async fn exists(&self, key: &str) -> StorageResult<bool> {
        let state = self.state.read().await;
        Ok(state.contains_key(key))
    }

    async fn list_keys(&self, prefix: &str) -> StorageResult<Vec<String>> {
        let state = self.state.read().await;
        let keys: Vec<String> = state
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        Ok(keys)
    }

    async fn increment(&self, key: &str, delta: i64) -> StorageResult<i64> {
        let mut state = self.state.write().await;

        let current = state
            .get(key)
            .and_then(|v| {
                if v.len() == 8 {
                    Some(i64::from_le_bytes(v.as_slice().try_into().unwrap()))
                } else {
                    None
                }
            })
            .unwrap_or(0);

        let new_value = current + delta;
        state.insert(key.to_string(), new_value.to_le_bytes().to_vec());

        Ok(new_value)
    }

    async fn get_many(&self, keys: &[String]) -> StorageResult<Vec<Option<Vec<u8>>>> {
        let state = self.state.read().await;
        let values: Vec<Option<Vec<u8>>> = keys
            .iter()
            .map(|k| state.get(k).cloned())
            .collect();
        Ok(values)
    }

    async fn set_many(&self, items: Vec<(String, Vec<u8>)>) -> StorageResult<()> {
        let mut state = self.state.write().await;
        for (key, value) in items {
            state.insert(key, value);
        }
        Ok(())
    }

    async fn persist(&self) -> StorageResult<()> {
        let content = self.serialize_state().await?;

        // Create parent directory if needed
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Write atomically
        let mut writer = AtomicWriter::new(&self.path)?;
        writer.write(content.as_bytes())?;
        writer.commit()?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_get_and_set() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("state.json");
        let store = FileStateStore::new(&path).await.unwrap();

        let value = b"test value".to_vec();
        store.set("key1", value.clone()).await.unwrap();

        let retrieved = store.get("key1").await.unwrap();
        assert_eq!(retrieved, Some(value));
    }

    #[tokio::test]
    async fn test_delete() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("state.json");
        let store = FileStateStore::new(&path).await.unwrap();

        store.set("key1", b"value".to_vec()).await.unwrap();
        assert!(store.exists("key1").await.unwrap());

        store.delete("key1").await.unwrap();
        assert!(!store.exists("key1").await.unwrap());
    }

    #[tokio::test]
    async fn test_list_keys_with_prefix() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("state.json");
        let store = FileStateStore::new(&path).await.unwrap();

        store.set("user:1", b"data1".to_vec()).await.unwrap();
        store.set("user:2", b"data2".to_vec()).await.unwrap();
        store.set("session:1", b"data3".to_vec()).await.unwrap();

        let user_keys = store.list_keys("user:").await.unwrap();
        assert_eq!(user_keys.len(), 2);
        assert!(user_keys.contains(&"user:1".to_string()));
        assert!(user_keys.contains(&"user:2".to_string()));
    }

    #[tokio::test]
    async fn test_increment() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("state.json");
        let store = FileStateStore::new(&path).await.unwrap();

        let value1 = store.increment("counter", 1).await.unwrap();
        assert_eq!(value1, 1);

        let value2 = store.increment("counter", 5).await.unwrap();
        assert_eq!(value2, 6);

        let value3 = store.increment("counter", -2).await.unwrap();
        assert_eq!(value3, 4);
    }

    #[tokio::test]
    async fn test_get_many() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("state.json");
        let store = FileStateStore::new(&path).await.unwrap();

        store.set("key1", b"value1".to_vec()).await.unwrap();
        store.set("key2", b"value2".to_vec()).await.unwrap();

        let keys = vec!["key1".to_string(), "key2".to_string(), "key3".to_string()];
        let values = store.get_many(&keys).await.unwrap();

        assert_eq!(values[0], Some(b"value1".to_vec()));
        assert_eq!(values[1], Some(b"value2".to_vec()));
        assert_eq!(values[2], None);
    }

    #[tokio::test]
    async fn test_set_many() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("state.json");
        let store = FileStateStore::new(&path).await.unwrap();

        let items = vec![
            ("key1".to_string(), b"value1".to_vec()),
            ("key2".to_string(), b"value2".to_vec()),
            ("key3".to_string(), b"value3".to_vec()),
        ];

        store.set_many(items).await.unwrap();

        assert_eq!(store.get("key1").await.unwrap(), Some(b"value1".to_vec()));
        assert_eq!(store.get("key2").await.unwrap(), Some(b"value2".to_vec()));
        assert_eq!(store.get("key3").await.unwrap(), Some(b"value3".to_vec()));
    }

    #[tokio::test]
    async fn test_persist_and_reload() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("state.json");

        // Create store and add data
        {
            let store = FileStateStore::new(&path).await.unwrap();
            store.set("key1", b"value1".to_vec()).await.unwrap();
            store.set("key2", b"value2".to_vec()).await.unwrap();
            store.persist().await.unwrap();
        }

        // Reload from disk
        let store2 = FileStateStore::new(&path).await.unwrap();
        assert_eq!(store2.get("key1").await.unwrap(), Some(b"value1".to_vec()));
        assert_eq!(store2.get("key2").await.unwrap(), Some(b"value2".to_vec()));
    }

    #[tokio::test]
    async fn test_auto_persist() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("state.json");

        let store = Arc::new(
            FileStateStore::with_persist_interval(&path, Duration::from_millis(100))
                .await
                .unwrap(),
        );

        // Start auto-persist
        let handle = store.clone().start_auto_persist();

        // Add some data
        store.set("key1", b"value1".to_vec()).await.unwrap();

        // Wait for auto-persist
        tokio::time::sleep(Duration::from_millis(250)).await;

        // Abort the background task
        handle.abort();

        // Reload and verify
        let store2 = FileStateStore::new(&path).await.unwrap();
        assert_eq!(store2.get("key1").await.unwrap(), Some(b"value1".to_vec()));
    }
}
