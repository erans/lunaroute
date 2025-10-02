//! Session recording configuration

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct SessionRecordingConfig {
    /// Enable session recording
    #[serde(default)]
    pub enabled: bool,

    /// JSONL writer configuration
    #[serde(default)]
    pub jsonl: Option<JsonlConfig>,

    /// SQLite writer configuration
    #[serde(default)]
    pub sqlite: Option<SqliteConfig>,

    /// Background worker configuration
    #[serde(default)]
    pub worker: WorkerConfig,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonlConfig {
    /// Enable JSONL writer
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Directory for session files
    pub directory: PathBuf,
}

impl Default for JsonlConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            directory: PathBuf::from("~/.lunaroute/sessions"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqliteConfig {
    /// Enable SQLite writer
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Path to SQLite database
    pub path: PathBuf,

    /// Maximum database connections
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
}

impl Default for SqliteConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: PathBuf::from("~/.lunaroute/sessions.db"),
            max_connections: default_max_connections(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    /// Maximum events to buffer before flushing
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,

    /// Maximum time to wait before flushing (milliseconds)
    #[serde(default = "default_batch_timeout_ms")]
    pub batch_timeout_ms: u64,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            batch_size: default_batch_size(),
            batch_timeout_ms: default_batch_timeout_ms(),
        }
    }
}

// Default value functions for serde
fn default_true() -> bool {
    true
}

fn default_max_connections() -> u32 {
    5
}

fn default_batch_size() -> usize {
    100
}

fn default_batch_timeout_ms() -> u64 {
    100
}

impl SessionRecordingConfig {
    /// Check if JSONL writer is enabled
    pub fn is_jsonl_enabled(&self) -> bool {
        self.enabled && self.jsonl.as_ref().is_some_and(|c| c.enabled)
    }

    /// Check if SQLite writer is enabled
    pub fn is_sqlite_enabled(&self) -> bool {
        self.enabled && self.sqlite.as_ref().is_some_and(|c| c.enabled)
    }

    /// Check if any writer is enabled
    pub fn has_writers(&self) -> bool {
        self.is_jsonl_enabled() || self.is_sqlite_enabled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SessionRecordingConfig::default();
        assert!(!config.enabled);
        assert!(!config.has_writers());
    }

    #[test]
    fn test_jsonl_enabled() {
        let config = SessionRecordingConfig {
            enabled: true,
            jsonl: Some(JsonlConfig::default()),
            sqlite: None,
            worker: WorkerConfig::default(),
        };

        assert!(config.is_jsonl_enabled());
        assert!(!config.is_sqlite_enabled());
        assert!(config.has_writers());
    }

    #[test]
    fn test_sqlite_enabled() {
        let config = SessionRecordingConfig {
            enabled: true,
            jsonl: None,
            sqlite: Some(SqliteConfig::default()),
            worker: WorkerConfig::default(),
        };

        assert!(!config.is_jsonl_enabled());
        assert!(config.is_sqlite_enabled());
        assert!(config.has_writers());
    }

    #[test]
    fn test_both_enabled() {
        let config = SessionRecordingConfig {
            enabled: true,
            jsonl: Some(JsonlConfig::default()),
            sqlite: Some(SqliteConfig::default()),
            worker: WorkerConfig::default(),
        };

        assert!(config.is_jsonl_enabled());
        assert!(config.is_sqlite_enabled());
        assert!(config.has_writers());
    }

    #[test]
    fn test_disabled_with_config() {
        let config = SessionRecordingConfig {
            enabled: false,
            jsonl: Some(JsonlConfig::default()),
            sqlite: Some(SqliteConfig::default()),
            worker: WorkerConfig::default(),
        };

        assert!(!config.is_jsonl_enabled());
        assert!(!config.is_sqlite_enabled());
        assert!(!config.has_writers());
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = SessionRecordingConfig {
            enabled: true,
            jsonl: Some(JsonlConfig {
                enabled: true,
                directory: PathBuf::from("/tmp/sessions"),
            }),
            sqlite: Some(SqliteConfig {
                enabled: true,
                path: PathBuf::from("/tmp/sessions.db"),
                max_connections: 10,
            }),
            worker: WorkerConfig {
                batch_size: 200,
                batch_timeout_ms: 50,
            },
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: SessionRecordingConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(config.enabled, deserialized.enabled);
        assert_eq!(
            config.jsonl.as_ref().unwrap().directory,
            deserialized.jsonl.as_ref().unwrap().directory
        );
        assert_eq!(
            config.sqlite.as_ref().unwrap().path,
            deserialized.sqlite.as_ref().unwrap().path
        );
    }
}
