//! Session recording configuration

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

    /// PII detection and redaction configuration
    #[serde(default)]
    pub pii: Option<PIIConfig>,

    /// Whether to capture User-Agent header from requests
    #[serde(default = "default_true")]
    pub capture_user_agent: bool,

    /// Maximum length for user_agent strings (truncates if longer)
    #[serde(default = "default_max_user_agent_length")]
    pub max_user_agent_length: usize,
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

/// PII detection and redaction configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PIIConfig {
    /// Enable PII detection and redaction
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Enable email detection
    #[serde(default = "default_true")]
    pub detect_email: bool,

    /// Enable phone number detection
    #[serde(default = "default_true")]
    pub detect_phone: bool,

    /// Enable SSN detection
    #[serde(default = "default_true")]
    pub detect_ssn: bool,

    /// Enable credit card detection
    #[serde(default = "default_true")]
    pub detect_credit_card: bool,

    /// Enable IP address detection
    #[serde(default = "default_true")]
    pub detect_ip_address: bool,

    /// Minimum confidence threshold (0.0 to 1.0)
    #[serde(default = "default_min_confidence")]
    pub min_confidence: f32,

    /// Redaction mode: "remove", "mask", "tokenize", or "partial"
    #[serde(default = "default_redaction_mode")]
    pub redaction_mode: String,

    /// HMAC secret for tokenization (required if redaction_mode is "tokenize")
    #[serde(default)]
    pub hmac_secret: Option<String>,

    /// Number of characters to show in partial mode
    #[serde(default = "default_partial_show_chars")]
    pub partial_show_chars: usize,

    /// Custom regex patterns for detection
    #[serde(default)]
    pub custom_patterns: Vec<CustomPatternConfig>,
}

impl Default for PIIConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            detect_email: true,
            detect_phone: true,
            detect_ssn: true,
            detect_credit_card: true,
            detect_ip_address: true,
            min_confidence: default_min_confidence(),
            redaction_mode: default_redaction_mode(),
            hmac_secret: None,
            partial_show_chars: default_partial_show_chars(),
            custom_patterns: Vec::new(),
        }
    }
}

/// Custom pattern configuration for PII detection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomPatternConfig {
    /// Name of the pattern
    pub name: String,

    /// Regex pattern to match
    pub pattern: String,

    /// Confidence score for matches (0.0 to 1.0)
    pub confidence: f32,

    /// Redaction mode: "tokenize" or "mask"
    #[serde(default = "default_custom_redaction_mode")]
    pub redaction_mode: String,

    /// Placeholder text for mask mode (e.g., "[API_KEY]")
    /// If None, defaults to "[CUS:name]"
    #[serde(default)]
    pub placeholder: Option<String>,
}

fn default_min_confidence() -> f32 {
    0.7
}

fn default_redaction_mode() -> String {
    "mask".to_string()
}

fn default_custom_redaction_mode() -> String {
    "mask".to_string()
}

fn default_partial_show_chars() -> usize {
    4
}

fn default_max_user_agent_length() -> usize {
    255
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

    /// Check if PII detection and redaction is enabled
    pub fn is_pii_enabled(&self) -> bool {
        self.enabled && self.pii.as_ref().is_some_and(|c| c.enabled)
    }

    /// Expand tilde (~) in paths to the user's home directory
    pub fn expand_paths(&mut self) {
        if let Some(jsonl) = &mut self.jsonl {
            jsonl.directory = expand_tilde(jsonl.directory.as_path());
        }
        if let Some(sqlite) = &mut self.sqlite {
            sqlite.path = expand_tilde(sqlite.path.as_path());
        }
    }
}

