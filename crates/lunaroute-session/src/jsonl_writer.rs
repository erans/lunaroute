//! JSONL session writer implementation with production features
//!
//! # Features
//!
//! ## Performance
//! - **File Handle Caching**: LRU cache for open file handles (default: 100)
//! - **Buffered Writes**: Configurable write buffers (default: 64KB)
//!
//! ## Security
//! - **Encryption at Rest**: AES-256-GCM with Argon2id key derivation
//! - **Crypto-secure Session IDs**: 128-bit entropy from OsRng
//!
//! ## Observability
//! - **Storage Metrics**: Track events written, bytes, cache performance, errors
//! - **Health Checks**: Verify storage accessibility and writability
//!
//! # Example
//!
//! ```no_run
//! use lunaroute_session::jsonl_writer::{JsonlWriter, JsonlConfig};
//! use lunaroute_session::SessionWriter;
//! use std::path::PathBuf;
//!
//! # async fn example() {
//! // Basic usage with defaults
//! let writer = JsonlWriter::new(PathBuf::from("/sessions"));
//!
//! // Production configuration
//! let config = JsonlConfig {
//!     cache_size: 100,
//!     buffer_size: 64 * 1024,
//!     encryption_password: Some("secure-password".to_string()),
//!     encryption_salt: None, // Auto-generated
//! };
//! let writer = JsonlWriter::with_config(PathBuf::from("/sessions"), config);
//!
//! // Monitor health and metrics
//! let health = writer.health_check().await;
//! let metrics = writer.metrics();
//!
//! // IMPORTANT: Always flush before dropping the writer
//! writer.flush().await;
//! # }
//! ```
//!
//! # Important Notes
//!
//! - **Always call `flush()`** before dropping the writer to ensure buffered data is written
//! - Session IDs must be alphanumeric with hyphens/underscores only (validated for security)
//! - Salt for encryption is persisted in `sessions_dir/.encryption_salt` for consistency

use crate::events::SessionEvent;
use crate::writer::{SessionWriter, WriterResult};
use async_trait::async_trait;
use chrono::Utc;
use lunaroute_storage::encryption::{encrypt, derive_key_from_password, generate_salt, KeyDerivationParams};
use lru::LruCache;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::{Mutex, Notify};

/// Storage metrics for observability
#[derive(Debug, Clone)]
pub struct StorageMetrics {
    /// Total number of events written
    pub events_written: u64,
    /// Total bytes written (after encryption if enabled)
    pub bytes_written: u64,
    /// Number of write errors encountered
    pub write_errors: u64,
    /// Number of cache hits
    pub cache_hits: u64,
    /// Number of cache misses (file opens)
    pub cache_misses: u64,
    /// Number of cache evictions
    pub cache_evictions: u64,
}

/// Health check result
#[derive(Debug, Clone, PartialEq)]
pub struct HealthStatus {
    /// Overall health status
    pub healthy: bool,
    /// Error message if unhealthy
    pub error: Option<String>,
}

/// Internal metrics tracker with atomic counters
#[derive(Debug)]
struct MetricsTracker {
    events_written: AtomicU64,
    bytes_written: AtomicU64,
    write_errors: AtomicU64,
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    cache_evictions: AtomicU64,
}

impl MetricsTracker {
    fn new() -> Self {
        Self {
            events_written: AtomicU64::new(0),
            bytes_written: AtomicU64::new(0),
            write_errors: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            cache_evictions: AtomicU64::new(0),
        }
    }

    fn snapshot(&self) -> StorageMetrics {
        StorageMetrics {
            events_written: self.events_written.load(Ordering::Relaxed),
            bytes_written: self.bytes_written.load(Ordering::Relaxed),
            write_errors: self.write_errors.load(Ordering::Relaxed),
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            cache_misses: self.cache_misses.load(Ordering::Relaxed),
            cache_evictions: self.cache_evictions.load(Ordering::Relaxed),
        }
    }
}

/// JSONL writer for session events with file handle caching and buffered writes
pub struct JsonlWriter {
    sessions_dir: PathBuf,
    config: JsonlConfig,
    // LRU cache for file handles - uses tokio::sync::Mutex for async compatibility
    file_cache: Mutex<LruCache<String, BufWriter<tokio::fs::File>>>,
    // Encryption key (if encryption is enabled)
    encryption_key: Option<[u8; 32]>,
    // Metrics tracker
    metrics: Arc<MetricsTracker>,
    // Pending file operations to prevent race conditions
    pending_ops: Arc<Mutex<HashMap<String, Arc<Notify>>>>,
}

#[derive(Debug, Clone)]
pub struct JsonlConfig {
    /// Maximum number of open file handles to cache (default: 100)
    pub cache_size: usize,
    /// Buffer size in bytes for each file (default: 64KB)
    pub buffer_size: usize,
    /// Encryption password for AES-256-GCM encryption at rest (optional)
    pub encryption_password: Option<String>,
    /// Salt for key derivation (if None, will be generated per-installation)
    /// WARNING: Changing the salt will make existing encrypted sessions unreadable
    pub encryption_salt: Option<[u8; 16]>,
}

