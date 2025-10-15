//! File-based ConfigStore implementation

use async_trait::async_trait;
use futures::stream;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use lunaroute_core::{
    Error, Result,
    config_store::{ConfigChange, ConfigChangeStream, ConfigStore},
    tenant::TenantId,
};

/// File-based configuration store for single-tenant mode
///
/// Stores configuration in a YAML file on disk and watches for changes
/// using the `notify` crate.
#[derive(Debug)]
pub struct FileConfigStore {
    /// Path to the configuration file
    config_path: PathBuf,
    /// Configuration version counter (incremented on each update)
    version: Arc<std::sync::atomic::AtomicU32>,
}

impl FileConfigStore {
    /// Create a new file-based configuration store
    ///
    /// # Arguments
    /// * `config_path` - Path to the YAML configuration file
    ///
    /// # Errors
    /// - `Error::Io` if the file doesn't exist or can't be read
    /// - `Error::Config` if the file isn't valid YAML
    pub async fn new(config_path: impl Into<PathBuf>) -> Result<Self> {
        let config_path = config_path.into();

        // Expand tilde if present
        let config_path = if config_path.starts_with("~") {
            dirs::home_dir()
                .ok_or_else(|| Error::Config("Could not determine home directory".to_string()))?
                .join(config_path.strip_prefix("~").unwrap())
        } else {
            config_path
        };

        // Verify the file exists
        if !config_path.exists() {
            return Err(Error::ConfigNotFound);
        }

        info!("Initialized FileConfigStore for {:?}", config_path);

        Ok(Self {
            config_path,
            version: Arc::new(std::sync::atomic::AtomicU32::new(1)),
        })
    }

    /// Read and parse the config file
    fn read_config_file(&self) -> Result<serde_json::Value> {
        let contents = std::fs::read_to_string(&self.config_path).map_err(|e| {
            error!("Failed to read config file: {}", e);
            Error::Io(e)
        })?;

        // Determine format based on file extension
        let config: serde_json::Value =
            if self.config_path.extension().and_then(|s| s.to_str()) == Some("toml") {
                // TOML format
                let toml_value: toml::Value = toml::from_str(&contents).map_err(|e| {
                    error!("Failed to parse TOML config: {}", e);
                    Error::Config(format!("Invalid TOML: {}", e))
                })?;
                serde_json::to_value(toml_value).map_err(|e| {
                    error!("Failed to convert TOML to JSON: {}", e);
                    Error::Config(format!("TOML conversion error: {}", e))
                })?
            } else {
                // Default to YAML
                serde_yaml::from_str(&contents).map_err(|e| {
                    error!("Failed to parse YAML config: {}", e);
                    Error::Config(format!("Invalid YAML: {}", e))
                })?
            };

        debug!("Successfully read config file");
        Ok(config)
    }

    /// Write config to file
    fn write_config_file(&self, config: &serde_json::Value) -> Result<()> {
        // Determine format based on file extension
        let contents = if self.config_path.extension().and_then(|s| s.to_str()) == Some("toml") {
            // TOML format
            let toml_value: toml::Value = serde_json::from_value(config.clone()).map_err(|e| {
                error!("Failed to convert JSON to TOML: {}", e);
                Error::Config(format!("JSON to TOML conversion error: {}", e))
            })?;
            toml::to_string_pretty(&toml_value).map_err(|e| {
                error!("Failed to serialize TOML: {}", e);
                Error::Config(format!("TOML serialization error: {}", e))
            })?
        } else {
            // Default to YAML
            serde_yaml::to_string(config).map_err(|e| {
                error!("Failed to serialize YAML: {}", e);
                Error::Config(format!("YAML serialization error: {}", e))
            })?
        };

        std::fs::write(&self.config_path, contents).map_err(|e| {
            error!("Failed to write config file: {}", e);
            Error::Io(e)
        })?;

        // Increment version
        self.version
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        info!("Successfully wrote config file");
        Ok(())
    }
}

#[async_trait]
impl ConfigStore for FileConfigStore {
    async fn get_config(&self, tenant_id: Option<TenantId>) -> Result<serde_json::Value> {
        // Single-tenant mode only
        if tenant_id.is_some() {
            return Err(Error::TenantRequired(
                "File-based config store does not support multi-tenancy".to_string(),
            ));
        }

        self.read_config_file()
    }

    async fn update_config(
        &self,
        tenant_id: Option<TenantId>,
        config: serde_json::Value,
    ) -> Result<()> {
        // Single-tenant mode only
        if tenant_id.is_some() {
            return Err(Error::TenantRequired(
                "File-based config store does not support multi-tenancy".to_string(),
            ));
        }

        // Validate before writing
        self.validate_config(&config).await?;

        self.write_config_file(&config)
    }

    async fn watch_changes(&self, tenant_id: Option<TenantId>) -> Result<ConfigChangeStream<'_>> {
        // Single-tenant mode only
        if tenant_id.is_some() {
            return Err(Error::TenantRequired(
                "File-based config store does not support multi-tenancy".to_string(),
            ));
        }

        // Create a channel for file system events
        let (tx, rx) = mpsc::channel(100);

        // Clone path and version for the watcher thread
        let config_path = self.config_path.clone();
        let version = self.version.clone();

