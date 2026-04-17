//! WebSocket ingress for OpenAI's Responses API.
//!
//! Codex CLI (with `supports_websockets = true`) opens a WebSocket to
//! `/v1/responses`. This module accepts the upgrade, parses `response.create`
//! frames, and drives the same HTTP streaming pipeline used by
//! `openai::responses_passthrough` via `openai::responses_sse_stream`.
//!
//! See `docs/superpowers/specs/2026-04-16-codex-websocket-responses-design.md`.

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::Response;
use futures::StreamExt as _;
use serde::Deserialize;
use std::sync::Arc;

use crate::openai::{OpenAIPassthroughState, SseEvent, responses_sse_stream};

/// Parsed client-to-server WebSocket frame.
#[derive(Debug, Clone)]
pub(crate) enum ClientEvent {
    /// `{"type": "response.create", "response": {...}}` — create a response.
    /// The inner `response` object is the usual Responses API create payload.
    ResponseCreate { response: serde_json::Value },
}

/// Error returned by `parse_client_frame`.
#[derive(Debug, thiserror::Error)]
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

/// Terminal Responses API event types — receiving one of these means the
/// current response is finished and the read loop may accept the next
/// `response.create` frame on the same connection.
const TERMINAL_EVENTS: &[&str] = &[
    "response.completed",
    "response.failed",
    "response.incomplete",
    "response.cancelled",
];

/// axum handler: accept the WebSocket upgrade on `/responses` or
/// `/v1/responses` and spawn a per-connection session.
pub async fn responses_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<OpenAIPassthroughState>>,
    headers: HeaderMap,
) -> Response {
    tracing::debug!("Responses API WebSocket upgrade");
    ws.on_upgrade(move |socket| run_ws_session(socket, state, headers))
}

/// Own the socket for a single WebSocket connection. Reads client frames,
/// runs each `response.create` through the shared SSE pipeline, sends each
/// resulting event back as a WS text frame. Sequential by construction — we
/// await each stream to completion before the next `recv`.
async fn run_ws_session(
    mut socket: WebSocket,
    state: Arc<OpenAIPassthroughState>,
    upgrade_headers: HeaderMap,
) {
    const ENDPOINT: &str = "responses";
    let started = std::time::Instant::now();
    if let Some(metrics) = &state.metrics {
        metrics.record_ws_connection_opened(ENDPOINT);
    }

    tracing::debug!("WS session started");

    while let Some(msg) = socket.recv().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("WS recv error: {e}");
                break;
            }
        };

        // Note: `ws_frames_total` tracks application-level activity. Text
        // frames are counted inside `handle_client_text` (once per inbound
        // frame, labeled by parse outcome), plus Binary frames here as
        // `"binary"`. Ping/Pong/Close are transport-level frames — not counted,
        // since they aren't useful signals for application dashboards.
        match msg {
            Message::Text(text) => {
                if let Err(e) =
                    handle_client_text(&mut socket, &state, &upgrade_headers, text.as_ref()).await
                {
                    tracing::warn!("WS text handling error: {e}");
                    // Connection stays open unless the error indicates a send failure
                    // (in which case the next recv will fail too).
                }
            }
            Message::Close(_) => {
                tracing::debug!("WS client closed");
                break;
            }
            Message::Ping(_) | Message::Pong(_) => {
                // axum handles ping/pong automatically; nothing to do.
            }
            Message::Binary(_) => {
                if let Some(metrics) = &state.metrics {
                    metrics.record_ws_frame(ENDPOINT, "client", "binary");
                }
                let _ = send_error(
                    &mut socket,
                    &state,
                    "unsupported_frame_type",
                    "binary frames are not supported",
                )
                .await;
            }
        }
    }

    if let Some(metrics) = &state.metrics {
        metrics.record_ws_connection_closed(ENDPOINT, started.elapsed().as_secs_f64());
    }
    tracing::debug!("WS session ended after {:?}", started.elapsed());
}

/// Dispatch one client text frame: parse, run the pipeline, forward events.
async fn handle_client_text(
    socket: &mut WebSocket,
    state: &Arc<OpenAIPassthroughState>,
    upgrade_headers: &HeaderMap,
    text: &str,
) -> Result<(), axum::Error> {
    const ENDPOINT: &str = "responses";
    let event = match parse_client_frame(text) {
        Ok(e) => e,
        Err(FrameError::MalformedJson(e)) => {
            if let Some(metrics) = &state.metrics {
                metrics.record_ws_frame(ENDPOINT, "client", "malformed_json");
            }
            return send_error(socket, state, "malformed_json", &e.to_string()).await;
        }
        Err(FrameError::MissingField(f)) => {
            if let Some(metrics) = &state.metrics {
                metrics.record_ws_frame(ENDPOINT, "client", "invalid_request");
            }
            return send_error(
                socket,
                state,
                "invalid_request",
                &format!("missing field: {f}"),
            )
            .await;
        }
        Err(FrameError::UnsupportedType(t)) => {
            if let Some(metrics) = &state.metrics {
                metrics.record_ws_frame(ENDPOINT, "client", "unsupported_event_type");
            }
            return send_error(
                socket,
                state,
                "unsupported_event_type",
                &format!("unsupported event type: {t}"),
            )
            .await;
        }
    };

    match event {
        ClientEvent::ResponseCreate { mut response } => {
            if let Some(metrics) = &state.metrics {
                metrics.record_ws_frame(ENDPOINT, "client", "response.create");
            }
            // Force stream=true unconditionally: the WebSocket transport is
            // inherently streaming; a client-supplied {"stream": false} would
            // break the pipeline.
            response["stream"] = serde_json::Value::Bool(true);
            let body_bytes = match serde_json::to_vec(&response) {
                Ok(b) => axum::body::Bytes::from(b),
                Err(e) => {
                    return send_error(socket, state, "internal_error", &e.to_string()).await;
                }
            };

            // The upgrade is a GET with no body, so Content-Type is absent.
            // Inject it so the upstream POST is handled as JSON.
            let mut ws_headers = upgrade_headers.clone();
            ws_headers.insert(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("application/json"),
            );

            let stream = match responses_sse_stream(state.clone(), ws_headers, body_bytes).await {
                Ok(s) => s,
                Err(e) => {
                    return send_error(socket, state, "upstream_error", &e.to_string()).await;
                }
            };

            forward_stream(socket, state, stream).await
        }
    }
}

