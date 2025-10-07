//! File-based configuration storage with hot-reload

use crate::traits::{ConfigStore, StorageError, StorageResult};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Trait for validating configuration values
pub trait ConfigValidator<T> {
    /// Validate a configuration value
    fn validate(&self, config: &T) -> Result<(), String>;
}

/// File-based configuration store
pub struct FileConfigStore {
    path: PathBuf,
    format: ConfigFormat,
    watcher: Arc<Mutex<Option<RecommendedWatcher>>>,
}

/// File-based configuration store with schema validation
pub struct ValidatedConfigStore<T, V>
where
    V: ConfigValidator<T>,
{
    store: FileConfigStore,
    validator: V,
    _phantom: std::marker::PhantomData<T>,
}

/// Configuration file format
#[derive(Debug, Clone, Copy)]
pub enum ConfigFormat {
    /// JSON format
    Json,
    /// YAML format
    Yaml,
    /// TOML format
    Toml,
}

impl FileConfigStore {
    /// Create a new file config store
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        let path = path.as_ref().to_path_buf();
        let format = Self::detect_format(&path);

        Self {
            path,
            format,
            watcher: Arc::new(Mutex::new(None)),
        }
    }

    /// Detect config format from file extension
    fn detect_format(path: &Path) -> ConfigFormat {
        match path.extension().and_then(|s| s.to_str()) {
            Some("json") => ConfigFormat::Json,
            Some("yaml") | Some("yml") => ConfigFormat::Yaml,
            Some("toml") => ConfigFormat::Toml,
            _ => ConfigFormat::Json, // Default
        }
    }

    /// Read config file contents
    fn read_file(&self) -> StorageResult<String> {
        std::fs::read_to_string(&self.path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                StorageError::NotFound(self.path.display().to_string())
            } else {
                StorageError::Io(e)
            }
        })
    }

    /// Parse config based on format
    fn parse<T>(&self, content: &str) -> StorageResult<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        match self.format {
            ConfigFormat::Json => serde_json::from_str(content)
                .map_err(|e| StorageError::Config(format!("JSON parse error: {}", e))),
            ConfigFormat::Yaml => serde_yaml::from_str(content)
                .map_err(|e| StorageError::Config(format!("YAML parse error: {}", e))),
            ConfigFormat::Toml => toml::from_str(content)
                .map_err(|e| StorageError::Config(format!("TOML parse error: {}", e))),
        }
    }

    /// Serialize config based on format
    fn serialize<T>(&self, value: &T) -> StorageResult<String>
    where
        T: Serialize,
    {
        match self.format {
            ConfigFormat::Json => serde_json::to_string_pretty(value)
                .map_err(|e| StorageError::Serialization(format!("JSON serialize error: {}", e))),
            ConfigFormat::Yaml => serde_yaml::to_string(value)
                .map_err(|e| StorageError::Serialization(format!("YAML serialize error: {}", e))),
            ConfigFormat::Toml => toml::to_string_pretty(value)
                .map_err(|e| StorageError::Serialization(format!("TOML serialize error: {}", e))),
        }
    }
}

#[async_trait::async_trait]
impl ConfigStore for FileConfigStore {
    async fn load<T>(&self) -> StorageResult<T>
    where
        T: for<'de> Deserialize<'de> + Send,
    {
        let content = self.read_file()?;
        self.parse(&content)
    }

    async fn save<T>(&self, config: &T) -> StorageResult<()>
    where
        T: Serialize + Send + Sync,
    {
        let content = self.serialize(config)?;

        // Create parent directory if needed
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Write atomically
        use crate::atomic_writer::AtomicWriter;
        let mut writer = AtomicWriter::new(&self.path)?;
        writer.write(content.as_bytes())?;
        writer.commit()?;

        Ok(())
    }

    async fn watch<F>(&self, callback: F) -> StorageResult<()>
    where
        F: Fn() + Send + Sync + 'static,
    {
        let path = self.path.clone();
        let callback = Arc::new(callback);

        // Create watcher
        let mut watcher: RecommendedWatcher = Watcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(_event) = res {
                    callback();
                }
            },
            notify::Config::default(),
        )
        .map_err(|e| StorageError::Config(format!("Watcher error: {}", e)))?;

        // Watch the config file
        watcher
            .watch(&path, RecursiveMode::NonRecursive)
            .map_err(|e| StorageError::Config(format!("Watch error: {}", e)))?;

        // Store watcher to keep it alive (prevents memory leak from std::mem::forget)
        let mut watcher_guard = self
            .watcher
            .lock()
            .map_err(|e| StorageError::Config(format!("Failed to acquire watcher lock: {}", e)))?;
        *watcher_guard = Some(watcher);

        Ok(())
    }

    async fn validate(&self) -> StorageResult<()> {
        // Try to read the file
        let content = self.read_file()?;

        // Try to parse as a generic serde_json::Value to validate syntax
        match self.format {
            ConfigFormat::Json => {
                serde_json::from_str::<serde_json::Value>(&content)
                    .map_err(|e| StorageError::Config(format!("Invalid JSON: {}", e)))?;
            }
            ConfigFormat::Yaml => {
                serde_yaml::from_str::<serde_yaml::Value>(&content)
                    .map_err(|e| StorageError::Config(format!("Invalid YAML: {}", e)))?;
            }
            ConfigFormat::Toml => {
                toml::from_str::<toml::Value>(&content)
                    .map_err(|e| StorageError::Config(format!("Invalid TOML: {}", e)))?;
            }
        }

        Ok(())
    }
}

