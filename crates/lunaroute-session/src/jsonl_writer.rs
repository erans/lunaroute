//! JSONL session writer implementation

use crate::events::SessionEvent;
use crate::writer::{SessionWriter, WriterResult};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;

/// JSONL writer for session events
pub struct JsonlWriter {
    sessions_dir: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct JsonlConfig {
    // Reserved for future configuration options
    // Currently empty but kept for backwards compatibility
}

/// Sanitize session ID to prevent path traversal attacks
/// Allows only alphanumeric characters, hyphens, and underscores
fn sanitize_session_id(session_id: &str) -> String {
    session_id
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .take(255) // Limit length to prevent filesystem issues
        .collect()
}

impl JsonlWriter {
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self { sessions_dir }
    }

    pub fn with_config(sessions_dir: PathBuf, _config: JsonlConfig) -> Self {
        // Config parameter kept for backwards compatibility but not used
        Self { sessions_dir }
    }

    /// Get the file path for a session (organized by date)
    fn get_session_file_path(&self, session_id: &str) -> PathBuf {
        let sanitized_id = sanitize_session_id(session_id);
        let today = Utc::now().format("%Y-%m-%d");
        self.sessions_dir
            .join(today.to_string())
            .join(format!("{}.jsonl", sanitized_id))
    }

    /// Open file for appending
    async fn open_session_file(&self, session_id: &str) -> WriterResult<tokio::fs::File> {
        let path = self.get_session_file_path(session_id);

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

        Ok(file)
    }
}

#[async_trait]
impl SessionWriter for JsonlWriter {
    async fn write_event(&self, event: &SessionEvent) -> WriterResult<()> {
        let session_id = event.session_id();
        let mut file = self.open_session_file(session_id).await?;

        let json = serde_json::to_string(event)?;
        file.write_all(json.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;

        Ok(())
    }

    async fn write_batch(&self, events: &[SessionEvent]) -> WriterResult<()> {
        // Group events by session for efficient file operations
        let mut by_session: HashMap<String, Vec<&SessionEvent>> = HashMap::new();

        for event in events {
            let session_id = event.session_id();
            by_session
                .entry(session_id.to_string())
                .or_default()
                .push(event);
        }

        // Write each session's events
        for (session_id, session_events) in by_session {
            let mut file = self.open_session_file(&session_id).await?;

            for event in session_events {
                let json = serde_json::to_string(event)?;
                file.write_all(json.as_bytes()).await?;
                file.write_all(b"\n").await?;
            }

            file.flush().await?;
        }

        Ok(())
    }

    async fn flush(&self) -> WriterResult<()> {
        // Files are flushed after each write, so nothing to do here
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
            metadata: SessionMetadata {
                client_ip: Some("127.0.0.1".to_string()),
                user_agent: Some("test".to_string()),
                api_version: Some("v1".to_string()),
                request_headers: HashMap::new(),
                session_tags: vec![],
            },
        };

        writer.write_event(&event).await.unwrap();

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
                    estimated_cost: None,
                }),
            },
        ];

        writer.write_batch(&events).await.unwrap();

        let today = Utc::now().format("%Y-%m-%d");
        let expected_path = dir.path().join(today.to_string()).join("test-1.jsonl");
        assert!(expected_path.exists());

        let content = tokio::fs::read_to_string(&expected_path).await.unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_sanitize_session_id() {
        // Normal IDs should pass through
        assert_eq!(sanitize_session_id("test-123"), "test-123");
        assert_eq!(sanitize_session_id("abc_def_123"), "abc_def_123");

        // Path traversal attempts should be sanitized
        assert_eq!(sanitize_session_id("../../../etc/passwd"), "etcpasswd");
        assert_eq!(sanitize_session_id("..\\..\\windows\\system32"), "windowssystem32");
        assert_eq!(sanitize_session_id("/absolute/path"), "absolutepath");

        // Special characters should be removed (except - and _)
        assert_eq!(sanitize_session_id("test@#$%123"), "test123");
        assert_eq!(sanitize_session_id("test;rm -rf /"), "testrm-rf");

        // Length should be limited to 255 chars
        let long_id = "a".repeat(300);
        assert_eq!(sanitize_session_id(&long_id).len(), 255);
    }

    #[tokio::test]
    async fn test_path_traversal_prevention() {
        let dir = tempdir().unwrap();
        let writer = JsonlWriter::new(dir.path().to_path_buf());

        // Try to use a malicious session ID
        let event = SessionEvent::Started {
            session_id: "../../../etc/passwd".to_string(),
            request_id: "req-1".to_string(),
            timestamp: Utc::now(),
            model_requested: "gpt-4".to_string(),
            provider: "openai".to_string(),
            listener: "openai".to_string(),
            metadata: SessionMetadata {
                client_ip: None,
                user_agent: None,
                api_version: None,
                request_headers: HashMap::new(),
                session_tags: vec![],
            },
        };

        writer.write_event(&event).await.unwrap();

        // Verify the file was created with sanitized name, not at /etc/passwd
        let today = Utc::now().format("%Y-%m-%d");
        let expected_path = dir.path().join(today.to_string()).join("etcpasswd.jsonl");
        assert!(expected_path.exists());

        // Verify that no files were created outside the temp directory
        assert!(!std::path::Path::new("/etc/passwd.jsonl").exists());
    }
}