/// Expand tilde (~) in a path to the user's home directory
/// Uses the dirs crate for cross-platform compatibility
/// Also canonicalizes the path to prevent path traversal attacks
fn expand_tilde(path: &Path) -> PathBuf {
    if let Some(path_str) = path.to_str()
        && (path_str.starts_with("~/") || path_str == "~")
    {
        // Use dirs::home_dir() for cross-platform compatibility
        if let Some(home) = dirs::home_dir() {
            let expanded = if path_str == "~" {
                home.clone()
            } else {
                // Join the home dir with the path after "~/"
                home.join(&path_str[2..])
            };

            // Canonicalize to resolve any .. or . components and prevent path traversal
            // Note: canonicalize() requires the path to exist, so we fall back to the
            // expanded path if it doesn't exist yet (e.g., during initial setup)
            match expanded.canonicalize() {
                Ok(canonical) => return canonical,
                Err(_) => {
                    // Path doesn't exist yet, return the expanded path
                    // but ensure it's within the home directory for security
                    if let Ok(home_canonical) = home.canonicalize() {
                        if expanded.starts_with(&home_canonical) {
                            return expanded;
                        } else {
                            tracing::warn!(
                                "Path expansion resulted in path outside home directory: {:?}",
                                expanded
                            );
                            return path.to_path_buf();
                        }
                    }
                    return expanded;
                }
            }
        }
    }
    path.to_path_buf()
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
            pii: None,
            capture_user_agent: true,
            max_user_agent_length: 255,
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
            pii: None,
            capture_user_agent: true,
            max_user_agent_length: 255,
        };

        assert!(!config.is_jsonl_enabled());
        assert!(config.is_sqlite_enabled());
        assert!(config.has_writers());
    }

    #[test]
    fn test_expand_tilde_home_only() {
        let path = PathBuf::from("~");
        let expanded = expand_tilde(&path);
        // Should expand to home directory
        assert!(expanded != path);
        assert!(!expanded.to_str().unwrap().contains('~'));
    }

    #[test]
    fn test_expand_tilde_with_path() {
        let path = PathBuf::from("~/.lunaroute/sessions");
        let expanded = expand_tilde(&path);
        // Should expand tilde
        assert!(!expanded.to_str().unwrap().starts_with('~'));
        // Should contain the subdirectory
        assert!(expanded.to_str().unwrap().contains(".lunaroute"));
    }

    #[test]
    fn test_expand_tilde_no_tilde() {
        let path = PathBuf::from("/absolute/path");
        let expanded = expand_tilde(&path);
        // Should remain unchanged
        assert_eq!(path, expanded);
    }

    #[test]
    fn test_expand_tilde_relative_path() {
        let path = PathBuf::from("relative/path");
        let expanded = expand_tilde(&path);
        // Should remain unchanged
        assert_eq!(path, expanded);
    }

    #[test]
    fn test_expand_paths() {
        let mut config = SessionRecordingConfig {
            enabled: true,
            jsonl: Some(JsonlConfig {
                enabled: true,
                directory: PathBuf::from("~/.lunaroute/sessions"),
                retention: RetentionPolicy::default(),
            }),
            sqlite: Some(SqliteConfig {
                enabled: true,
                path: PathBuf::from("~/.lunaroute/sessions.db"),
                max_connections: 5,
            }),
            worker: WorkerConfig::default(),
            pii: None,
            capture_user_agent: true,
            max_user_agent_length: 255,
        };

        config.expand_paths();

        // Both paths should be expanded
        assert!(!config.jsonl.as_ref().unwrap().directory.to_str().unwrap().starts_with('~'));
        assert!(!config.sqlite.as_ref().unwrap().path.to_str().unwrap().starts_with('~'));
    }

    #[test]
    fn test_both_enabled() {
        let config = SessionRecordingConfig {
            enabled: true,
            jsonl: Some(JsonlConfig::default()),
            sqlite: Some(SqliteConfig::default()),
            worker: WorkerConfig::default(),
            pii: None,
            capture_user_agent: true,
            max_user_agent_length: 255,
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
            pii: None,
            capture_user_agent: true,
            max_user_agent_length: 255,
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
            pii: None,
            capture_user_agent: true,
            max_user_agent_length: 255,
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

    #[test]
    fn test_pii_config_default() {
        let config = PIIConfig::default();
        assert!(config.enabled);
        assert!(config.detect_email);
        assert!(config.detect_phone);
        assert!(config.detect_ssn);
        assert!(config.detect_credit_card);
        assert!(config.detect_ip_address);
        assert_eq!(config.min_confidence, 0.7);
        assert_eq!(config.redaction_mode, "mask");
        assert_eq!(config.partial_show_chars, 4);
        assert_eq!(config.custom_patterns.len(), 0);
    }

    #[test]
    fn test_pii_config_serde() {
        let config = PIIConfig {
            enabled: true,
            detect_email: true,
            detect_phone: false,
            detect_ssn: true,
            detect_credit_card: false,
            detect_ip_address: true,
            min_confidence: 0.85,
            redaction_mode: "tokenize".to_string(),
            hmac_secret: Some("secret".to_string()),
            partial_show_chars: 6,
            custom_patterns: vec![CustomPatternConfig {
                name: "test".to_string(),
                pattern: r"\d+".to_string(),
                confidence: 0.9,
                redaction_mode: "mask".to_string(),
                placeholder: Some("[REDACTED]".to_string()),
            }],
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: PIIConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(config.enabled, deserialized.enabled);
        assert_eq!(config.detect_email, deserialized.detect_email);
        assert_eq!(config.min_confidence, deserialized.min_confidence);
        assert_eq!(config.redaction_mode, deserialized.redaction_mode);
        assert_eq!(config.custom_patterns.len(), deserialized.custom_patterns.len());
    }

    #[test]
    fn test_is_pii_enabled() {
        let config = SessionRecordingConfig {
            enabled: true,
            jsonl: None,
            sqlite: None,
            worker: WorkerConfig::default(),
            pii: Some(PIIConfig {
                enabled: true,
                ..PIIConfig::default()
            }),
            capture_user_agent: true,
            max_user_agent_length: 255,
        };

        assert!(config.is_pii_enabled());
    }

    #[test]
    fn test_is_pii_disabled_when_session_disabled() {
        let config = SessionRecordingConfig {
            enabled: false,
            jsonl: None,
            sqlite: None,
            worker: WorkerConfig::default(),
            pii: Some(PIIConfig {
                enabled: true,
                ..PIIConfig::default()
            }),
            capture_user_agent: true,
            max_user_agent_length: 255,
        };

        assert!(!config.is_pii_enabled());
    }

    #[test]
    fn test_is_pii_disabled_when_pii_disabled() {
        let config = SessionRecordingConfig {
            enabled: true,
            jsonl: None,
            sqlite: None,
            worker: WorkerConfig::default(),
            pii: Some(PIIConfig {
                enabled: false,
                ..PIIConfig::default()
            }),
            capture_user_agent: true,
            max_user_agent_length: 255,
        };

        assert!(!config.is_pii_enabled());
    }

    #[test]
    fn test_is_pii_disabled_when_no_pii_config() {
        let config = SessionRecordingConfig {
            enabled: true,
            jsonl: None,
            sqlite: None,
            worker: WorkerConfig::default(),
            pii: None,
            capture_user_agent: true,
            max_user_agent_length: 255,
        };

        assert!(!config.is_pii_enabled());
    }
}
