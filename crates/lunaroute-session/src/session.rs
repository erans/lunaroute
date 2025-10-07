//! Session management and metadata
//!
//! This module defines the Session type and related metadata structures.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Anonymize an IP address for privacy compliance
///
/// For IPv4: zeros out the last octet (e.g., "192.168.1.100" -> "192.168.1.0")
/// For IPv6: zeros out the last 80 bits (e.g., "2001:db8::1" -> "2001:db8::")
pub fn anonymize_ip(ip: &str) -> String {
    // Try to parse as IP address
    if let Ok(addr) = ip.parse::<std::net::IpAddr>() {
        match addr {
            std::net::IpAddr::V4(v4) => {
                let octets = v4.octets();
                format!("{}.{}.{}.0", octets[0], octets[1], octets[2])
            }
            std::net::IpAddr::V6(v6) => {
                let segments = v6.segments();
                format!("{}:{}:{}::", segments[0], segments[1], segments[2])
            }
        }
    } else {
        // If not a valid IP, return as-is (might be hostname)
        ip.to_string()
    }
}

/// Session identifier (UUID format)
pub type SessionId = String;

/// Session metadata
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionMetadata {
    /// Session ID
    pub id: SessionId,

    /// Model requested
    pub model: String,

    /// Provider used (e.g., "openai", "anthropic")
    pub provider: String,

    /// Listener type (e.g., "openai", "anthropic")
    pub listener: String,

    /// Whether the request was streaming
    pub streaming: bool,

    /// Request timestamp (Unix timestamp in seconds)
    pub timestamp: i64,

    /// Request latency in seconds
    pub latency_seconds: f64,

    /// Total tokens used (if available)
    pub total_tokens: Option<u32>,

    /// Prompt tokens
    pub prompt_tokens: Option<u32>,

    /// Completion tokens
    pub completion_tokens: Option<u32>,

    /// Whether the request succeeded
    pub success: bool,

    /// Error message if failed
    pub error: Option<String>,

    /// Finish reason (if completed successfully)
    pub finish_reason: Option<String>,

    /// Request ID from ingress
    pub request_id: Option<String>,

    /// User agent
    pub user_agent: Option<String>,

    /// Client IP address
    pub client_ip: Option<String>,

    /// Custom metadata
    pub custom: HashMap<String, String>,
}

impl SessionMetadata {
    /// Create a new session metadata
    pub fn new(id: SessionId, model: String, provider: String, listener: String) -> Self {
        Self {
            id,
            model,
            provider,
            listener,
            streaming: false,
            timestamp: chrono::Utc::now().timestamp(),
            latency_seconds: 0.0,
            total_tokens: None,
            prompt_tokens: None,
            completion_tokens: None,
            success: false,
            error: None,
            finish_reason: None,
            request_id: None,
            user_agent: None,
            client_ip: None,
            custom: HashMap::new(),
        }
    }

    /// Mark as streaming session
    pub fn with_streaming(mut self, streaming: bool) -> Self {
        self.streaming = streaming;
        self
    }

    /// Set request ID
    pub fn with_request_id(mut self, request_id: String) -> Self {
        self.request_id = Some(request_id);
        self
    }

    /// Set user agent
    pub fn with_user_agent(mut self, user_agent: String) -> Self {
        self.user_agent = Some(user_agent);
        self
    }

    /// Set client IP (stores the raw IP - use with_client_ip_anonymized for privacy)
    pub fn with_client_ip(mut self, client_ip: String) -> Self {
        self.client_ip = Some(client_ip);
        self
    }

    /// Set client IP with automatic anonymization for privacy compliance
    ///
    /// This is the recommended method for GDPR/privacy compliance.
    /// IPv4: zeros last octet (192.168.1.100 -> 192.168.1.0)
    /// IPv6: zeros last 80 bits (2001:db8::1 -> 2001:db8::)
    pub fn with_client_ip_anonymized(mut self, client_ip: String) -> Self {
        self.client_ip = Some(anonymize_ip(&client_ip));
        self
    }

    /// Update with usage statistics
    pub fn with_usage(mut self, prompt_tokens: u32, completion_tokens: u32) -> Self {
        self.prompt_tokens = Some(prompt_tokens);
        self.completion_tokens = Some(completion_tokens);
        self.total_tokens = Some(prompt_tokens + completion_tokens);
        self
    }

    /// Mark as successful with latency
    pub fn with_success(mut self, latency_seconds: f64, finish_reason: Option<String>) -> Self {
        self.success = true;
        self.latency_seconds = latency_seconds;
        self.finish_reason = finish_reason;
        self
    }

    /// Mark as failed with error
    pub fn with_error(mut self, error: String, latency_seconds: f64) -> Self {
        self.success = false;
        self.error = Some(error);
        self.latency_seconds = latency_seconds;
        self
    }