impl Default for JsonlConfig {
    fn default() -> Self {
        Self {
            cache_size: 100,
            buffer_size: 64 * 1024, // 64KB
            encryption_password: None,
            encryption_salt: None,
        }
    }
}

/// Validate session ID to prevent path traversal attacks
/// Returns error if ID contains unsafe characters
fn validate_session_id(session_id: &str) -> WriterResult<()> {
    // Check for path traversal attempts
    if session_id.contains("..") || session_id.contains('/') || session_id.contains('\\') {
        return Err(std::io::Error::other(format!(
            "Invalid session ID: contains path traversal characters: {}",
            session_id
        ))
        .into());
    }

    // Check for empty or too long IDs
    if session_id.is_empty() {
        return Err(std::io::Error::other("Invalid session ID: empty").into());
    }

    if session_id.len() > 255 {
        return Err(std::io::Error::other(format!(
            "Invalid session ID: too long (max 255 chars): {}",
            session_id.len()
        ))
        .into());
    }

    // Check that ID only contains safe characters (alphanumeric, hyphen, underscore)
    if !session_id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(std::io::Error::other(format!(
            "Invalid session ID: contains unsafe characters: {}",
            session_id
        ))
        .into());
    }

    Ok(())
}

impl JsonlWriter {
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self::with_config(sessions_dir, JsonlConfig::default())
    }

    /// Load or generate a persistent salt for encryption key derivation
    /// The salt is stored in sessions_dir/.encryption_salt
    fn load_or_generate_persistent_salt(sessions_dir: &Path) -> [u8; 16] {
        let salt_path = sessions_dir.join(".encryption_salt");

        // Try to load existing salt
        if let Ok(salt_bytes) = std::fs::read(&salt_path)
            && salt_bytes.len() == 16
        {
            let mut salt = [0u8; 16];
            salt.copy_from_slice(&salt_bytes);
            return salt;
        }

        // Generate new salt and save it
        let salt = generate_salt();

        // Create directory if needed
        if let Some(parent) = salt_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        // Save salt (best effort - if it fails, we'll regenerate next time)
        let _ = std::fs::write(&salt_path, salt);

        salt
    }

    pub fn with_config(sessions_dir: PathBuf, config: JsonlConfig) -> Self {
        let cache_size = NonZeroUsize::new(config.cache_size).expect("cache_size must be > 0");

        // Derive encryption key if password is provided
        let encryption_key = config.encryption_password.as_ref().map(|password| {
            // Use persistent salt if not explicitly provided
            let salt = config.encryption_salt
                .unwrap_or_else(|| Self::load_or_generate_persistent_salt(&sessions_dir));
            let params = KeyDerivationParams::default();
            derive_key_from_password(password, &salt, &params)
                .expect("Failed to derive encryption key")
        });

        Self {
            sessions_dir,
            config,
            file_cache: Mutex::new(LruCache::new(cache_size)),
            encryption_key,
            metrics: Arc::new(MetricsTracker::new()),
            pending_ops: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get current storage metrics snapshot
    pub fn metrics(&self) -> StorageMetrics {
        self.metrics.snapshot()
    }

    /// Check storage health
    /// Verifies that the sessions directory is writable and accessible
    pub async fn health_check(&self) -> HealthStatus {
        // Check if sessions directory exists and is writable
        match tokio::fs::metadata(&self.sessions_dir).await {
            Ok(metadata) => {
                if !metadata.is_dir() {
                    return HealthStatus {
                        healthy: false,
                        error: Some(format!(
                            "Sessions path is not a directory: {}",
                            self.sessions_dir.display()
                        )),
                    };
                }
            }
            Err(_) => {
                // Try to create it
                if let Err(create_err) = tokio::fs::create_dir_all(&self.sessions_dir).await {
                    return HealthStatus {
                        healthy: false,
                        error: Some(format!(
                            "Cannot access or create sessions directory: {} ({})",
                            self.sessions_dir.display(),
                            create_err
                        )),
                    };
                }
            }
        }

        // Try to write a test file to verify writability
        let test_file = self.sessions_dir.join(".healthcheck");
        match tokio::fs::write(&test_file, b"test").await {
            Ok(_) => {
                // Clean up test file
                let _ = tokio::fs::remove_file(&test_file).await;
                HealthStatus {
                    healthy: true,
                    error: None,
                }
            }
            Err(e) => HealthStatus {
                healthy: false,
                error: Some(format!(
                    "Sessions directory is not writable: {}",
                    e
                )),
            },
        }
    }

    /// Get the file path for a session (organized by date)
    fn get_session_file_path(&self, session_id: &str) -> WriterResult<PathBuf> {
        validate_session_id(session_id)?;
        let today = Utc::now().format("%Y-%m-%d");
        Ok(self
            .sessions_dir
            .join(today.to_string())
            .join(format!("{}.jsonl", session_id)))
    }

    /// Get cache key for a session (date + session_id)
    fn get_cache_key(&self, session_id: &str) -> WriterResult<String> {
        validate_session_id(session_id)?;
        let today = Utc::now().format("%Y-%m-%d");
        Ok(format!("{}:{}", today, session_id))
    }

    /// Open file for appending (used when cache miss)
    async fn open_session_file(&self, session_id: &str) -> WriterResult<BufWriter<tokio::fs::File>> {
        let path = self.get_session_file_path(session_id)?;

        // Create directory if needed
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Open file in append mode
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;

        // Wrap in BufWriter for buffering
        Ok(BufWriter::with_capacity(self.config.buffer_size, file))
    }

    /// Get or create a cached file handle
    /// Uses pending operations tracking to prevent race conditions
    async fn get_cached_file(&self, session_id: &str) -> WriterResult<()> {
        let cache_key = self.get_cache_key(session_id)?;

        loop {
            // Check if file is already cached
            {
                let cache = self.file_cache.lock().await;
                if cache.contains(&cache_key) {
                    self.metrics.cache_hits.fetch_add(1, Ordering::Relaxed);
                    return Ok(());
                }
            }

            // Check if another task is already opening this file
            let notify = {
                let mut pending = self.pending_ops.lock().await;

                if let Some(notifier) = pending.get(&cache_key) {
                    // Another task is opening this file, wait for it
                    notifier.clone()
                } else {
                    // We'll open the file - mark as pending
                    let notifier = Arc::new(Notify::new());
                    pending.insert(cache_key.clone(), notifier.clone());

                    // Release lock and proceed to open file
                    drop(pending);

                    // Track cache miss
                    self.metrics.cache_misses.fetch_add(1, Ordering::Relaxed);

                    // Open the file
                    let file = match self.open_session_file(session_id).await {
                        Ok(f) => f,
                        Err(e) => {
                            // Remove pending operation and notify waiters
                            let mut pending = self.pending_ops.lock().await;
                            if let Some(notifier) = pending.remove(&cache_key) {
                                notifier.notify_waiters();
                            }
                            return Err(e);
                        }
                    };

                    // Add to cache
                    {
                        let mut cache = self.file_cache.lock().await;

                        // Double-check it wasn't added while we were opening
                        if !cache.contains(&cache_key) {
                            // If cache is full, evict LRU entry
                            if let Some((_, mut evicted)) = cache.push(cache_key.clone(), file) {
                                self.metrics.cache_evictions.fetch_add(1, Ordering::Relaxed);

                                // Release lock before flushing evicted file
                                drop(cache);

                                // Best-effort flush of evicted file
                                let _ = evicted.flush().await;
                                drop(evicted);
                            }
                        }
                    }

                    // Remove from pending and notify waiters
                    let mut pending = self.pending_ops.lock().await;
                    if let Some(notifier) = pending.remove(&cache_key) {
                        notifier.notify_waiters();
                    }

                    return Ok(());
                }
            };

            // Wait for the other task to finish opening the file
            notify.notified().await;
            // Loop back to check cache again
        }
    }

    /// Optionally encrypt data before writing
    fn encrypt_if_enabled(&self, data: &[u8]) -> WriterResult<Vec<u8>> {
        if let Some(key) = &self.encryption_key {
            Ok(encrypt(data, key).map_err(|e| {
                std::io::Error::other(format!("Encryption failed: {}", e))
            })?)
        } else {
            Ok(data.to_vec())
        }
    }

    /// Write data to cached file
    async fn write_to_file(&self, session_id: &str, data: &[u8]) -> WriterResult<()> {
        // Ensure file is cached
        if let Err(e) = self.get_cached_file(session_id).await {
            self.metrics.write_errors.fetch_add(1, Ordering::Relaxed);
            return Err(e);
        }

        let cache_key = self.get_cache_key(session_id)?;
        let mut cache = self.file_cache.lock().await;

        if let Some(file) = cache.get_mut(&cache_key) {
            if let Err(e) = file.write_all(data).await {
                self.metrics.write_errors.fetch_add(1, Ordering::Relaxed);
                return Err(e.into());
            }
            // Track bytes written
            self.metrics.bytes_written.fetch_add(data.len() as u64, Ordering::Relaxed);
        }

        Ok(())
    }
}

#[async_trait]
impl SessionWriter for JsonlWriter {
    async fn write_event(&self, event: &SessionEvent) -> WriterResult<()> {
        let session_id = event.session_id();

        // Serialize event
        let json = serde_json::to_string(event)?;

        // Prepare JSON line (with newline)
        let mut line = json.into_bytes();
        line.push(b'\n');

        // Encrypt if enabled
        let data = self.encrypt_if_enabled(&line)?;

        // Write to cached file handle
        self.write_to_file(session_id, &data).await?;

        // Track event written
        self.metrics.events_written.fetch_add(1, Ordering::Relaxed);

        Ok(())
    }

    async fn write_batch(&self, events: &[SessionEvent]) -> WriterResult<()> {
        // Group events by session and buffer all data before writing
        let mut by_session: HashMap<String, Vec<u8>> = HashMap::new();
        let mut events_count = 0u64;

        for event in events {
            let session_id = event.session_id().to_string();
            let buffer = by_session.entry(session_id).or_default();

            // Serialize event to the buffer
            let json = serde_json::to_string(event)?;
            buffer.extend_from_slice(json.as_bytes());
            buffer.push(b'\n');

            events_count += 1;
        }

        // Single write per session with all events batched together
        for (session_id, data) in by_session {
            // Encrypt entire batch if enabled
            let encrypted_data = self.encrypt_if_enabled(&data)?;

            // Write batched data to cached file handle (single write operation)
            self.write_to_file(&session_id, &encrypted_data).await?;
        }

        // Track events written
        self.metrics.events_written.fetch_add(events_count, Ordering::Relaxed);

        Ok(())
    }

    async fn flush(&self) -> WriterResult<()> {
        // Flush all cached file handles
        let mut cache = self.file_cache.lock().await;

        for (_, file) in cache.iter_mut() {
            file.flush().await?;
        }

        Ok(())
    }

    fn supports_batching(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::*;
    use chrono::Utc;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_jsonl_writer_single_event() {
        let dir = tempdir().unwrap();
        let writer = JsonlWriter::new(dir.path().to_path_buf());

        let event = SessionEvent::Started {
            session_id: "test-123".to_string(),
            request_id: "req-456".to_string(),
            timestamp: Utc::now(),
            model_requested: "gpt-4".to_string(),
            provider: "openai".to_string(),
            listener: "openai".to_string(),
            is_streaming: false,
            metadata: SessionMetadata {
                client_ip: Some("127.0.0.1".to_string()),
                user_agent: Some("test".to_string()),
                api_version: Some("v1".to_string()),
                request_headers: HashMap::new(),
                session_tags: vec![],
            },
        };

        writer.write_event(&event).await.unwrap();
        writer.flush().await.unwrap(); // Flush buffers before reading

        // Verify file exists
        let today = Utc::now().format("%Y-%m-%d");
        let expected_path = dir.path().join(today.to_string()).join("test-123.jsonl");
        assert!(expected_path.exists());

        // Verify content
        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        assert!(content.contains("test-123"));
        assert!(content.contains("req-456"));
    }

    #[tokio::test]
    async fn test_jsonl_writer_batch() {
        let dir = tempdir().unwrap();
        let writer = JsonlWriter::new(dir.path().to_path_buf());

        let events = vec![
            SessionEvent::Started {
                session_id: "test-1".to_string(),
                request_id: "req-1".to_string(),
                timestamp: Utc::now(),
                model_requested: "gpt-4".to_string(),
                provider: "openai".to_string(),
                listener: "openai".to_string(),
                is_streaming: false,
                metadata: SessionMetadata {
                    client_ip: None,
                    user_agent: None,
                    api_version: None,
                    request_headers: HashMap::new(),
                    session_tags: vec![],
                },
            },
            SessionEvent::Completed {
                session_id: "test-1".to_string(),
                request_id: "req-1".to_string(),
                timestamp: Utc::now(),
                success: true,
                error: None,
                finish_reason: Some("stop".to_string()),
                final_stats: Box::new(FinalSessionStats {
                    total_duration_ms: 1000,
                    provider_time_ms: 900,
                    proxy_overhead_ms: 100.0,
                    total_tokens: TokenTotals {
                        total_input: 10,
                        total_output: 20,
                        total_thinking: 0,
                        total_cached: 0,
                        grand_total: 30,
                        by_model: HashMap::new(),
                    },
                    tool_summary: ToolUsageSummary {
                        total_tool_calls: 0,
                        unique_tool_count: 0,
                        by_tool: HashMap::new(),
                        total_tool_time_ms: 0,
                        tool_error_count: 0,
                    },
                    performance: PerformanceMetrics {
                        avg_provider_latency_ms: 900.0,
                        p50_latency_ms: Some(900),
                        p95_latency_ms: Some(900),
                        p99_latency_ms: Some(900),
                        max_latency_ms: 900,
                        min_latency_ms: 900,
                        avg_pre_processing_ms: 50.0,
                        avg_post_processing_ms: 50.0,
                        proxy_overhead_percentage: 10.0,
                    },
                    streaming_stats: None,
                    estimated_cost: None,
                }),
            },
        ];

        writer.write_batch(&events).await.unwrap();
        writer.flush().await.unwrap(); // Flush buffers before reading

        let today = Utc::now().format("%Y-%m-%d");
        let expected_path = dir.path().join(today.to_string()).join("test-1.jsonl");
        assert!(expected_path.exists());

        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_validate_session_id() {
        // Normal IDs should pass
        assert!(validate_session_id("test-123").is_ok());
        assert!(validate_session_id("abc_def_123").is_ok());
        assert!(validate_session_id("ValidSessionID123").is_ok());

        // Path traversal attempts should be rejected
        assert!(validate_session_id("../../../etc/passwd").is_err());
        assert!(validate_session_id("..\\..\\windows\\system32").is_err());
        assert!(validate_session_id("/absolute/path").is_err());
        assert!(validate_session_id("test/../etc").is_err());

        // Special characters should be rejected (except - and _)
        assert!(validate_session_id("test@#$%123").is_err());
        assert!(validate_session_id("test;rm -rf /").is_err());
        assert!(validate_session_id("test session").is_err()); // spaces not allowed

        // Empty or too long IDs should be rejected
        assert!(validate_session_id("").is_err());
        let long_id = "a".repeat(256);
        assert!(validate_session_id(&long_id).is_err());

        // Edge case: exactly 255 chars should pass
        let max_id = "a".repeat(255);
        assert!(validate_session_id(&max_id).is_ok());
    }

    #[tokio::test]
    async fn test_path_traversal_prevention() {
        let dir = tempdir().unwrap();
        let writer = JsonlWriter::new(dir.path().to_path_buf());

        // Try to use a malicious session ID - should be rejected
        let event = SessionEvent::Started {
            session_id: "../../../etc/passwd".to_string(),
            request_id: "req-1".to_string(),
            timestamp: Utc::now(),
            model_requested: "gpt-4".to_string(),
            provider: "openai".to_string(),
            listener: "openai".to_string(),
            is_streaming: false,
            metadata: SessionMetadata {
                client_ip: None,
                user_agent: None,
                api_version: None,
                request_headers: HashMap::new(),
                session_tags: vec![],
            },
        };

        // Malicious session ID should be rejected
        let result = writer.write_event(&event).await;
        assert!(result.is_err(), "Path traversal attempt should be rejected");
        assert!(
            result.unwrap_err().to_string().contains("path traversal"),
            "Error should mention path traversal"
        );

        // Verify that no files were created outside the temp directory
        assert!(!std::path::Path::new("/etc/passwd.jsonl").exists());

        // Verify no files were created at all for the malicious session
        let today = Utc::now().format("%Y-%m-%d");
        let sessions_date_dir = dir.path().join(today.to_string());
        // The directory might not even exist if no valid sessions were written
        if sessions_date_dir.exists() {
            let entries: Vec<_> = std::fs::read_dir(&sessions_date_dir)
                .unwrap()
                .collect();
            assert_eq!(entries.len(), 0, "No session files should be created for invalid IDs");
        }
    }

    #[tokio::test]
    async fn test_jsonl_writer_streaming_session() {
        let dir = tempdir().unwrap();
        let writer = JsonlWriter::new(dir.path().to_path_buf());

        let session_id = "streaming-session-789";
        let request_id = "req-stream-012";

        let events = vec![
            SessionEvent::Started {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                timestamp: Utc::now(),
                model_requested: "claude-sonnet-4".to_string(),
                provider: "anthropic".to_string(),
                listener: "anthropic".to_string(),
                is_streaming: true,
                metadata: SessionMetadata {
                    client_ip: Some("10.0.0.1".to_string()),
                    user_agent: Some("test-streaming".to_string()),
                    api_version: Some("2023-06-01".to_string()),
                    request_headers: HashMap::new(),
                    session_tags: vec!["test".to_string(), "streaming".to_string()],
                },
            },
            SessionEvent::StreamStarted {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                timestamp: Utc::now(),
                time_to_first_token_ms: 125,
            },
            SessionEvent::Completed {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                timestamp: Utc::now(),
                success: true,
                error: None,
                finish_reason: Some("end_turn".to_string()),
                final_stats: Box::new(FinalSessionStats {
                    total_duration_ms: 3500,
                    provider_time_ms: 3400,
                    proxy_overhead_ms: 100.0,
                    total_tokens: TokenTotals {
                        total_input: 50,
                        total_output: 300,
                        total_thinking: 25,
                        total_cached: 10,
                        grand_total: 375,
                        by_model: HashMap::new(),
                    },
                    tool_summary: ToolUsageSummary::default(),
                    performance: PerformanceMetrics::default(),
                    streaming_stats: Some(StreamingStats {
                        time_to_first_token_ms: 125,
                        total_chunks: 28,
                        streaming_duration_ms: 3375,
                        avg_chunk_latency_ms: 120.5,
                        p50_chunk_latency_ms: Some(110),
                        p95_chunk_latency_ms: Some(180),
                        p99_chunk_latency_ms: Some(200),
                        max_chunk_latency_ms: 250,
                        min_chunk_latency_ms: 80,
                    }),
                    estimated_cost: None,
                }),
            },
        ];

        writer.write_batch(&events).await.unwrap();
        writer.flush().await.unwrap(); // Flush buffers before reading

        let today = Utc::now().format("%Y-%m-%d");
        let expected_path = dir.path().join(today.to_string()).join(format!("{}.jsonl", session_id));
        assert!(expected_path.exists());

        // Read and verify the content
        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3); // Started, StreamStarted, Completed

        // Parse and verify each event
        let started: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(started["type"], "started");
        assert_eq!(started["is_streaming"], true);

        let stream_started: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(stream_started["type"], "stream_started");
        assert_eq!(stream_started["time_to_first_token_ms"], 125);

        let completed: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(completed["type"], "completed");

        // streaming_stats is flattened at the top level of Completed event
        let streaming_stats = &completed["streaming_stats"];
        assert!(!streaming_stats.is_null(), "streaming_stats should not be null");
        assert!(streaming_stats.is_object(), "streaming_stats should be an object");
        assert_eq!(streaming_stats["total_chunks"], 28);
        assert_eq!(streaming_stats["time_to_first_token_ms"], 125);
        assert_eq!(streaming_stats["streaming_duration_ms"], 3375);
        assert_eq!(streaming_stats["p95_chunk_latency_ms"], 180);
    }

    #[tokio::test]
    async fn test_file_handle_caching() {
        let dir = tempdir().unwrap();
        let config = JsonlConfig {
            cache_size: 2, // Small cache to test eviction
            buffer_size: 1024,
            encryption_password: None,
            encryption_salt: None,
        };
        let writer = JsonlWriter::with_config(dir.path().to_path_buf(), config);

        // Write to session 1
        let event1 = SessionEvent::Started {
            session_id: "session-1".to_string(),
            request_id: "req-1".to_string(),
            timestamp: Utc::now(),
            model_requested: "gpt-4".to_string(),
            provider: "openai".to_string(),
            listener: "openai".to_string(),
            is_streaming: false,
            metadata: SessionMetadata {
                client_ip: None,
                user_agent: None,
                api_version: None,
                request_headers: HashMap::new(),
                session_tags: vec![],
            },
        };
        writer.write_event(&event1).await.unwrap();

        // Write to session 2
        let event2 = SessionEvent::Started {
            session_id: "session-2".to_string(),
            request_id: "req-2".to_string(),
            timestamp: Utc::now(),
            model_requested: "gpt-4".to_string(),
            provider: "openai".to_string(),
            listener: "openai".to_string(),
            is_streaming: false,
            metadata: SessionMetadata {
                client_ip: None,
                user_agent: None,
                api_version: None,
                request_headers: HashMap::new(),
                session_tags: vec![],
            },
        };
        writer.write_event(&event2).await.unwrap();

        // Write to session 3 (should evict session-1 from cache)
        let event3 = SessionEvent::Started {
            session_id: "session-3".to_string(),
            request_id: "req-3".to_string(),
            timestamp: Utc::now(),
            model_requested: "gpt-4".to_string(),
            provider: "openai".to_string(),
            listener: "openai".to_string(),
            is_streaming: false,
            metadata: SessionMetadata {
                client_ip: None,
                user_agent: None,
                api_version: None,
                request_headers: HashMap::new(),
                session_tags: vec![],
            },
        };
        writer.write_event(&event3).await.unwrap();

        // Write again to session-1 (should open file again since it was evicted)
        let event4 = SessionEvent::Started {
            session_id: "session-1".to_string(),
            request_id: "req-4".to_string(),
            timestamp: Utc::now(),
            model_requested: "gpt-4".to_string(),
            provider: "openai".to_string(),
            listener: "openai".to_string(),
            is_streaming: false,
            metadata: SessionMetadata {
                client_ip: None,
                user_agent: None,
                api_version: None,
                request_headers: HashMap::new(),
                session_tags: vec![],
            },
        };
        writer.write_event(&event4).await.unwrap();

        // Flush all writes
        writer.flush().await.unwrap();

        // Verify all files exist and have correct content
        let today = Utc::now().format("%Y-%m-%d");
        for session_id in ["session-1", "session-2", "session-3"] {
            let path = dir.path().join(today.to_string()).join(format!("{}.jsonl", session_id));
            assert!(path.exists(), "Session file should exist: {}", session_id);

            let content = tokio::fs::read_to_string(&path).await.unwrap();
            assert!(content.contains(session_id));
        }

        // session-1 should have 2 events (req-1 and req-4)
        let session1_path = dir.path().join(today.to_string()).join("session-1.jsonl");
        let content = tokio::fs::read_to_string(&session1_path).await.unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2, "session-1 should have 2 events");
        assert!(content.contains("req-1"));
        assert!(content.contains("req-4"));
    }

    #[tokio::test]
    async fn test_configurable_buffer_size() {
        let dir = tempdir().unwrap();
        let config = JsonlConfig {
            cache_size: 10,
            buffer_size: 512, // Small buffer for testing
            encryption_password: None,
            encryption_salt: None,
        };
        let writer = JsonlWriter::with_config(dir.path().to_path_buf(), config);

        // Write multiple events
        for i in 0..10 {
            let event = SessionEvent::Started {
                session_id: "buffered-session".to_string(),
                request_id: format!("req-{}", i),
                timestamp: Utc::now(),
                model_requested: "gpt-4".to_string(),
                provider: "openai".to_string(),
                listener: "openai".to_string(),
                is_streaming: false,
                metadata: SessionMetadata {
                client_ip: None,
                user_agent: None,
                api_version: None,
                request_headers: HashMap::new(),
                session_tags: vec![],
            },
            };
            writer.write_event(&event).await.unwrap();
        }

        // Flush to write buffered data
        writer.flush().await.unwrap();

        // Verify all events were written
        let today = Utc::now().format("%Y-%m-%d");
        let path = dir.path().join(today.to_string()).join("buffered-session.jsonl");
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 10, "All 10 events should be written");
    }

    #[tokio::test]
    async fn test_encryption_at_rest() {
        let dir = tempdir().unwrap();
        let password = "test-encryption-password-123";
        let salt = [42u8; 16]; // Fixed salt for deterministic testing

        let config = JsonlConfig {
            cache_size: 10,
            buffer_size: 1024,
            encryption_password: Some(password.to_string()),
            encryption_salt: Some(salt),
        };
        let writer = JsonlWriter::with_config(dir.path().to_path_buf(), config);

        let event = SessionEvent::Started {
            session_id: "encrypted-session".to_string(),
            request_id: "req-encrypted-1".to_string(),
            timestamp: Utc::now(),
            model_requested: "gpt-4".to_string(),
            provider: "openai".to_string(),
            listener: "openai".to_string(),
            is_streaming: false,
            metadata: SessionMetadata {
                client_ip: Some("192.168.1.1".to_string()),
                user_agent: Some("test-agent".to_string()),
                api_version: Some("v1".to_string()),
                request_headers: HashMap::new(),
                session_tags: vec!["sensitive".to_string()],
            },
        };

        writer.write_event(&event).await.unwrap();
        writer.flush().await.unwrap();

        // Read the raw file - should be encrypted (not readable JSON)
        let today = Utc::now().format("%Y-%m-%d");
        let path = dir.path().join(today.to_string()).join("encrypted-session.jsonl");
        assert!(path.exists());

        let raw_content = tokio::fs::read(&path).await.unwrap();

        // Raw content should NOT contain plaintext
        let raw_str = String::from_utf8_lossy(&raw_content);
        assert!(!raw_str.contains("encrypted-session"), "Session ID should be encrypted");
        assert!(!raw_str.contains("req-encrypted-1"), "Request ID should be encrypted");
        assert!(!raw_str.contains("192.168.1.1"), "IP address should be encrypted");
        assert!(!raw_str.contains("sensitive"), "Tags should be encrypted");

        // File should contain binary data (encrypted)
        assert!(raw_content.len() > 12, "Should have nonce + ciphertext");

        // Each line should start with 12 bytes of nonce (binary)
        // This is a basic check that encryption was applied
        assert!(raw_content[0..12].iter().any(|&b| b != 0), "Should have non-zero nonce bytes");
    }

    #[tokio::test]
    async fn test_crypto_secure_session_ids() {
        use crate::recorder::{FileSessionRecorder, SessionRecorder};

        let dir = tempdir().unwrap();
        let recorder = FileSessionRecorder::new(dir.path());

        // Generate multiple session IDs
        let id1 = recorder.generate_session_id();
        let id2 = recorder.generate_session_id();
        let id3 = recorder.generate_session_id();

        // Should be 32 hex characters (16 bytes * 2)
        assert_eq!(id1.len(), 32, "Session ID should be 32 hex characters");
        assert_eq!(id2.len(), 32, "Session ID should be 32 hex characters");
        assert_eq!(id3.len(), 32, "Session ID should be 32 hex characters");

        // Should be unique
        assert_ne!(id1, id2, "Session IDs should be unique");
        assert_ne!(id2, id3, "Session IDs should be unique");
        assert_ne!(id1, id3, "Session IDs should be unique");

        // Should be valid hex
        assert!(id1.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(id2.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(id3.chars().all(|c| c.is_ascii_hexdigit()));

        // Should be filesystem-safe (no special characters)
        assert!(id1.chars().all(|c| c.is_alphanumeric()));
        assert!(id2.chars().all(|c| c.is_alphanumeric()));
        assert!(id3.chars().all(|c| c.is_alphanumeric()));
    }

    #[tokio::test]
    async fn test_encryption_disabled_by_default() {
        let dir = tempdir().unwrap();
        let config = JsonlConfig::default(); // No encryption
        let writer = JsonlWriter::with_config(dir.path().to_path_buf(), config);

        let event = SessionEvent::Started {
            session_id: "unencrypted-session".to_string(),
            request_id: "req-plain-1".to_string(),
            timestamp: Utc::now(),
            model_requested: "gpt-4".to_string(),
            provider: "openai".to_string(),
            listener: "openai".to_string(),
            is_streaming: false,
            metadata: SessionMetadata {
                client_ip: None,
                user_agent: None,
                api_version: None,
                request_headers: HashMap::new(),
                session_tags: vec![],
            },
        };

        writer.write_event(&event).await.unwrap();
        writer.flush().await.unwrap();

        // Read the file - should be plaintext JSON
        let today = Utc::now().format("%Y-%m-%d");
        let path = dir.path().join(today.to_string()).join("unencrypted-session.jsonl");
        let content = tokio::fs::read_to_string(&path).await.unwrap();

        // Should contain readable JSON
        assert!(content.contains("unencrypted-session"), "Should contain plaintext session ID");
        assert!(content.contains("req-plain-1"), "Should contain plaintext request ID");

        // Should be parseable as JSON
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 1);
        let parsed: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed["type"], "started");
    }

    #[tokio::test]
    async fn test_storage_metrics() {
        let dir = tempdir().unwrap();
        let config = JsonlConfig {
            cache_size: 2,
            buffer_size: 1024,
            encryption_password: None,
            encryption_salt: None,
        };
        let writer = JsonlWriter::with_config(dir.path().to_path_buf(), config);

        // Initial metrics should be zero
        let metrics = writer.metrics();
        assert_eq!(metrics.events_written, 0);
        assert_eq!(metrics.bytes_written, 0);
        assert_eq!(metrics.write_errors, 0);
        assert_eq!(metrics.cache_hits, 0);
        assert_eq!(metrics.cache_misses, 0);
        assert_eq!(metrics.cache_evictions, 0);

        // Write some events
        for i in 0..5 {
            let event = SessionEvent::Started {
                session_id: format!("session-{}", i % 3), // 3 different sessions
                request_id: format!("req-{}", i),
                timestamp: Utc::now(),
                model_requested: "gpt-4".to_string(),
                provider: "openai".to_string(),
                listener: "openai".to_string(),
                is_streaming: false,
                metadata: SessionMetadata {
                    client_ip: None,
                    user_agent: None,
                    api_version: None,
                    request_headers: HashMap::new(),
                    session_tags: vec![],
                },
            };
            writer.write_event(&event).await.unwrap();
        }

        writer.flush().await.unwrap();

        // Check metrics
        let metrics = writer.metrics();
        assert_eq!(metrics.events_written, 5, "Should track 5 events written");
        assert!(metrics.bytes_written > 0, "Should track bytes written");

        // With cache_size=2 and 3 sessions, we should see:
        // Event sequence: session-0, session-1, session-2, session-0, session-1
        // - session-0 (i=0): cache miss, opens file
        // - session-1 (i=1): cache miss, opens file
        // - session-2 (i=2): cache miss, opens file, evicts session-0 (LRU)
        // - session-0 (i=3): cache miss, opens file, evicts session-1 (LRU)
        // - session-1 (i=4): cache miss, opens file, evicts session-2 (LRU)
        // Total: 5 misses, 0 hits, 3 evictions
        assert_eq!(metrics.cache_misses, 5, "Should have 5 cache misses");
        assert_eq!(metrics.cache_hits, 0, "Should have 0 cache hits");
        assert_eq!(metrics.cache_evictions, 3, "Should have 3 evictions");
        assert_eq!(metrics.write_errors, 0, "Should have no write errors");
    }

    #[tokio::test]
    async fn test_health_check_success() {
        let dir = tempdir().unwrap();
        let writer = JsonlWriter::new(dir.path().to_path_buf());

        let health = writer.health_check().await;
        assert!(health.healthy, "Health check should pass for writable directory");
        assert_eq!(health.error, None, "Should have no error message");
    }

    #[tokio::test]
    async fn test_health_check_creates_directory() {
        let dir = tempdir().unwrap();
        let sessions_dir = dir.path().join("non-existent-dir");

        // Directory doesn't exist yet
        assert!(!sessions_dir.exists());

        let writer = JsonlWriter::new(sessions_dir.clone());
        let health = writer.health_check().await;

        assert!(health.healthy, "Health check should create directory");
        assert!(sessions_dir.exists(), "Directory should be created");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_health_check_readonly_directory() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let sessions_dir = dir.path().join("readonly-dir");
        fs::create_dir(&sessions_dir).unwrap();

        // Make directory read-only (remove write permissions)
        let mut perms = fs::metadata(&sessions_dir).unwrap().permissions();
        perms.set_mode(0o555); // r-xr-xr-x (read and execute only)
        fs::set_permissions(&sessions_dir, perms).unwrap();

        let writer = JsonlWriter::new(sessions_dir.clone());
        let health = writer.health_check().await;

        // Restore write permissions for cleanup
        let mut perms = fs::metadata(&sessions_dir).unwrap().permissions();
        perms.set_mode(0o755); // rwxr-xr-x (owner has write)
        fs::set_permissions(&sessions_dir, perms).unwrap();

        assert!(!health.healthy, "Health check should fail for read-only directory");
        assert!(health.error.is_some(), "Should have error message");
        assert!(
            health.error.unwrap().contains("not writable"),
            "Error should mention writability"
        );
    }
}
