//! Session recorder
//!
//! This module provides the SessionRecorder trait and implementations for
//! recording LLM request/response sessions.
//!
//! # Security
//!
//! Session IDs are generated using cryptographically secure random number
//! generation (OsRng) with 128 bits of entropy, encoded as 32 hex characters.
//! This ensures session IDs are unpredictable and filesystem-safe.

use crate::session::{SessionId, SessionMetadata, SessionQuery};
use lunaroute_core::{
    normalized::{NormalizedRequest, NormalizedResponse, NormalizedStreamEvent},
    Result, Error,
};
use async_trait::async_trait;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// Generate a cryptographically secure session ID using OsRng
/// Format: 32 hex characters (128 bits of entropy)
fn generate_secure_session_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Recorded session data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedSession {
    /// Session metadata
    pub metadata: SessionMetadata,

    /// Original request
    pub request: NormalizedRequest,

    /// Response (if non-streaming)
    pub response: Option<NormalizedResponse>,

    /// Stream events (if streaming)
    pub stream_events: Vec<NormalizedStreamEvent>,
}

/// Session recorder trait
#[async_trait]
pub trait SessionRecorder: Send + Sync {
    /// Generate a new cryptographically secure session ID
    /// Uses OsRng for 128 bits of entropy
    fn generate_session_id(&self) -> SessionId {
        generate_secure_session_id()
    }

    /// Start recording a new session
    async fn start_session(
        &self,
        session_id: SessionId,
        request: &NormalizedRequest,
        metadata: SessionMetadata,
    ) -> Result<()>;

    /// Record a non-streaming response
    async fn record_response(
        &self,
        session_id: &SessionId,
        response: &NormalizedResponse,
    ) -> Result<()>;

    /// Record a stream event
    async fn record_stream_event(
        &self,
        session_id: &SessionId,
        event: &NormalizedStreamEvent,
    ) -> Result<()>;

    /// Complete a session with final metadata
    async fn complete_session(
        &self,
        session_id: &SessionId,
        metadata: SessionMetadata,
    ) -> Result<()>;

    /// Query sessions
    async fn query_sessions(&self, query: &SessionQuery) -> Result<Vec<SessionMetadata>>;

    /// Get a recorded session by ID
    async fn get_session(&self, session_id: &SessionId) -> Result<Option<RecordedSession>>;

    /// Delete a session
    async fn delete_session(&self, session_id: &SessionId) -> Result<()>;
}

/// File-based session recorder
pub struct FileSessionRecorder {
    base_path: PathBuf,
}

impl FileSessionRecorder {
    /// Create a new file session recorder
    pub fn new<P: AsRef<Path>>(base_path: P) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
        }
    }

    /// Validate session ID to prevent path traversal attacks
    fn validate_session_id(session_id: &SessionId) -> Result<()> {
        // Check length
        if session_id.is_empty() || session_id.len() > 255 {
            return Err(Error::Internal("Invalid session ID length".to_string()));
        }

        // Only allow alphanumeric, dash, and underscore (safe for filesystem)
        if !session_id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            return Err(Error::Internal(format!(
                "Invalid session ID format: {}. Only alphanumeric, dash, and underscore allowed",
                session_id
            )));
        }

        // Prevent path traversal patterns
        if session_id.contains("..") || session_id.contains('/') || session_id.contains('\\') {
            return Err(Error::Internal(format!(
                "Invalid session ID: {} contains path traversal characters",
                session_id
            )));
        }

        Ok(())
    }

    /// Get session directory path (with validation)
    fn session_dir(&self, session_id: &SessionId) -> Result<PathBuf> {
        Self::validate_session_id(session_id)?;
        Ok(self.base_path.join(session_id))
    }

    /// Get metadata file path
    fn metadata_path(&self, session_id: &SessionId) -> Result<PathBuf> {
        Ok(self.session_dir(session_id)?.join("metadata.json"))
    }

    /// Get events file path
    fn events_path(&self, session_id: &SessionId) -> Result<PathBuf> {
        Ok(self.session_dir(session_id)?.join("events.ndjson"))
    }
}

