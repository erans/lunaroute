//! Shared ingress types and utilities

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Request ID for tracing
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RequestId(String);

impl RequestId {
    /// Generate a new request ID
    pub fn generate() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros();

        Self(format!("req_{:x}_{:x}", timestamp, count))
    }

    /// Create from existing string
    pub fn from_string(s: String) -> Self {
        Self(s)
    }

    /// Get the string value
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Trace context for distributed tracing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceContext {
    /// W3C Trace ID
    pub trace_id: String,
    /// W3C Span ID
    pub span_id: String,
    /// Parent span ID
    pub parent_span_id: Option<String>,
    /// Trace flags
    pub trace_flags: u8,
}

impl TraceContext {
    /// Generate a new trace context
    pub fn generate() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros();

        Self {
            trace_id: format!("{:032x}", timestamp ^ (count as u128)),
            span_id: format!("{:016x}", count),
            parent_span_id: None,
            trace_flags: 1, // Sampled
        }
    }

    /// Parse from W3C traceparent header
    pub fn from_traceparent(header: &str) -> Option<Self> {
        let parts: Vec<&str> = header.split('-').collect();
        if parts.len() != 4 {
            return None;
        }

        let trace_id = parts[1].to_string();
        let parent_span_id = parts[2].to_string();
        let trace_flags = u8::from_str_radix(parts[3], 16).ok()?;

        Some(Self {
            trace_id,
            span_id: format!("{:016x}", rand::random::<u64>()),
            parent_span_id: Some(parent_span_id),
            trace_flags,
        })
    }

    /// Format as W3C traceparent header
    pub fn to_traceparent(&self) -> String {
        format!(
            "00-{}-{}-{:02x}",
            self.trace_id, self.span_id, self.trace_flags
        )
    }
}

/// Ingress error types
#[derive(Debug, Error)]
pub enum IngressError {
    /// Invalid request format
    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    /// Missing required header
    #[error("Missing required header: {0}")]
    MissingHeader(String),

    /// Authentication failed
    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),

    /// Request too large
    #[error("Request too large: {0} bytes")]
    RequestTooLarge(usize),

    /// Timeout
    #[error("Request timeout")]
    Timeout,

    /// Serialization error
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Internal error
    #[error("Internal error: {0}")]
    Internal(String),
}

impl axum::response::IntoResponse for IngressError {
    fn into_response(self) -> axum::response::Response {
        use axum::http::StatusCode;

        let (status, message) = match self {
            IngressError::InvalidRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            IngressError::MissingHeader(msg) => (StatusCode::BAD_REQUEST, msg),
            IngressError::AuthenticationFailed(msg) => (StatusCode::UNAUTHORIZED, msg),
            IngressError::RequestTooLarge(size) => {
                (StatusCode::PAYLOAD_TOO_LARGE, format!("Request too large: {} bytes", size))
            }
            IngressError::Timeout => (StatusCode::REQUEST_TIMEOUT, "Request timeout".to_string()),
            IngressError::Serialization(err) => {
                (StatusCode::BAD_REQUEST, format!("Serialization error: {}", err))
            }
            IngressError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        let body = serde_json::json!({
            "error": {
                "message": message,
                "type": "invalid_request_error",
                "code": status.as_u16(),
            }
        });

        (status, axum::Json(body)).into_response()
    }
}

/// Ingress result type
pub type IngressResult<T> = Result<T, IngressError>;

/// Request metadata collected during ingress
#[derive(Debug, Clone)]
pub struct RequestMetadata {
    /// Request ID
    pub request_id: RequestId,
    /// Trace context
    pub trace_context: TraceContext,
    /// Client IP address
    pub client_ip: Option<String>,
    /// User agent
    pub user_agent: Option<String>,
    /// Authenticated user/key ID
    pub auth_id: Option<String>,
    /// Request timestamp
    pub timestamp: i64,
}

impl RequestMetadata {
    /// Create new request metadata
    pub fn new() -> Self {
        Self {
            request_id: RequestId::generate(),
            trace_context: TraceContext::generate(),
            client_ip: None,
            user_agent: None,
            auth_id: None,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
        }
    }

