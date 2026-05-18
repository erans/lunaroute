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
use std::sync::Arc;

use crate::openai::{OpenAIPassthroughState, SseEvent, responses_sse_stream};

/// Parsed client-to-server WebSocket frame.
#[derive(Debug, Clone)]
pub(crate) enum ClientEvent {
    /// `{"type": "response.create", ...}` — create a response.
    ///
    /// Codex (the only known WS client) sends a flat payload tagged by `type`
    /// (`#[serde(tag = "type")]`), not a nested envelope. `response` holds every
    /// field except `type`, `generate`, and `client_metadata`, and is forwarded
    /// as the body of the upstream Responses API POST.
    ResponseCreate {
        response: serde_json::Value,
        /// `generate: false` — Codex's warmup ping sent during session startup
        /// to prime the connection. Short-circuits locally so we don't burn an
        /// upstream call on every Codex launch.
        warmup: bool,
    },
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
/// Accepted shape: `{"type": "response.create", "model": "...", "input": [...], ...}` —
/// the fields of the Responses API create body are at the top level, next to
/// `type`. This matches Codex's `ResponsesWsRequest` enum (`#[serde(tag =
/// "type")]`) in `codex-rs/codex-api/src/common.rs`.
///
/// Codex-only fields are stripped before forwarding upstream:
/// * `generate: false` signals a warmup ping — we short-circuit on the
///   `warmup` flag rather than forwarding.
/// * `client_metadata` carries traceparent/tracestate; the Responses API
///   doesn't accept it.
pub(crate) fn parse_client_frame(text: &str) -> Result<ClientEvent, FrameError> {
    let value: serde_json::Value = serde_json::from_str(text)?;
    let obj = match value {
        serde_json::Value::Object(map) => map,
        _ => {
            return Err(FrameError::MissingField(
                "type (frame must be a JSON object)",
            ));
        }
    };
    let mut obj = obj;

    let type_field = obj
        .remove("type")
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or(FrameError::MissingField("type"))?;

    match type_field.as_str() {
        "response.create" => {
            let warmup = obj
                .remove("generate")
                .and_then(|v| v.as_bool())
                .map(|generate| !generate)
                .unwrap_or(false);
            obj.remove("client_metadata");
            Ok(ClientEvent::ResponseCreate {
                response: serde_json::Value::Object(obj),
                warmup,
            })
        }
        other => Err(FrameError::UnsupportedType(other.to_string())),
    }
}

/// Build a synthetic `response.completed` frame for a warmup ping.
///
/// Codex's `prewarm_websocket` loop exits when it sees a `ResponseEvent::Completed`
/// event, which `process_responses_event` only yields if the frame has
/// `type == "response.completed"` and a `response` object containing an `id`.
/// No upstream call happens — this is sent straight back to the client.
fn warmup_completed_frame() -> String {
    serde_json::json!({
        "type": "response.completed",
        "response": {
            "id": format!("warmup-{}", uuid::Uuid::new_v4()),
        },
    })
    .to_string()
}

/// Strip a synthetic `previous_response_id` before forwarding upstream.
///
/// Codex's session state captures the `id` from each `response.completed` and
/// echoes it as `previous_response_id` on the next turn. Our warmup's synthetic
/// id (`warmup-<uuid>`) is not known upstream, which would 400 with
/// "Unsupported parameter: previous_response_id". Dropping it makes the first
/// real turn behave like a fresh conversation; real ids from upstream on
/// subsequent turns pass through untouched.
fn strip_synthetic_warmup_previous_id(response: &mut serde_json::Value) {
    let is_warmup_id = response
        .get("previous_response_id")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s.starts_with("warmup-"));
    if is_warmup_id && let Some(obj) = response.as_object_mut() {
        obj.remove("previous_response_id");
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
        ClientEvent::ResponseCreate {
            mut response,
            warmup,
        } => {
            if let Some(metrics) = &state.metrics {
                let label = if warmup {
                    "response.create.warmup"
                } else {
                    "response.create"
                };
                metrics.record_ws_frame(ENDPOINT, "client", label);
            }
            // Codex sends a warmup ping (`generate: false`) on session startup
            // to prime the connection. Short-circuit locally — forwarding it
            // upstream would burn a real Responses API call on every Codex
            // launch. Codex's `prewarm_websocket` loop exits as soon as it
            // sees a `response.completed` event.
            if warmup {
                return socket
                    .send(Message::Text(warmup_completed_frame().into()))
                    .await;
            }
            // Force stream=true unconditionally: the WebSocket transport is
            // inherently streaming; a client-supplied {"stream": false} would
            // break the pipeline.
            response["stream"] = serde_json::Value::Bool(true);
            strip_synthetic_warmup_previous_id(&mut response);
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
    fn parses_response_create_flat_shape() {
        // Codex's actual wire format: fields flat at top level next to `type`.
        let text =
            r#"{"type":"response.create","model":"gpt-5","input":"hi","instructions":"be brief"}"#;
        let ev = parse_client_frame(text).unwrap();
        match ev {
            ClientEvent::ResponseCreate { response, warmup } => {
                assert!(!warmup, "no `generate` field means not a warmup");
                assert_eq!(
                    response.get("model").and_then(|v| v.as_str()),
                    Some("gpt-5")
                );
                assert_eq!(
                    response.get("instructions").and_then(|v| v.as_str()),
                    Some("be brief")
                );
                // `type` is stripped before forwarding upstream.
                assert!(response.get("type").is_none());
            }
        }
    }

    #[test]
    fn generate_false_marks_warmup() {
        let text = r#"{"type":"response.create","model":"gpt-5","input":"hi","generate":false}"#;
        let ev = parse_client_frame(text).unwrap();
        match ev {
            ClientEvent::ResponseCreate { response, warmup } => {
                assert!(warmup, "generate: false is Codex's warmup ping");
                // `generate` must be stripped before forwarding upstream.
                assert!(response.get("generate").is_none());
            }
        }
    }

    #[test]
    fn generate_true_is_not_warmup() {
        let text = r#"{"type":"response.create","model":"gpt-5","input":"hi","generate":true}"#;
        let ev = parse_client_frame(text).unwrap();
        match ev {
            ClientEvent::ResponseCreate { response, warmup } => {
                assert!(!warmup);
                assert!(response.get("generate").is_none());
            }
        }
    }

    #[test]
    fn client_metadata_is_stripped() {
        // Codex sends `client_metadata` with traceparent/tracestate; the
        // Responses API doesn't accept it, so strip it before forwarding.
        let text = r#"{"type":"response.create","model":"gpt-5","input":"hi","client_metadata":{"traceparent":"00-abc"}}"#;
        let ev = parse_client_frame(text).unwrap();
        match ev {
            ClientEvent::ResponseCreate { response, .. } => {
                assert!(response.get("client_metadata").is_none());
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
    fn rejects_missing_type_field() {
        let text = r#"{"model":"gpt-5"}"#;
        let err = parse_client_frame(text).unwrap_err();
        assert!(matches!(err, FrameError::MissingField("type")));
    }

    #[test]
    fn rejects_non_object_frame() {
        let text = r#"[1,2,3]"#;
        let err = parse_client_frame(text).unwrap_err();
        assert!(matches!(err, FrameError::MissingField(_)));
    }

    #[test]
    fn rejects_malformed_json() {
        let err = parse_client_frame("{not json").unwrap_err();
        assert!(matches!(err, FrameError::MalformedJson(_)));
    }

    #[test]
    fn warmup_completed_frame_has_response_id() {
        // Codex's `prewarm_websocket` loop only accepts a frame as "completed"
        // if it's `type == "response.completed"` AND has a parseable
        // `response.id`. Pin that shape.
        let frame = warmup_completed_frame();
        let value: serde_json::Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(
            value.get("type").and_then(|v| v.as_str()),
            Some("response.completed")
        );
        let id = value
            .get("response")
            .and_then(|r| r.get("id"))
            .and_then(|v| v.as_str())
            .expect("response.id must be present");
        assert!(id.starts_with("warmup-"), "got {id}");
    }

    #[test]
    fn strips_warmup_previous_response_id() {
        // After the synthetic warmup completes, Codex echoes the warmup id as
        // `previous_response_id` on the first real turn. Upstream 400s on
        // unknown ids, so strip them before forwarding.
        let mut body = serde_json::json!({
            "model": "gpt-5",
            "previous_response_id": "warmup-abc-123",
            "input": "hi",
        });
        strip_synthetic_warmup_previous_id(&mut body);
        assert!(body.get("previous_response_id").is_none());
        assert_eq!(body.get("model").and_then(|v| v.as_str()), Some("gpt-5"));
    }

    #[test]
    fn preserves_real_previous_response_id() {
        // Real upstream-issued ids (e.g. `resp_...`) must NOT be stripped —
        // they're needed for multi-turn session continuity.
        let mut body = serde_json::json!({
            "model": "gpt-5",
            "previous_response_id": "resp_abc123",
            "input": "hi",
        });
        strip_synthetic_warmup_previous_id(&mut body);
        assert_eq!(
            body.get("previous_response_id").and_then(|v| v.as_str()),
            Some("resp_abc123")
        );
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