#[async_trait]
impl SessionRecorder for FileSessionRecorder {
    async fn start_session(
        &self,
        session_id: SessionId,
        request: &NormalizedRequest,
        metadata: SessionMetadata,
    ) -> Result<()> {
        // Create session directory
        let session_dir = self.session_dir(&session_id)?;
        fs::create_dir_all(&session_dir).await
            .map_err(|e| Error::Internal(format!("Failed to create session directory {}: {}", session_id, e)))?;

        // Write session metadata
        let metadata_json = serde_json::to_vec_pretty(&metadata)?;
        fs::write(self.metadata_path(&session_id)?, &metadata_json).await
            .map_err(|e| Error::Internal(format!("Failed to write metadata for session {}: {}", session_id, e)))?;

        // Write request as first line of NDJSON
        let request_json = serde_json::to_vec(&request)?;
        let mut events_file = fs::File::create(self.events_path(&session_id)?).await
            .map_err(|e| Error::Internal(format!("Failed to create events file for session {}: {}", session_id, e)))?;
        events_file.write_all(&request_json).await
            .map_err(|e| Error::Internal(format!("Failed to write request for session {}: {}", session_id, e)))?;
        events_file.write_all(b"\n").await
            .map_err(|e| Error::Internal(format!("Failed to write newline for session {}: {}", session_id, e)))?;

        tracing::debug!(session_id = %session_id, "Started recording session");
        Ok(())
    }

    async fn record_response(
        &self,
        session_id: &SessionId,
        response: &NormalizedResponse,
    ) -> Result<()> {
        // Append response as NDJSON
        let response_json = serde_json::to_vec(&response)?;
        let mut events_file = fs::OpenOptions::new()
            .append(true)
            .open(self.events_path(session_id)?)
            .await
            .map_err(|e| Error::Internal(format!("Failed to open events file for session {}: {}", session_id, e)))?;
        events_file.write_all(&response_json).await
            .map_err(|e| Error::Internal(format!("Failed to write response for session {}: {}", session_id, e)))?;
        events_file.write_all(b"\n").await
            .map_err(|e| Error::Internal(format!("Failed to write newline for session {}: {}", session_id, e)))?;

        tracing::debug!(session_id = %session_id, "Recorded response");
        Ok(())
    }

    async fn record_stream_event(
        &self,
        session_id: &SessionId,
        event: &NormalizedStreamEvent,
    ) -> Result<()> {
        // Append stream event as NDJSON
        let event_json = serde_json::to_vec(&event)?;
        let mut events_file = fs::OpenOptions::new()
            .append(true)
            .open(self.events_path(session_id)?)
            .await
            .map_err(|e| Error::Internal(format!("Failed to open events file for session {}: {}", session_id, e)))?;
        events_file.write_all(&event_json).await
            .map_err(|e| Error::Internal(format!("Failed to write event for session {}: {}", session_id, e)))?;
        events_file.write_all(b"\n").await
            .map_err(|e| Error::Internal(format!("Failed to write newline for session {}: {}", session_id, e)))?;

        Ok(())
    }

    async fn complete_session(
        &self,
        session_id: &SessionId,
        metadata: SessionMetadata,
    ) -> Result<()> {
        // Update session metadata with final information
        let metadata_json = serde_json::to_vec_pretty(&metadata)?;
        fs::write(self.metadata_path(session_id)?, &metadata_json).await
            .map_err(|e| Error::Internal(format!("Failed to write final metadata for session {}: {}", session_id, e)))?;

        tracing::info!(
            session_id = %session_id,
            model = %metadata.model,
            provider = %metadata.provider,
            success = metadata.success,
            latency_seconds = metadata.latency_seconds,
            "Completed recording session"
        );
        Ok(())
    }