        // Spawn watcher in a blocking task
        tokio::task::spawn_blocking(move || {
            let (notify_tx, notify_rx) = std::sync::mpsc::channel();

            // Create watcher - use std::result::Result to avoid conflict with our Result type
            let mut watcher = match RecommendedWatcher::new(
                move |res: std::result::Result<Event, notify::Error>| {
                    if let Err(e) = notify_tx.send(res) {
                        error!("Failed to send file watch event: {}", e);
                    }
                },
                notify::Config::default(),
            ) {
                Ok(w) => w,
                Err(e) => {
                    error!("Failed to create file watcher: {}", e);
                    return;
                }
            };

            // Watch the config file
            if let Err(e) = watcher.watch(&config_path, RecursiveMode::NonRecursive) {
                error!("Failed to watch config file: {}", e);
                return;
            }

            info!("Watching config file for changes: {:?}", config_path);

            // Process events
            while let Ok(event_result) = notify_rx.recv() {
                match event_result {
                    Ok(event) => {
                        // Only emit events for modify operations
                        if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                            let change = ConfigChange {
                                tenant_id: None,
                                timestamp: chrono::Utc::now(),
                                version: version.load(std::sync::atomic::Ordering::SeqCst),
                            };

                            if tx.blocking_send(Ok(change)).is_err() {
                                debug!("Config change stream closed, stopping watcher");
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        warn!("File watch error: {}", e);
                        if tx
                            .blocking_send(Err(Error::Internal(format!("File watch error: {}", e))))
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });

        // Convert mpsc receiver to stream
        let stream = stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        });

        Ok(Box::pin(stream))
    }

    async fn validate_config(&self, config: &serde_json::Value) -> Result<()> {
        // Basic validation: ensure it's an object
        if !config.is_object() {
            return Err(Error::Config(
                "Configuration must be a JSON object".to_string(),
            ));
        }

        // Try to deserialize as ServerConfig to validate structure
        // For now, we'll just do basic checks since ServerConfig is defined in lunaroute-server
        // In a production system, you'd want to import and validate against the actual schema

        let obj = config.as_object().unwrap();

        // Check for required/common fields
        if let Some(port) = obj.get("port") {
            if !port.is_number() {
                return Err(Error::Config("'port' must be a number".to_string()));
            }
            if let Some(port_val) = port.as_u64()
                && port_val > 65535
            {
                return Err(Error::Config("'port' must be <= 65535".to_string()));
            }
        }

        if let Some(host) = obj.get("host")
            && !host.is_string()
        {
            return Err(Error::Config("'host' must be a string".to_string()));
        }

        debug!("Config validation passed");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_file_not_found() {
        let result = FileConfigStore::new("/nonexistent/config.yaml").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::ConfigNotFound));
    }

    #[tokio::test]
    async fn test_read_yaml_config() {
        // Create a temp config file
        let temp_file = NamedTempFile::new().unwrap();
        let config_path = temp_file.path();

        // Write sample YAML config
        std::fs::write(
            config_path,
            r#"
host: "0.0.0.0"
port: 8080
api_dialect: "openai"
"#,
        )
        .unwrap();

        let store = FileConfigStore::new(config_path).await.unwrap();
        let config = store.get_config(None).await.unwrap();

        assert_eq!(config["host"], "0.0.0.0");
        assert_eq!(config["port"], 8080);
    }

    #[tokio::test]
    async fn test_update_config() {
        // Create a temp config file
        let temp_file = NamedTempFile::new().unwrap();
        let config_path = temp_file.path();

        // Write initial config
        std::fs::write(config_path, "host: '127.0.0.1'\nport: 8080\n").unwrap();

        let store = FileConfigStore::new(config_path).await.unwrap();

        // Update config
        let new_config = serde_json::json!({
            "host": "0.0.0.0",
            "port": 9000
        });

        store.update_config(None, new_config.clone()).await.unwrap();

        // Read it back
        let retrieved = store.get_config(None).await.unwrap();
        assert_eq!(retrieved["port"], 9000);
    }

    #[tokio::test]
    async fn test_validate_config() {
        let temp_file = NamedTempFile::new().unwrap();
        std::fs::write(temp_file.path(), "host: '127.0.0.1'\n").unwrap();

        let store = FileConfigStore::new(temp_file.path()).await.unwrap();

        // Valid config
        let valid = serde_json::json!({"host": "127.0.0.1", "port": 8080});
        assert!(store.validate_config(&valid).await.is_ok());

        // Invalid: not an object
        let invalid1 = serde_json::json!([1, 2, 3]);
        assert!(store.validate_config(&invalid1).await.is_err());

        // Invalid: port too high
        let invalid2 = serde_json::json!({"port": 70000});
        assert!(store.validate_config(&invalid2).await.is_err());

        // Invalid: port not a number
        let invalid3 = serde_json::json!({"port": "hello"});
        assert!(store.validate_config(&invalid3).await.is_err());
    }

    #[tokio::test]
    async fn test_multi_tenant_rejected() {
        let temp_file = NamedTempFile::new().unwrap();
        std::fs::write(temp_file.path(), "host: '127.0.0.1'\n").unwrap();

        let store = FileConfigStore::new(temp_file.path()).await.unwrap();
        let tenant_id = Some(TenantId::new());

        // get_config with tenant should fail
        assert!(store.get_config(tenant_id).await.is_err());

        // update_config with tenant should fail
        let config = serde_json::json!({"host": "127.0.0.1"});
        assert!(store.update_config(tenant_id, config).await.is_err());

        // watch_changes with tenant should fail
        assert!(store.watch_changes(tenant_id).await.is_err());
    }
}
