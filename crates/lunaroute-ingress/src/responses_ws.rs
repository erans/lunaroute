//! WebSocket ingress for OpenAI's Responses API.
//!
//! Codex CLI (with `supports_websockets = true`) opens a WebSocket to
//! `/v1/responses`. This module accepts the upgrade, parses `response.create`
//! frames, and drives the same HTTP streaming pipeline used by
//! `openai::responses_passthrough` via `openai::responses_sse_stream`.
//!
//! See `docs/superpowers/specs/2026-04-16-codex-websocket-responses-design.md`.

use serde::Deserialize;

/// Parsed client-to-server WebSocket frame.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) enum ClientEvent {
    /// `{"type": "response.create", "response": {...}}` — create a response.
    /// The inner `response` object is the usual Responses API create payload.
    ResponseCreate { response: serde_json::Value },
}

/// Error returned by `parse_client_frame`.
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub(crate) enum FrameError {
    #[error("malformed JSON: {0}")]
    MalformedJson(#[from] serde_json::Error),
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("unsupported event type: {0}")]
    UnsupportedType(String),
}

/// Parse a client text frame into a `ClientEvent`.
///
/// Accepted shape: `{"type": "response.create", "response": {...}}` with
/// `response` being any JSON object (the Responses API create body). Anything
/// else returns a `FrameError` the caller maps to a structured error frame.
#[allow(dead_code)]
pub(crate) fn parse_client_frame(text: &str) -> Result<ClientEvent, FrameError> {
    #[derive(Deserialize)]
    struct Envelope {
        r#type: String,
        #[serde(default)]
        response: Option<serde_json::Value>,
    }

    let envelope: Envelope = serde_json::from_str(text)?;
    match envelope.r#type.as_str() {
        "response.create" => {
            let response = envelope
                .response
                .ok_or(FrameError::MissingField("response"))?;
            if !response.is_object() {
                return Err(FrameError::MissingField("response (must be object)"));
            }
            Ok(ClientEvent::ResponseCreate { response })
        }
        other => Err(FrameError::UnsupportedType(other.to_string())),
    }
}

/// Build a server-side error frame payload in the Responses API event shape.
#[allow(dead_code)]
pub(crate) fn error_frame(code: &str, message: &str) -> String {
    serde_json::json!({
        "type": "error",
        "error": {
            "code": code,
            "message": message,
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_response_create() {
        let text = r#"{"type":"response.create","response":{"model":"gpt-5","input":"hi"}}"#;
        let ev = parse_client_frame(text).unwrap();
        match ev {
            ClientEvent::ResponseCreate { response } => {
                assert_eq!(
                    response.get("model").and_then(|v| v.as_str()),
                    Some("gpt-5")
                );
            }
        }
    }

    #[test]
    fn rejects_unknown_type() {
        let text = r#"{"type":"response.cancel"}"#;
        let err = parse_client_frame(text).unwrap_err();
        assert!(matches!(err, FrameError::UnsupportedType(ref t) if t == "response.cancel"));
    }

    #[test]
    fn rejects_missing_response_field() {
        let text = r#"{"type":"response.create"}"#;
        let err = parse_client_frame(text).unwrap_err();
        assert!(matches!(err, FrameError::MissingField("response")));
    }

    #[test]
    fn rejects_non_object_response() {
        let text = r#"{"type":"response.create","response":"not-an-object"}"#;
        let err = parse_client_frame(text).unwrap_err();
        assert!(matches!(err, FrameError::MissingField(_)));
    }

    #[test]
    fn rejects_malformed_json() {
        let err = parse_client_frame("{not json").unwrap_err();
        assert!(matches!(err, FrameError::MalformedJson(_)));
    }

    #[test]
    fn error_frame_has_expected_shape() {
        let frame = error_frame("unsupported_event_type", "nope");
        let value: serde_json::Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(value.get("type").and_then(|v| v.as_str()), Some("error"));
        assert_eq!(
            value
                .get("error")
                .and_then(|e| e.get("code"))
                .and_then(|v| v.as_str()),
            Some("unsupported_event_type")
        );
    }
}