    /// Set trace context from traceparent header
    pub fn with_traceparent(mut self, traceparent: &str) -> Self {
        if let Some(ctx) = TraceContext::from_traceparent(traceparent) {
            self.trace_context = ctx;
        }
        self
    }

    /// Set client IP
    pub fn with_client_ip(mut self, ip: String) -> Self {
        self.client_ip = Some(ip);
        self
    }

    /// Set user agent
    pub fn with_user_agent(mut self, ua: String) -> Self {
        self.user_agent = Some(ua);
        self
    }

    /// Set auth ID
    pub fn with_auth_id(mut self, id: String) -> Self {
        self.auth_id = Some(id);
        self
    }
}

impl Default for RequestMetadata {
    fn default() -> Self {
        Self::new()
    }
}

/// Stream event wrapper
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEvent {
    /// Event data as JSON
    pub data: serde_json::Value,
    /// Event type (optional)
    pub event: Option<String>,
    /// Event ID (optional)
    pub id: Option<String>,
}

impl StreamEvent {
    /// Create a new stream event
    pub fn new(data: serde_json::Value) -> Self {
        Self {
            data,
            event: None,
            id: None,
        }
    }

    /// Format as SSE (Server-Sent Events)
    pub fn to_sse(&self) -> String {
        let mut output = String::new();

        if let Some(ref event) = self.event {
            output.push_str(&format!("event: {}\n", event));
        }

        if let Some(ref id) = self.id {
            output.push_str(&format!("id: {}\n", id));
        }

        output.push_str(&format!("data: {}\n\n", self.data));
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_id_generation() {
        let id1 = RequestId::generate();
        let id2 = RequestId::generate();
        assert_ne!(id1, id2);
        assert!(id1.as_str().starts_with("req_"));
    }

    #[test]
    fn test_request_id_from_string() {
        let id = RequestId::from_string("test_123".to_string());
        assert_eq!(id.as_str(), "test_123");
    }

    #[test]
    fn test_trace_context_generation() {
        let ctx = TraceContext::generate();
        assert_eq!(ctx.trace_id.len(), 32);
        assert_eq!(ctx.span_id.len(), 16);
        assert_eq!(ctx.trace_flags, 1);
    }

    #[test]
    fn test_trace_context_traceparent() {
        let ctx = TraceContext::generate();
        let header = ctx.to_traceparent();
        assert!(header.starts_with("00-"));

        let parsed = TraceContext::from_traceparent(&header).unwrap();
        assert_eq!(parsed.trace_id, ctx.trace_id);
    }

    #[test]
    fn test_request_metadata_creation() {
        let meta = RequestMetadata::new();
        assert!(meta.request_id.as_str().starts_with("req_"));
        assert!(meta.timestamp > 0);
    }

    #[test]
    fn test_request_metadata_builder() {
        let meta = RequestMetadata::new()
            .with_client_ip("127.0.0.1".to_string())
            .with_user_agent("test-agent".to_string())
            .with_auth_id("user_123".to_string());

        assert_eq!(meta.client_ip, Some("127.0.0.1".to_string()));
        assert_eq!(meta.user_agent, Some("test-agent".to_string()));
        assert_eq!(meta.auth_id, Some("user_123".to_string()));
    }

    #[test]
    fn test_stream_event_to_sse() {
        let event = StreamEvent {
            data: serde_json::json!({"test": "value"}),
            event: Some("message".to_string()),
            id: Some("1".to_string()),
        };

        let sse = event.to_sse();
        assert!(sse.contains("event: message\n"));
        assert!(sse.contains("id: 1\n"));
        assert!(sse.contains("data: {"));
    }
}