    async fn query_sessions(&self, query: &SessionQuery) -> Result<Vec<SessionMetadata>> {
        // Read all session directories
        let mut results = Vec::new();

        let mut entries = fs::read_dir(&self.base_path).await
            .map_err(|e| Error::Internal(format!("Failed to read sessions directory: {}", e)))?;

        while let Some(entry) = entries.next_entry().await
            .map_err(|e| Error::Internal(format!("Failed to read directory entry: {}", e)))? {

            // Use file_type() to avoid following symlinks (security fix)
            let file_type = entry.file_type().await
                .map_err(|e| Error::Internal(format!("Failed to get file type: {}", e)))?;

            if !file_type.is_dir() {
                continue;
            }

            let session_id = match entry.file_name().to_str() {
                Some(id) => id.to_string(),
                None => {
                    tracing::warn!("Skipping session with non-UTF8 name");
                    continue;
                }
            };

            // Validate session ID format
            if let Err(e) = Self::validate_session_id(&session_id) {
                tracing::warn!(session_id = %session_id, error = %e, "Skipping session with invalid ID");
                continue;
            }

            // Read metadata file
            let metadata_path = self.metadata_path(&session_id)?;
            let metadata_data = match fs::read(&metadata_path).await {
                Ok(data) => data,
                Err(e) => {
                    tracing::warn!(session_id = %session_id, error = %e, "Skipping session without readable metadata");
                    continue;
                }
            };

            let metadata: SessionMetadata = match serde_json::from_slice(&metadata_data) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(session_id = %session_id, error = %e, "Skipping session with invalid metadata JSON");
                    continue;
                }
            };

            // Apply filters
            if let Some(model_pattern) = &query.model
                && !metadata.model.contains(model_pattern) {
                    continue;
                }

            if let Some(provider) = &query.provider
                && &metadata.provider != provider {
                    continue;
                }

            if let Some(success) = query.success
                && metadata.success != success {
                    continue;
                }

            if let Some(streaming) = query.streaming
                && metadata.streaming != streaming {
                    continue;
                }

            if let Some(since) = query.since
                && metadata.timestamp < since {
                    continue;
                }

            if let Some(until) = query.until
                && metadata.timestamp > until {
                    continue;
                }

