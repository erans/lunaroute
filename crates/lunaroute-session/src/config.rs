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

    /// Retention policy for session cleanup
    #[serde(default)]
    pub retention: RetentionPolicy,
}

impl Default for JsonlConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            directory: PathBuf::from("~/.lunaroute/sessions"),
            retention: RetentionPolicy::default(),
        }
    }
}

/// Retention policy for session cleanup and archival
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionPolicy {
    /// Maximum age of sessions in days (None = no age limit)
    #[serde(default)]
    pub max_age_days: Option<u32>,

    /// Maximum total size in gigabytes (None = no size limit)
    #[serde(default)]
    pub max_total_size_gb: Option<u32>,

    /// Enable compression for sessions older than this many days (None = no compression)
    #[serde(default)]
    pub compress_after_days: Option<u32>,

    /// Run cleanup task every N minutes
    #[serde(default = "default_cleanup_interval_minutes")]
    pub cleanup_interval_minutes: u32,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            max_age_days: Some(30),        // Default: 30 days
            max_total_size_gb: Some(10),   // Default: 10 GB
            compress_after_days: Some(7),   // Default: compress after 7 days
            cleanup_interval_minutes: default_cleanup_interval_minutes(),
        }
    }
}

fn default_cleanup_interval_minutes() -> u32 {
    60 // Run cleanup every hour by default
}

impl RetentionPolicy {
    /// Check if this policy has any limits enabled
    pub fn has_limits(&self) -> bool {
        self.max_age_days.is_some() || self.max_total_size_gb.is_some()
    }

    /// Check if compression is enabled
    pub fn compression_enabled(&self) -> bool {
        self.compress_after_days.is_some()
    }

    /// Validate the retention policy configuration
    pub fn validate(&self) -> Result<(), String> {
        if let Some(compress_after) = self.compress_after_days
            && compress_after == 0
        {
            return Err("compress_after_days must be at least 1".to_string());
        }

        if let Some(max_age) = self.max_age_days {
            if max_age == 0 {
                return Err("max_age_days must be at least 1".to_string());
            }

            // Compression should happen before deletion
            if let Some(compress_after) = self.compress_after_days
                && compress_after >= max_age
            {
                return Err(
                    "compress_after_days must be less than max_age_days".to_string()
                );
            }
        }

        if let Some(max_size) = self.max_total_size_gb
            && max_size == 0
        {
            return Err("max_total_size_gb must be at least 1".to_string());
        }

        if self.cleanup_interval_minutes == 0 {
            return Err("cleanup_interval_minutes must be at least 1".to_string());
        }

        Ok(())
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
                retention: RetentionPolicy::default(),
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

    #[test]
    fn test_retention_policy_default() {
        let policy = RetentionPolicy::default();
        assert_eq!(policy.max_age_days, Some(30));
        assert_eq!(policy.max_total_size_gb, Some(10));
        assert_eq!(policy.compress_after_days, Some(7));
        assert_eq!(policy.cleanup_interval_minutes, 60);
        assert!(policy.has_limits());
        assert!(policy.compression_enabled());
        assert!(policy.validate().is_ok());
    }

    #[test]
    fn test_retention_policy_validation_valid() {
        let policy = RetentionPolicy {
            max_age_days: Some(30),
            max_total_size_gb: Some(10),
            compress_after_days: Some(7),
            cleanup_interval_minutes: 60,
        };
        assert!(policy.validate().is_ok());
    }

    #[test]
    fn test_retention_policy_validation_compress_after_greater_than_max_age() {
        let policy = RetentionPolicy {
            max_age_days: Some(7),
            max_total_size_gb: None,
            compress_after_days: Some(7), // Equal to max_age
            cleanup_interval_minutes: 60,
        };
        assert!(policy.validate().is_err());
        assert_eq!(
            policy.validate().unwrap_err(),
            "compress_after_days must be less than max_age_days"
        );
    }

    #[test]
    fn test_retention_policy_validation_zero_values() {
        let policy = RetentionPolicy {
            max_age_days: Some(0),
            max_total_size_gb: None,
            compress_after_days: None,
            cleanup_interval_minutes: 60,
        };
        assert!(policy.validate().is_err());

        let policy = RetentionPolicy {
            max_age_days: None,
            max_total_size_gb: Some(0),
            compress_after_days: None,
            cleanup_interval_minutes: 60,
        };
        assert!(policy.validate().is_err());

        let policy = RetentionPolicy {
            max_age_days: None,
            max_total_size_gb: None,
            compress_after_days: Some(0),
            cleanup_interval_minutes: 60,
        };
        assert!(policy.validate().is_err());

        let policy = RetentionPolicy {
            max_age_days: None,
            max_total_size_gb: None,
            compress_after_days: None,
            cleanup_interval_minutes: 0,
        };
        assert!(policy.validate().is_err());
    }

    #[test]
    fn test_retention_policy_no_limits() {
        let policy = RetentionPolicy {
            max_age_days: None,
            max_total_size_gb: None,
            compress_after_days: None,
            cleanup_interval_minutes: 60,
        };
        assert!(!policy.has_limits());
        assert!(!policy.compression_enabled());
        assert!(policy.validate().is_ok());
    }

    #[test]
    fn test_retention_policy_serde() {
        let policy = RetentionPolicy {
            max_age_days: Some(15),
            max_total_size_gb: Some(5),
            compress_after_days: Some(3),
            cleanup_interval_minutes: 30,
        };

        let json = serde_json::to_string(&policy).unwrap();
        let deserialized: RetentionPolicy = serde_json::from_str(&json).unwrap();

        assert_eq!(policy.max_age_days, deserialized.max_age_days);
        assert_eq!(policy.max_total_size_gb, deserialized.max_total_size_gb);
        assert_eq!(policy.compress_after_days, deserialized.compress_after_days);
        assert_eq!(
            policy.cleanup_interval_minutes,
            deserialized.cleanup_interval_minutes
        );
    }
}