impl<T, V> ValidatedConfigStore<T, V>
where
    T: for<'de> Deserialize<'de> + Serialize + Send + Sync,
    V: ConfigValidator<T>,
{
    /// Create a new validated config store
    pub fn new<P: AsRef<Path>>(path: P, validator: V) -> Self {
        Self {
            store: FileConfigStore::new(path),
            validator,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Load and validate configuration
    pub async fn load(&self) -> StorageResult<T> {
        let config: T = self.store.load().await?;
        self.validator
            .validate(&config)
            .map_err(|e| StorageError::Config(format!("Validation failed: {}", e)))?;
        Ok(config)
    }

    /// Validate and save configuration
    pub async fn save(&self, config: &T) -> StorageResult<()> {
        self.validator
            .validate(config)
            .map_err(|e| StorageError::Config(format!("Validation failed: {}", e)))?;
        self.store.save(config).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use tempfile::TempDir;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct TestConfig {
        name: String,
        value: i32,
        enabled: bool,
    }

    #[tokio::test]
    async fn test_json_config_store() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.json");

        let store = FileConfigStore::new(&config_path);

        let config = TestConfig {
            name: "test".to_string(),
            value: 42,
            enabled: true,
        };

        // Save
        store.save(&config).await.unwrap();

        // Load
        let loaded: TestConfig = store.load().await.unwrap();
        assert_eq!(config, loaded);
    }

    #[tokio::test]
    async fn test_yaml_config_store() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.yaml");

        let store = FileConfigStore::new(&config_path);

        let config = TestConfig {
            name: "test".to_string(),
            value: 42,
            enabled: true,
        };

        store.save(&config).await.unwrap();
        let loaded: TestConfig = store.load().await.unwrap();
        assert_eq!(config, loaded);
    }

    #[tokio::test]
    async fn test_toml_config_store() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        let store = FileConfigStore::new(&config_path);

        let config = TestConfig {
            name: "test".to_string(),
            value: 42,
            enabled: true,
        };

        store.save(&config).await.unwrap();
        let loaded: TestConfig = store.load().await.unwrap();
        assert_eq!(config, loaded);
    }

    #[tokio::test]
    async fn test_config_validation() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.json");

        let store = FileConfigStore::new(&config_path);

        // Write invalid JSON
        std::fs::write(&config_path, b"{ invalid json }").unwrap();

        // Validation should fail
        assert!(store.validate().await.is_err());

        // Write valid JSON
        std::fs::write(
            &config_path,
            br#"{"name": "test", "value": 42, "enabled": true}"#,
        )
        .unwrap();

        // Validation should succeed
        assert!(store.validate().await.is_ok());
    }

    #[tokio::test]
    async fn test_config_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("nonexistent.json");

        let store = FileConfigStore::new(&config_path);

        let result: StorageResult<TestConfig> = store.load().await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), StorageError::NotFound(_)));
    }

    #[test]
    fn test_format_detection() {
        assert!(matches!(
            FileConfigStore::detect_format(Path::new("config.json")),
            ConfigFormat::Json
        ));
        assert!(matches!(
            FileConfigStore::detect_format(Path::new("config.yaml")),
            ConfigFormat::Yaml
        ));
        assert!(matches!(
            FileConfigStore::detect_format(Path::new("config.yml")),
            ConfigFormat::Yaml
        ));
        assert!(matches!(
            FileConfigStore::detect_format(Path::new("config.toml")),
            ConfigFormat::Toml
        ));
    }

    // Schema validation tests
    struct RangeValidator {
        min_value: i32,
        max_value: i32,
    }

    impl ConfigValidator<TestConfig> for RangeValidator {
        fn validate(&self, config: &TestConfig) -> Result<(), String> {
            if config.value < self.min_value {
                return Err(format!(
                    "Value {} is below minimum {}",
                    config.value, self.min_value
                ));
            }
            if config.value > self.max_value {
                return Err(format!(
                    "Value {} is above maximum {}",
                    config.value, self.max_value
                ));
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_validated_config_store_success() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.json");

        let validator = RangeValidator {
            min_value: 0,
            max_value: 100,
        };
        let store = ValidatedConfigStore::new(&config_path, validator);

        let config = TestConfig {
            name: "test".to_string(),
            value: 50,
            enabled: true,
        };

        store.save(&config).await.unwrap();
        let loaded = store.load().await.unwrap();
        assert_eq!(config, loaded);
    }

    #[tokio::test]
    async fn test_validated_config_store_fails_on_save() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.json");

        let validator = RangeValidator {
            min_value: 0,
            max_value: 100,
        };
        let store = ValidatedConfigStore::new(&config_path, validator);

        let invalid_config = TestConfig {
            name: "test".to_string(),
            value: 200, // Above max
            enabled: true,
        };

        let result = store.save(&invalid_config).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Validation failed")
        );
    }

    #[tokio::test]
    async fn test_validated_config_store_fails_on_load() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.json");

        // Write config without validation
        let store = FileConfigStore::new(&config_path);
        let invalid_config = TestConfig {
            name: "test".to_string(),
            value: 200, // Above max
            enabled: true,
        };
        store.save(&invalid_config).await.unwrap();

        // Try to load with validation
        let validator = RangeValidator {
            min_value: 0,
            max_value: 100,
        };
        let validated_store = ValidatedConfigStore::new(&config_path, validator);

        let result = validated_store.load().await;
        assert!(result.is_err());
    }
}