    /// Add custom metadata
    pub fn with_custom(mut self, key: String, value: String) -> Self {
        self.custom.insert(key, value);
        self
    }
}

/// Session query filter
#[derive(Debug, Clone, Default)]
pub struct SessionQuery {
    /// Filter by model pattern (regex)
    pub model: Option<String>,

    /// Filter by provider
    pub provider: Option<String>,

    /// Filter by success status
    pub success: Option<bool>,

    /// Filter by streaming status
    pub streaming: Option<bool>,

    /// Minimum timestamp
    pub since: Option<i64>,

    /// Maximum timestamp
    pub until: Option<i64>,

    /// Limit number of results
    pub limit: Option<usize>,
}

impl SessionQuery {
    /// Create a new empty query
    pub fn new() -> Self {
        Self::default()
    }

    /// Filter by model
    pub fn model(mut self, model: String) -> Self {
        self.model = Some(model);
        self
    }

    /// Filter by provider
    pub fn provider(mut self, provider: String) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Filter by success status
    pub fn success(mut self, success: bool) -> Self {
        self.success = Some(success);
        self
    }

    /// Filter by streaming status
    pub fn streaming(mut self, streaming: bool) -> Self {
        self.streaming = Some(streaming);
        self
    }

    /// Filter since timestamp
    pub fn since(mut self, timestamp: i64) -> Self {
        self.since = Some(timestamp);
        self
    }

    /// Filter until timestamp
    pub fn until(mut self, timestamp: i64) -> Self {
        self.until = Some(timestamp);
        self
    }

    /// Limit results
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_metadata_creation() {
        let metadata = SessionMetadata::new(
            "test-session-id".to_string(),
            "gpt-5-mini".to_string(),
            "openai".to_string(),
            "openai".to_string(),
        );

        assert_eq!(metadata.id, "test-session-id");
        assert_eq!(metadata.model, "gpt-5-mini");
        assert_eq!(metadata.provider, "openai");
        assert!(!metadata.streaming);
        assert!(!metadata.success);
    }

    #[test]
    fn test_session_metadata_builder() {
        let metadata = SessionMetadata::new(
            "test-id".to_string(),
            "claude-sonnet-4-5".to_string(),
            "anthropic".to_string(),
            "anthropic".to_string(),
        )
        .with_streaming(true)
        .with_usage(100, 50)
        .with_success(1.5, Some("stop".to_string()))
        .with_request_id("req-123".to_string());

        assert!(metadata.streaming);
        assert!(metadata.success);
        assert_eq!(metadata.prompt_tokens, Some(100));
        assert_eq!(metadata.completion_tokens, Some(50));
        assert_eq!(metadata.total_tokens, Some(150));
        assert_eq!(metadata.latency_seconds, 1.5);
        assert_eq!(metadata.finish_reason, Some("stop".to_string()));
        assert_eq!(metadata.request_id, Some("req-123".to_string()));
    }

    #[test]
    fn test_session_metadata_error() {
        let metadata = SessionMetadata::new(
            "test-id".to_string(),
            "gpt-5-mini".to_string(),
            "openai".to_string(),
            "openai".to_string(),
        )
        .with_error("Provider timeout".to_string(), 30.0);

        assert!(!metadata.success);
        assert_eq!(metadata.error, Some("Provider timeout".to_string()));
        assert_eq!(metadata.latency_seconds, 30.0);
    }

    #[test]
    fn test_session_query_builder() {
        let query = SessionQuery::new()
            .model("gpt-.*".to_string())
            .provider("openai".to_string())
            .success(true)
            .streaming(false)
            .limit(10);

        assert_eq!(query.model, Some("gpt-.*".to_string()));
        assert_eq!(query.provider, Some("openai".to_string()));
        assert_eq!(query.success, Some(true));
        assert_eq!(query.streaming, Some(false));
        assert_eq!(query.limit, Some(10));
    }

    #[test]
    fn test_session_metadata_serialization() {
        let metadata = SessionMetadata::new(
            "test-id".to_string(),
            "gpt-5-mini".to_string(),
            "openai".to_string(),
            "openai".to_string(),
        )
        .with_success(1.0, Some("stop".to_string()));

        let json = serde_json::to_string(&metadata).unwrap();
        let deserialized: SessionMetadata = serde_json::from_str(&json).unwrap();

        assert_eq!(metadata, deserialized);
    }

    #[test]
    fn test_session_metadata_custom_fields() {
        let metadata = SessionMetadata::new(
            "test-id".to_string(),
            "gpt-5-mini".to_string(),
            "openai".to_string(),
            "openai".to_string(),
        )
        .with_custom("tenant_id".to_string(), "tenant-123".to_string())
        .with_custom("environment".to_string(), "production".to_string());

        assert_eq!(
            metadata.custom.get("tenant_id"),
            Some(&"tenant-123".to_string())
        );
        assert_eq!(
            metadata.custom.get("environment"),
            Some(&"production".to_string())
        );
    }
}