/// Forward a stream of `SseEvent`s as WebSocket text frames until a terminal
/// event is seen or the stream ends.
async fn forward_stream(
    socket: &mut WebSocket,
    state: &Arc<OpenAIPassthroughState>,
    mut stream: futures::stream::BoxStream<'static, Result<SseEvent, crate::IngressError>>,
) -> Result<(), axum::Error> {
    const ENDPOINT: &str = "responses";
    while let Some(result) = stream.next().await {
        match result {
            Ok(ev) => {
                if let Some(metrics) = &state.metrics {
                    let ty = event_label(&ev);
                    metrics.record_ws_frame(ENDPOINT, "server", &ty);
                }
                // Send just the `data` payload — the event name is already
                // embedded in the JSON's `type` field per the Responses WS spec.
                socket.send(Message::Text(ev.data.clone().into())).await?;
                if is_terminal(&ev) {
                    return Ok(());
                }
            }
            Err(e) => {
                return send_error(socket, state, "stream_error", &e.to_string()).await;
            }
        }
    }
    // Stream ended with no terminal event — emit a synthetic error so the
    // client doesn't hang waiting.
    send_error(
        socket,
        state,
        "stream_ended",
        "upstream stream ended without a terminal event",
    )
    .await
}

/// Derive the `type` label for `ws_frames_total` from a server-side SSE event.
///
/// Prefers the Responses API event type embedded in `ev.data`'s `type` field
/// (e.g. `response.output_text.delta`, `response.completed`). Falls back to
/// the SSE `event:` name, and finally to `"message"` for an otherwise opaque
/// frame. Keeps cardinality bounded to the Responses API vocabulary.
fn event_label(ev: &SseEvent) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&ev.data)
        && let Some(t) = value.get("type").and_then(|v| v.as_str())
        && !t.is_empty()
    {
        return t.to_string();
    }
    if !ev.event.is_empty() {
        return ev.event.clone();
    }
    "message".to_string()
}

fn is_terminal(ev: &SseEvent) -> bool {
    if TERMINAL_EVENTS.contains(&ev.event.as_str()) {
        return true;
    }
    // Fall back to inspecting the JSON `type` field inside `data`.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&ev.data)
        && let Some(t) = value.get("type").and_then(|v| v.as_str())
    {
        return TERMINAL_EVENTS.contains(&t);
    }
    false
}

async fn send_error(
    socket: &mut WebSocket,
    state: &Arc<OpenAIPassthroughState>,
    code: &str,
    message: &str,
) -> Result<(), axum::Error> {
    // Error frames are server-originated and represent abnormal application
    // activity — count them so they show up in dashboards alongside normal
    // `response.*` event types.
    if let Some(metrics) = &state.metrics {
        metrics.record_ws_frame("responses", "server", "error");
    }
    socket
        .send(Message::Text(error_frame(code, message).into()))
        .await
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

    // Note: integration tests that exercise the `ws_frames_total`
    // instrumentation end-to-end would require threading a real `Metrics`
    // through the test `passthrough_router`, which is out of scope for this
    // fix. The unit tests below pin the `event_label` precedence that those
    // instrumentation paths depend on.

    #[test]
    fn event_label_prefers_json_type_over_sse_event() {
        let ev = SseEvent {
            event: "response.output_text.delta".to_string(),
            data: r#"{"type":"response.completed"}"#.to_string(),
        };
        assert_eq!(event_label(&ev), "response.completed");
    }

    #[test]
    fn event_label_falls_back_to_sse_event_when_data_not_json() {
        let ev = SseEvent {
            event: "response.output_text.delta".to_string(),
            data: "not-json".to_string(),
        };
        assert_eq!(event_label(&ev), "response.output_text.delta");
    }

    #[test]
    fn event_label_falls_back_to_sse_event_when_json_has_no_type() {
        let ev = SseEvent {
            event: "custom.event".to_string(),
            data: r#"{"foo":"bar"}"#.to_string(),
        };
        assert_eq!(event_label(&ev), "custom.event");
    }

    #[test]
    fn event_label_falls_back_to_sse_event_when_json_type_empty() {
        let ev = SseEvent {
            event: "custom.event".to_string(),
            data: r#"{"type":""}"#.to_string(),
        };
        assert_eq!(event_label(&ev), "custom.event");
    }

    #[test]
    fn event_label_returns_message_when_both_empty() {
        let ev = SseEvent {
            event: String::new(),
            data: String::new(),
        };
        assert_eq!(event_label(&ev), "message");
    }

    #[test]
    fn event_label_returns_message_when_data_not_json_and_event_empty() {
        let ev = SseEvent {
            event: String::new(),
            data: "not-json".to_string(),
        };
        assert_eq!(event_label(&ev), "message");
    }
}