            results.push(metadata);
        }

        // Apply limit
        if let Some(limit) = query.limit {
            results.truncate(limit);
        }

        Ok(results)
    }

    async fn get_session(&self, session_id: &SessionId) -> Result<Option<RecordedSession>> {
        // Validate session ID first
        Self::validate_session_id(session_id)?;

        // Read session metadata
        let metadata_path = self.metadata_path(session_id)?;
        let metadata_data = match fs::read(&metadata_path).await {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(Error::Internal(format!("Failed to read metadata for session {}: {}", session_id, e))),
        };

        let metadata: SessionMetadata = serde_json::from_slice(&metadata_data)
            .map_err(|e| Error::Internal(format!("Invalid metadata JSON for session {}: {}", session_id, e)))?;

        // Read events (NDJSON format)
        let events_path = self.events_path(session_id)?;
        let events_data = match fs::read(&events_path).await {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::Internal(format!("Session {} has no events data", session_id)));
            }
            Err(e) => return Err(Error::Internal(format!("Failed to read events for session {}: {}", session_id, e))),
        };

        let mut lines = events_data.split(|&b| b == b'\n').filter(|line| !line.is_empty());

        // First line is the request
        let request: NormalizedRequest = if let Some(line) = lines.next() {
            serde_json::from_slice(line)?
        } else {
            return Err(Error::Internal("Session has no request data".to_string()));
        };

        let mut response = None;
        let mut stream_events = Vec::new();

        // Remaining lines are either response or stream events
        for line in lines {
            // Try to parse as NormalizedResponse first
            if let Ok(resp) = serde_json::from_slice::<NormalizedResponse>(line) {
                response = Some(resp);
                continue;
            }

            // Try to parse as NormalizedStreamEvent
            if let Ok(event) = serde_json::from_slice::<NormalizedStreamEvent>(line) {
                stream_events.push(event);
            }
        }

        Ok(Some(RecordedSession {
            metadata,
            request,
            response,
            stream_events,
        }))
    }

    async fn delete_session(&self, session_id: &SessionId) -> Result<()> {
        // Validate session ID first
        Self::validate_session_id(session_id)?;

        let session_dir = self.session_dir(session_id)?;
        fs::remove_dir_all(&session_dir).await
            .map_err(|e| Error::Internal(format!("Failed to delete session {}: {}", session_id, e)))?;
        tracing::info!(session_id = %session_id, "Deleted session");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunaroute_core::normalized::{Message, MessageContent, Role, Usage, Choice, FinishReason};
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn create_test_request() -> NormalizedRequest {
        NormalizedRequest {
            model: "gpt-5-mini".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text("test".to_string()),
                name: None,
                tool_calls: vec![],
                tool_call_id: None,
            }],
            system: None,
            max_tokens: Some(100),
            temperature: Some(0.7),
            top_p: None,
            top_k: None,
            stop_sequences: vec![],
            stream: false,
            tools: vec![],
            tool_choice: None,
            metadata: HashMap::new(),
        }
    }

    fn create_test_response() -> NormalizedResponse {
        NormalizedResponse {
            id: "test-response".to_string(),
            model: "gpt-5-mini".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: MessageContent::Text("response".to_string()),
                    name: None,
                    tool_calls: vec![],
                    tool_call_id: None,
                },
                finish_reason: Some(FinishReason::Stop),
            }],
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 20,
                total_tokens: 30,
            },
            created: 1234567890,
            metadata: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn test_session_recorder_lifecycle() {
        let temp_dir = TempDir::new().unwrap();
        let recorder = FileSessionRecorder::new(temp_dir.path());

        let session_id = recorder.generate_session_id();
        let request = create_test_request();
        let response = create_test_response();

        // Start session
        let metadata = SessionMetadata::new(
            session_id.clone(),
            "gpt-5-mini".to_string(),
            "openai".to_string(),
            "openai".to_string(),
        );

        recorder.start_session(session_id.clone(), &request, metadata.clone()).await.unwrap();

        // Record response
        recorder.record_response(&session_id, &response).await.unwrap();

        // Complete session
        let final_metadata = metadata
            .with_usage(10, 20)
            .with_success(1.5, Some("stop".to_string()));

        recorder.complete_session(&session_id, final_metadata).await.unwrap();

        // Retrieve session
        let recorded = recorder.get_session(&session_id).await.unwrap().unwrap();

        assert_eq!(recorded.request.model, "gpt-5-mini");
        assert!(recorded.response.is_some());
        assert_eq!(recorded.response.unwrap().id, "test-response");
        assert_eq!(recorded.metadata.total_tokens, Some(30));
    }

    #[tokio::test]
    async fn test_session_query() {
        let temp_dir = TempDir::new().unwrap();
        let recorder = FileSessionRecorder::new(temp_dir.path());

        // Create multiple sessions
        for i in 0..5 {
            let session_id = recorder.generate_session_id();
            let request = create_test_request();

            let metadata = SessionMetadata::new(
                session_id.clone(),
                if i < 3 { "gpt-5-mini" } else { "claude-sonnet-4-5" }.to_string(),
                if i < 3 { "openai" } else { "anthropic" }.to_string(),
                "openai".to_string(),
            )
            .with_success(1.0, Some("stop".to_string()));

            recorder.start_session(session_id.clone(), &request, metadata.clone()).await.unwrap();
            recorder.complete_session(&session_id, metadata).await.unwrap();
        }

        // Query OpenAI sessions
        let query = SessionQuery::new().provider("openai".to_string());
        let results = recorder.query_sessions(&query).await.unwrap();

        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|m| m.provider == "openai"));
    }

    #[tokio::test]
    async fn test_session_deletion() {
        let temp_dir = TempDir::new().unwrap();
        let recorder = FileSessionRecorder::new(temp_dir.path());

        let session_id = recorder.generate_session_id();
        let request = create_test_request();

        let metadata = SessionMetadata::new(
            session_id.clone(),
            "gpt-5-mini".to_string(),
            "openai".to_string(),
            "openai".to_string(),
        );

        recorder.start_session(session_id.clone(), &request, metadata).await.unwrap();

        // Verify session exists
        assert!(recorder.get_session(&session_id).await.unwrap().is_some());

        // Delete session
        recorder.delete_session(&session_id).await.unwrap();

        // Verify session is gone
        assert!(recorder.get_session(&session_id).await.unwrap().is_none());
    }
}
