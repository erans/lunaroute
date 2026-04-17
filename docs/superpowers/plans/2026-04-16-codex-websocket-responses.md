# Codex CLI WebSocket Responses API — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Accept OpenAI Responses API traffic over WebSocket on `/v1/responses` and `/responses`, so Codex CLI with `supports_websockets = true` works against lunaroute with full feature parity (session recording, LUNAROUTE markers, metrics, provider registry).

**Architecture:** WebSocket terminator. Accept the upgrade, parse each `response.create` frame, drive the existing HTTP Responses pipeline upstream (with `stream=true`), translate SSE events back to WS text frames. No new egress code; reuses `OpenAIConnector` and the existing streaming pipeline via a small refactor that extracts the SSE-generating core into a reusable function.

**Tech Stack:** Rust, axum 0.8 (`ws` feature already enabled), `tokio-tungstenite` (already transitive via axum; added as dev-dep for integration tests), `eventsource-stream`, existing lunaroute infrastructure.

**Spec:** [docs/superpowers/specs/2026-04-16-codex-websocket-responses-design.md](../specs/2026-04-16-codex-websocket-responses-design.md)

---

## File Map

- **Create:**
  - `crates/lunaroute-ingress/src/responses_ws.rs` — new module with frame parser, WS handler, read loop.
  - `crates/lunaroute-integration-tests/tests/responses_websocket.rs` — integration test driving a real TCP server with `tokio-tungstenite`.
- **Modify:**
  - `crates/lunaroute-ingress/src/openai.rs` — extract `responses_sse_stream` helper from `responses_passthrough`; add `SseEvent` type; expose whatever `responses_ws.rs` needs from `OpenAIPassthroughState`.
  - `crates/lunaroute-ingress/src/lib.rs` — `pub mod responses_ws;`.
  - `crates/lunaroute-observability/src/metrics.rs` — add WS counters/histograms.
  - `Cargo.toml` (workspace) — add `tokio-tungstenite = "0.28"` to `[workspace.dependencies]`.
  - `crates/lunaroute-integration-tests/Cargo.toml` — pull `tokio-tungstenite` into `[dev-dependencies]`.

All other existing behavior is unchanged. HTTP `/responses` passthrough stays byte-identical.

---

## Task 1: Baseline + add `tokio-tungstenite` to workspace

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/lunaroute-integration-tests/Cargo.toml`

- [ ] **Step 1: Run full test suite to confirm clean baseline**

Run: `cargo test --workspace --no-fail-fast 2>&1 | tail -40`
Expected: all tests pass. If anything is red on `main`, stop and fix before proceeding.

- [ ] **Step 2: Add `tokio-tungstenite` to workspace deps**

Edit `Cargo.toml` (root), add in the `[workspace.dependencies]` block near the other HTTP libs (after `eventsource-stream = "0.2"`):

```toml
tokio-tungstenite = "0.28"
```

- [ ] **Step 3: Pull into integration tests as a dev-dep**

Edit `crates/lunaroute-integration-tests/Cargo.toml`, add to `[dev-dependencies]`:

```toml
tokio-tungstenite = { workspace = true }
```

- [ ] **Step 4: Verify workspace still builds**

Run: `cargo check --workspace`
Expected: clean build, no new warnings.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/lunaroute-integration-tests/Cargo.toml
git commit -m "chore: add tokio-tungstenite for WS integration tests"
```

---

## Task 2: Extract `SseEvent` + `responses_sse_stream` core (pure refactor)

**Goal:** Lift the streaming pipeline out of `responses_passthrough` into a function the WS handler can call. Zero behavior change to the HTTP path.

**Files:**
- Modify: `crates/lunaroute-ingress/src/openai.rs:~890-1700`

- [ ] **Step 1: Add `SseEvent` struct near top of `openai.rs`**

Insert right after the existing `MAX_COLLECTED_EVENTS` constant (around line 34):

```rust
/// A single SSE event yielded by the shared responses pipeline.
///
/// Both the HTTP handler (which re-wraps in `axum::response::sse::Event`)
/// and the WebSocket handler (which sends the `data` payload as a text frame)
/// consume this.
#[derive(Debug, Clone)]
pub(crate) struct SseEvent {
    pub event: String,
    pub data: String,
}
```

- [ ] **Step 2: Locate the streaming branch of `responses_passthrough`**

Open `crates/lunaroute-ingress/src/openai.rs` and find `async fn responses_passthrough` (around line 904). Within it, find the `if is_streaming { ... }` block (starts around line 1213). Note the end of the block — it's where the function returns `Ok(Sse::new(mapped_stream).keep_alive(sse_keepalive).into_response())`.

This whole block becomes the body of the new helper, with one change: instead of returning an axum SSE response, return a `BoxStream<Result<SseEvent, IngressError>>`.

- [ ] **Step 3: Introduce the new helper `responses_sse_stream`**

Add this new function directly above `async fn responses_passthrough`:

```rust
use futures::stream::BoxStream;

/// Drive the upstream `/responses` streaming pipeline and return a stream of
/// SSE events. Shared by HTTP `responses_passthrough` and the WebSocket
/// handler in `crate::responses_ws`.
///
/// Performs: header filtering, LUNAROUTE marker detection, session recording
/// (Started / RequestRecorded / StatsUpdated / ToolCallRecorded / Completed),
/// upstream call via `OpenAIConnector::stream_passthrough_to_endpoint_bytes`,
/// SSE parsing, and metric extraction. Identical side effects to the original
/// streaming branch — just no axum wrapping.
pub(crate) async fn responses_sse_stream(
    state: Arc<OpenAIPassthroughState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<BoxStream<'static, Result<SseEvent, IngressError>>, IngressError> {
    // Body copied verbatim from the streaming branch of `responses_passthrough`
    // below, with the following minimal changes:
    //   1. The final `Ok(Sse::new(mapped_stream).keep_alive(...).into_response())`
    //      is replaced with `Ok(mapped_stream.boxed())`.
    //   2. The `filter_map` closure emits `SseEvent { event, data }` instead of
    //      `axum::response::sse::Event::default().data(event.data).event(event.event)`.
    //   3. The stream error branch emits
    //      `SseEvent { event: "error".into(), data: format!("error: {e}") }`
    //      instead of an axum `Event::default().data(...)`.
    //
    // All header filtering, session ID extraction, session event emission,
    // marker handling, tool-call mapping, and stats accumulation stays exactly
    // as in the original streaming branch.
    //
    // NOTE: The non-streaming branch of `responses_passthrough` is NOT moved
    // here — WS always streams. The HTTP handler keeps its non-streaming path.

    todo!("Implement by moving streaming-branch code from responses_passthrough")
}
```

This stub will be filled in in the next step.

- [ ] **Step 4: Move the streaming-branch code into the helper**

In the same file, cut the entire body of the `if is_streaming { ... }` block from `responses_passthrough` (from the line `if is_streaming {` through the return of `Ok(Sse::new(mapped_stream)...into_response())`). Paste it into `responses_sse_stream`, replacing the `todo!(...)`. Apply the three minimal changes from the comment:

At the `filter_map` emission site, change:
```rust
match result {
    Ok(event) => Some(
        Ok::<_, eventsource_stream::EventStreamError<std::io::Error>>(
            Event::default().data(event.data).event(event.event),
        ),
    ),
    Err(e) => Some(
        Ok::<_, eventsource_stream::EventStreamError<std::io::Error>>(
            Event::default().data(format!("error: {}", e)),
        ),
    ),
}
```
to:
```rust
match result {
    Ok(event) => Some(Ok::<SseEvent, IngressError>(SseEvent {
        event: event.event,
        data: event.data,
    })),
    Err(e) => Some(Ok::<SseEvent, IngressError>(SseEvent {
        event: "error".into(),
        data: format!("error: {e}"),
    })),
}
```

At the function tail, change:
```rust
Ok(Sse::new(mapped_stream)
    .keep_alive(sse_keepalive)
    .into_response())
```
to:
```rust
use futures::StreamExt as _;
Ok(mapped_stream.boxed())
```

Remove the now-unused `sse_keepalive_interval` / `sse_keepalive_enabled_flag` clones from the helper — they were only used to build the axum `KeepAlive`.

- [ ] **Step 5: Replace streaming branch in `responses_passthrough`**

Back in `responses_passthrough`, where the `if is_streaming { ... }` block used to be, write:

```rust
if is_streaming {
    let sse_keepalive_interval = state.sse_keepalive_interval_secs;
    let sse_keepalive_enabled_flag = state.sse_keepalive_enabled;

    let event_stream = responses_sse_stream(state.clone(), headers.clone(), body.clone()).await?;

    use axum::response::sse::{Event, KeepAlive, Sse};
    use futures::StreamExt as _;
    let axum_stream = event_stream.map(|result| match result {
        Ok(ev) => Ok::<_, std::convert::Infallible>(
            Event::default().data(ev.data).event(ev.event),
        ),
        Err(e) => Ok::<_, std::convert::Infallible>(
            Event::default().data(format!("error: {e}")),
        ),
    });

    let keepalive = if sse_keepalive_enabled_flag {
        KeepAlive::new().interval(std::time::Duration::from_secs(sse_keepalive_interval))
    } else {
        KeepAlive::new().interval(std::time::Duration::from_secs(86400))
    };

    return Ok(Sse::new(axum_stream).keep_alive(keepalive).into_response());
}
```

The non-streaming branch below is untouched.

- [ ] **Step 6: Fix imports; resolve compile**

Run: `cargo check -p lunaroute-ingress`
Expected: compiles clean. If there are `use` errors, the helper needs the same imports the streaming branch used (`futures::StreamExt`, `eventsource_stream::EventStream`, `lunaroute_session::*`, etc.) — copy the relevant `use` statements over. Do NOT shadow or reorder anything in `responses_passthrough`.

- [ ] **Step 7: Run the passthrough streaming recording integration tests**

Run:
```bash
cargo test -p lunaroute_integration_tests --test passthrough_streaming_recording -- --nocapture 2>&1 | tail -30
```
Expected: all 4 tests pass. This is the tightest check that the refactor preserved behavior (session events + SSE frames).

- [ ] **Step 8: Run the full workspace test suite**

Run: `cargo test --workspace --no-fail-fast 2>&1 | tail -30`
Expected: all tests pass. Nothing regressed.

- [ ] **Step 9: Commit**

```bash
git add crates/lunaroute-ingress/src/openai.rs
git commit -m "refactor: extract responses_sse_stream from responses_passthrough

Pure code motion: streaming pipeline for /v1/responses now lives in a
reusable helper returning Stream<SseEvent>. HTTP handler wraps it in
axum Sse as before. No behavior change; all existing integration tests
pass unchanged. Prepares for WebSocket ingress on /responses."
```

---

## Task 3: Frame parser + `responses_ws.rs` skeleton

**Goal:** Land the new module with a frame parser and unit tests. No handler yet, no routing yet.

**Files:**
- Create: `crates/lunaroute-ingress/src/responses_ws.rs`
- Modify: `crates/lunaroute-ingress/src/lib.rs`

- [ ] **Step 1: Create the module file**

Create `crates/lunaroute-ingress/src/responses_ws.rs` with:

```rust
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_response_create() {
        let text = r#"{"type":"response.create","response":{"model":"gpt-5","input":"hi"}}"#;
        let ev = parse_client_frame(text).unwrap();
        match ev {
            ClientEvent::ResponseCreate { response } => {
                assert_eq!(response.get("model").and_then(|v| v.as_str()), Some("gpt-5"));
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
```

- [ ] **Step 2: Wire module into `lib.rs`**

Edit `crates/lunaroute-ingress/src/lib.rs`, add alongside the other `pub mod` declarations:

```rust
pub mod responses_ws;
```

- [ ] **Step 3: Run the new tests**

Run: `cargo test -p lunaroute-ingress responses_ws:: -- --nocapture`
Expected: 6 tests pass (`parses_response_create`, `rejects_unknown_type`, `rejects_missing_response_field`, `rejects_non_object_response`, `rejects_malformed_json`, `error_frame_has_expected_shape`).

- [ ] **Step 4: Run the full workspace test suite**

Run: `cargo test --workspace --no-fail-fast 2>&1 | tail -20`
Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/lunaroute-ingress/src/responses_ws.rs crates/lunaroute-ingress/src/lib.rs
git commit -m "feat: frame parser for Responses API WebSocket ingress"
```

---

## Task 4: WebSocket handler + read loop

**Goal:** Accept the upgrade, run a read loop that parses `response.create`, drives `responses_sse_stream`, and forwards events as WS text frames. Not yet wired into the router — that's Task 5.

**Files:**
- Modify: `crates/lunaroute-ingress/src/responses_ws.rs`
- Modify: `crates/lunaroute-ingress/src/openai.rs` (may need to expose `OpenAIPassthroughState` or its fields — check in Step 1)

- [ ] **Step 1: Make `OpenAIPassthroughState` and `responses_sse_stream` reachable from sibling module**

Open `crates/lunaroute-ingress/src/openai.rs`. Find `struct OpenAIPassthroughState` (near line 1920ish). If it is not `pub(crate)` or public, change it to `pub(crate) struct OpenAIPassthroughState`. Do the same with `fn responses_sse_stream` if it isn't already (it was defined `pub(crate)` in Task 2).

Run: `cargo check -p lunaroute-ingress`
Expected: clean.

- [ ] **Step 2: Append WS handler code to `responses_ws.rs`**

Append to `crates/lunaroute-ingress/src/responses_ws.rs` (before the `#[cfg(test)]` module):

```rust
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::Response;
use futures::StreamExt as _;
use std::sync::Arc;

use crate::openai::{OpenAIPassthroughState, SseEvent, responses_sse_stream};

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
    tracing::debug!("WS session started");

    while let Some(msg) = socket.recv().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("WS recv error: {e}");
                break;
            }
        };

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
                let _ = send_error(
                    &mut socket,
                    "unsupported_frame_type",
                    "binary frames are not supported",
                )
                .await;
            }
        }
    }

    tracing::debug!("WS session ended");
}

/// Dispatch one client text frame: parse, run the pipeline, forward events.
async fn handle_client_text(
    socket: &mut WebSocket,
    state: &Arc<OpenAIPassthroughState>,
    upgrade_headers: &HeaderMap,
    text: &str,
) -> Result<(), axum::Error> {
    let event = match parse_client_frame(text) {
        Ok(e) => e,
        Err(FrameError::MalformedJson(e)) => {
            return send_error(socket, "malformed_json", &e.to_string()).await;
        }
        Err(FrameError::MissingField(f)) => {
            return send_error(socket, "invalid_request", &format!("missing field: {f}"))
                .await;
        }
        Err(FrameError::UnsupportedType(t)) => {
            return send_error(
                socket,
                "unsupported_event_type",
                &format!("unsupported event type: {t}"),
            )
            .await;
        }
    };

    match event {
        ClientEvent::ResponseCreate { mut response } => {
            // Force stream=true: upstream HTTP needs it for streaming.
            if response.get("stream").is_none() {
                response["stream"] = serde_json::Value::Bool(true);
            }
            let body_bytes = match serde_json::to_vec(&response) {
                Ok(b) => axum::body::Bytes::from(b),
                Err(e) => {
                    return send_error(socket, "internal_error", &e.to_string()).await;
                }
            };

            let stream = match responses_sse_stream(
                state.clone(),
                upgrade_headers.clone(),
                body_bytes,
            )
            .await
            {
                Ok(s) => s,
                Err(e) => {
                    return send_error(socket, "upstream_error", &e.to_string()).await;
                }
            };

            forward_stream(socket, stream).await
        }
    }
}

/// Forward a stream of `SseEvent`s as WebSocket text frames until a terminal
/// event is seen or the stream ends.
async fn forward_stream(
    socket: &mut WebSocket,
    mut stream: futures::stream::BoxStream<'static, Result<SseEvent, crate::IngressError>>,
) -> Result<(), axum::Error> {
    while let Some(result) = stream.next().await {
        match result {
            Ok(ev) => {
                // Send just the `data` payload — the event name is already
                // embedded in the JSON's `type` field per the Responses WS spec.
                socket.send(Message::Text(ev.data.clone().into())).await?;
                if is_terminal(&ev) {
                    return Ok(());
                }
            }
            Err(e) => {
                return send_error(socket, "stream_error", &e.to_string()).await;
            }
        }
    }
    // Stream ended with no terminal event — emit a synthetic error so the
    // client doesn't hang waiting.
    send_error(
        socket,
        "stream_ended",
        "upstream stream ended without a terminal event",
    )
    .await
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
    code: &str,
    message: &str,
) -> Result<(), axum::Error> {
    socket
        .send(Message::Text(error_frame(code, message).into()))
        .await
}
```

- [ ] **Step 3: Re-export the handler from the crate for external use**

Open `crates/lunaroute-ingress/src/lib.rs`. Below the existing `pub use` lines, add:

```rust
pub use responses_ws::responses_ws_handler;
```

- [ ] **Step 4: Compile**

Run: `cargo check -p lunaroute-ingress 2>&1 | tail -20`
Expected: clean. If `OpenAIPassthroughState` is still private and fails — double back to Step 1 and make it `pub(crate)`. If `crate::IngressError` import path fails, use `crate::types::IngressError` instead.

- [ ] **Step 5: Run existing tests to confirm nothing broke**

Run: `cargo test --workspace --no-fail-fast 2>&1 | tail -20`
Expected: all pass (handler is not yet wired to any route, so this is pure "compiles and doesn't regress" check).

- [ ] **Step 6: Commit**

```bash
git add crates/lunaroute-ingress/src/responses_ws.rs crates/lunaroute-ingress/src/lib.rs crates/lunaroute-ingress/src/openai.rs
git commit -m "feat: WebSocket handler + read loop for Responses API ingress"
```

---

## Task 5: Wire WS handler into `passthrough_router`

**Goal:** Codex CLI can now hit `ws://host/responses` or `ws://host/v1/responses` and the handler runs.

**Files:**
- Modify: `crates/lunaroute-ingress/src/openai.rs:~1953-1958` (in `passthrough_router`)

- [ ] **Step 1: Update route definitions to accept GET (upgrade) alongside POST**

In `passthrough_router` (around line 1953), change:

```rust
.route("/v1/chat/completions", post(chat_completions_passthrough))
.route("/v1/responses", post(responses_passthrough))
.route("/responses", post(responses_passthrough)) // For Codex compatibility (base_url without /v1)
.route("/v1/models", axum::routing::get(models_passthrough))
.route("/models", axum::routing::get(models_passthrough)) // For Codex compatibility (base_url without /v1)
```

to:

```rust
use axum::routing::get;
.route("/v1/chat/completions", post(chat_completions_passthrough))
.route(
    "/v1/responses",
    post(responses_passthrough).get(crate::responses_ws::responses_ws_handler),
)
.route(
    "/responses",
    post(responses_passthrough).get(crate::responses_ws::responses_ws_handler),
)
.route("/v1/models", get(models_passthrough))
.route("/models", get(models_passthrough))
```

(The `use axum::routing::get;` goes near the other `use axum::routing::post;` at the top of the function body.)

- [ ] **Step 2: Compile**

Run: `cargo check -p lunaroute-ingress 2>&1 | tail -10`
Expected: clean.

- [ ] **Step 3: Run all tests**

Run: `cargo test --workspace --no-fail-fast 2>&1 | tail -20`
Expected: all pass. HTTP `/responses` behavior is preserved because POST still dispatches to `responses_passthrough`; the WS handler is only reached on a GET-with-upgrade.

- [ ] **Step 4: Commit**

```bash
git add crates/lunaroute-ingress/src/openai.rs
git commit -m "feat: wire WS handler into /responses routes"
```

---

## Task 6: Integration test — full WS roundtrip with session recording

**Goal:** Prove the whole chain works end-to-end against a mocked upstream.

**Files:**
- Create: `crates/lunaroute-integration-tests/tests/responses_websocket.rs`

- [ ] **Step 1: Write the integration test file**

Create `crates/lunaroute-integration-tests/tests/responses_websocket.rs`:

```rust
//! Integration tests for OpenAI Responses API WebSocket ingress.
//!
//! Verifies:
//! 1. A `response.create` frame drives the HTTP pipeline and streams events back.
//! 2. Session events (Started, RequestRecorded, Completed) are recorded.
//! 3. Multiple `response.create` frames on one connection run sequentially.
//! 4. Upstream errors are translated into structured error frames.

mod common;

use common::InMemorySessionStore;
use futures::{SinkExt, StreamExt};
use lunaroute_egress::openai::{OpenAIConfig, OpenAIConnector};
use lunaroute_session::SessionEvent;
use serde_json::json;
use std::sync::Arc;
use tokio_tungstenite::tungstenite::Message;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Bind the passthrough router to a random localhost port and return the port +
/// server task handle. The task runs `axum::serve` on a `tokio::net::TcpListener`.
async fn spawn_passthrough(
    connector: Arc<OpenAIConnector>,
    store: Arc<InMemorySessionStore>,
) -> u16 {
    let app = lunaroute_ingress::openai::passthrough_router(
        connector, None, None, Some(store), 15, true, None,
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    // Give the server a beat to start accepting.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    port
}

fn upstream_sse_body() -> String {
    // Minimal Responses API stream: created → output_text.delta → completed.
    // Each event is `data: <json>\n\n` per SSE.
    [
        r#"{"type":"response.created","response":{"id":"resp_1","model":"gpt-5"}}"#,
        r#"{"type":"response.output_text.delta","delta":"hi"}"#,
        r#"{"type":"response.completed","response":{"id":"resp_1","usage":{"input_tokens":5,"output_tokens":1,"total_tokens":6}}}"#,
    ]
    .iter()
    .map(|e| format!("data: {e}\n\n"))
    .collect::<String>()
}

#[tokio::test]
async fn ws_response_create_streams_events_and_records_session() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_string(upstream_sse_body()))
        .expect(1)
        .mount(&mock_server)
        .await;

    let store = Arc::new(InMemorySessionStore::new());

    let config = OpenAIConfig {
        api_key: "test-api-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let connector = Arc::new(OpenAIConnector::new(config).await.unwrap());

    let port = spawn_passthrough(connector, store.clone()).await;

    // Connect WebSocket to the /v1/responses endpoint with an Authorization header.
    let url = format!("ws://127.0.0.1:{port}/v1/responses");
    let request = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
        url.as_str(),
    )
    .unwrap();
    // Inject Authorization header — mimic what Codex CLI sends at upgrade.
    let mut request = request;
    request
        .headers_mut()
        .insert("authorization", "Bearer test-api-key".parse().unwrap());

    let (mut ws, _resp) = tokio_tungstenite::connect_async(request).await.unwrap();

    // Send a response.create.
    let create = json!({
        "type": "response.create",
        "response": {
            "model": "gpt-5",
            "input": "hello"
        }
    });
    ws.send(Message::Text(create.to_string().into()))
        .await
        .unwrap();

    // Collect frames until the `response.completed` arrives.
    let mut seen_types: Vec<String> = Vec::new();
    while let Some(frame) = ws.next().await {
        let msg = frame.unwrap();
        let text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Close(_) => break,
            _ => continue,
        };
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let ty = value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        seen_types.push(ty.clone());
        if ty == "response.completed" {
            break;
        }
    }

    assert_eq!(
        seen_types,
        vec![
            "response.created",
            "response.output_text.delta",
            "response.completed"
        ],
        "unexpected event order; got {seen_types:?}"
    );

    // Give async session writers a moment to flush.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let events = store.get_events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SessionEvent::Started { .. })),
        "expected Started event; got {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SessionEvent::RequestRecorded { .. })),
        "expected RequestRecorded event"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SessionEvent::Completed { .. })),
        "expected Completed event"
    );

    ws.close(None).await.ok();
}

#[tokio::test]
async fn ws_runs_two_response_creates_sequentially_on_one_connection() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_string(upstream_sse_body()))
        .expect(2)
        .mount(&mock_server)
        .await;

    let store = Arc::new(InMemorySessionStore::new());

    let config = OpenAIConfig {
        api_key: "test-api-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let connector = Arc::new(OpenAIConnector::new(config).await.unwrap());

    let port = spawn_passthrough(connector, store.clone()).await;
    let url = format!("ws://127.0.0.1:{port}/v1/responses");
    let mut request = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
        url.as_str(),
    )
    .unwrap();
    request
        .headers_mut()
        .insert("authorization", "Bearer test-api-key".parse().unwrap());
    let (mut ws, _resp) = tokio_tungstenite::connect_async(request).await.unwrap();

    for _ in 0..2 {
        let create = json!({
            "type": "response.create",
            "response": { "model": "gpt-5", "input": "hi" }
        });
        ws.send(Message::Text(create.to_string().into())).await.unwrap();

        loop {
            let msg = ws.next().await.unwrap().unwrap();
            let Message::Text(t) = msg else { continue };
            let value: serde_json::Value = serde_json::from_str(&t).unwrap();
            if value.get("type").and_then(|v| v.as_str()) == Some("response.completed") {
                break;
            }
        }
    }

    ws.close(None).await.ok();

    // Upstream should have seen exactly 2 POSTs; wiremock's `.expect(2)` above
    // asserts this implicitly on drop.
}

#[tokio::test]
async fn ws_sends_error_frame_for_unsupported_event_type() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_string(upstream_sse_body()))
        .mount(&mock_server)
        .await;

    let config = OpenAIConfig {
        api_key: "test-api-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let connector = Arc::new(OpenAIConnector::new(config).await.unwrap());
    let store = Arc::new(InMemorySessionStore::new());
    let port = spawn_passthrough(connector, store).await;

    let url = format!("ws://127.0.0.1:{port}/v1/responses");
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();

    ws.send(Message::Text(r#"{"type":"response.cancel"}"#.into()))
        .await
        .unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    let Message::Text(text) = msg else {
        panic!("expected text frame, got {msg:?}")
    };
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value.get("type").and_then(|v| v.as_str()), Some("error"));
    assert_eq!(
        value
            .get("error")
            .and_then(|e| e.get("code"))
            .and_then(|v| v.as_str()),
        Some("unsupported_event_type")
    );

    ws.close(None).await.ok();
}
```

- [ ] **Step 2: Run the new integration tests**

Run:
```bash
cargo test -p lunaroute_integration_tests --test responses_websocket -- --nocapture 2>&1 | tail -40
```
Expected: all 3 tests pass.

- [ ] **Step 3: Run the full workspace test suite**

Run: `cargo test --workspace --no-fail-fast 2>&1 | tail -20`
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add crates/lunaroute-integration-tests/tests/responses_websocket.rs
git commit -m "test: end-to-end WS Responses API integration tests"
```

---

## Task 7: WS metrics

**Goal:** Emit connection + frame counters so operators can see WS traffic in Prometheus.

**Files:**
- Modify: `crates/lunaroute-observability/src/metrics.rs`
- Modify: `crates/lunaroute-ingress/src/responses_ws.rs`

- [ ] **Step 1: Add new metric fields and registration in `Metrics`**

In `crates/lunaroute-observability/src/metrics.rs`, find the `Metrics` struct definition. Add four new fields (alongside the existing counters/histograms):

```rust
pub ws_connections_opened: CounterVec,
pub ws_connections_closed: CounterVec,
pub ws_connection_duration_seconds: HistogramVec,
pub ws_frames_total: CounterVec,
```

In `impl Metrics { pub fn new(...) -> Result<Self, prometheus::Error> { ... }`, register them. Follow the pattern of the existing `fallbacks_total` and `request_duration_seconds` registration. Use these names and labels:

```rust
let ws_connections_opened = CounterVec::new(
    Opts::new(
        "lunaroute_ws_connections_opened_total",
        "Total WebSocket connections accepted, by endpoint",
    ),
    &["endpoint"],
)?;
let ws_connections_closed = CounterVec::new(
    Opts::new(
        "lunaroute_ws_connections_closed_total",
        "Total WebSocket connections closed, by endpoint",
    ),
    &["endpoint"],
)?;
let ws_connection_duration_seconds = HistogramVec::new(
    HistogramOpts::new(
        "lunaroute_ws_connection_duration_seconds",
        "Duration of WebSocket connections in seconds, by endpoint",
    )
    .buckets(vec![1.0, 10.0, 60.0, 300.0, 900.0, 1800.0, 3600.0]),
    &["endpoint"],
)?;
let ws_frames_total = CounterVec::new(
    Opts::new(
        "lunaroute_ws_frames_total",
        "Total WebSocket frames, by endpoint, direction (client|server), and type",
    ),
    &["endpoint", "direction", "type"],
)?;

registry.register(Box::new(ws_connections_opened.clone()))?;
registry.register(Box::new(ws_connections_closed.clone()))?;
registry.register(Box::new(ws_connection_duration_seconds.clone()))?;
registry.register(Box::new(ws_frames_total.clone()))?;
```

And include them in the final `Metrics { ... }` construction. Run:

```bash
cargo check -p lunaroute-observability
```
Expected: clean.

- [ ] **Step 2: Add helper methods on `Metrics`**

In the same file, inside `impl Metrics`:

```rust
pub fn record_ws_connection_opened(&self, endpoint: &str) {
    self.ws_connections_opened
        .with_label_values(&[endpoint])
        .inc();
}

pub fn record_ws_connection_closed(&self, endpoint: &str, duration_secs: f64) {
    self.ws_connections_closed
        .with_label_values(&[endpoint])
        .inc();
    self.ws_connection_duration_seconds
        .with_label_values(&[endpoint])
        .observe(duration_secs);
}

pub fn record_ws_frame(&self, endpoint: &str, direction: &str, frame_type: &str) {
    self.ws_frames_total
        .with_label_values(&[endpoint, direction, frame_type])
        .inc();
}
```

Run: `cargo check -p lunaroute-observability`
Expected: clean.

- [ ] **Step 3: Thread `state` into `forward_stream` and instrument both functions**

In `crates/lunaroute-ingress/src/responses_ws.rs`, replace `run_ws_session`, `handle_client_text`, and `forward_stream` with the instrumented versions below. All three change together so the `state` reference flows through cleanly.

Replace `run_ws_session`:

```rust
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

        match msg {
            Message::Text(text) => {
                if let Some(metrics) = &state.metrics {
                    metrics.record_ws_frame(ENDPOINT, "client", "text");
                }
                if let Err(e) =
                    handle_client_text(&mut socket, &state, &upgrade_headers, text.as_ref()).await
                {
                    tracing::warn!("WS text handling error: {e}");
                }
            }
            Message::Close(_) => {
                tracing::debug!("WS client closed");
                break;
            }
            Message::Ping(_) | Message::Pong(_) => {}
            Message::Binary(_) => {
                let _ = send_error(
                    &mut socket,
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
```

Update `handle_client_text` — change the final `forward_stream(socket, stream).await` call site to pass `state`:

```rust
forward_stream(socket, state, stream).await
```

Replace `forward_stream` with:

```rust
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
                    let ty = if ev.event.is_empty() { "message" } else { ev.event.as_str() };
                    metrics.record_ws_frame(ENDPOINT, "server", ty);
                }
                socket.send(Message::Text(ev.data.clone().into())).await?;
                if is_terminal(&ev) {
                    return Ok(());
                }
            }
            Err(e) => {
                return send_error(socket, "stream_error", &e.to_string()).await;
            }
        }
    }
    send_error(
        socket,
        "stream_ended",
        "upstream stream ended without a terminal event",
    )
    .await
}
```

- [ ] **Step 4: Compile**

Run: `cargo check -p lunaroute-ingress`
Expected: clean.

- [ ] **Step 5: Run all tests**

Run: `cargo test --workspace --no-fail-fast 2>&1 | tail -20`
Expected: all pass — the integration tests run without a `Metrics` instance (the `None` passed to `passthrough_router`), so metric code paths are skipped and don't affect behavior.

- [ ] **Step 6: Commit**

```bash
git add crates/lunaroute-observability/src/metrics.rs crates/lunaroute-ingress/src/responses_ws.rs
git commit -m "feat: prometheus metrics for WS Responses ingress"
```

---

## Task 8: Documentation + smoke test

**Goal:** Leave a trail so the next person can verify Codex CLI actually works.

**Files:**
- Modify: `README.md` (add short note under the Codex CLI section)
- Create: `docs/plans/2026-04-16-codex-ws-smoke.md` — manual smoke-test runbook

- [ ] **Step 1: README tweak**

In `README.md`, find the "OpenAI Codex CLI" bullet (around line 529). Replace it with:

```markdown
- ✅ **OpenAI Codex CLI** - Automatic auth.json integration. Supports both HTTP and WebSocket transports — set `supports_websockets = true` in `~/.codex/config.toml` to use the WS path (lunaroute terminates the WS and drives the HTTP pipeline; session recording, markers, and metrics all work the same).
```

- [ ] **Step 2: Smoke-test runbook**

Create `docs/plans/2026-04-16-codex-ws-smoke.md`:

```markdown
# Codex CLI WebSocket Smoke Test

Verify the end-to-end Codex → lunaroute → OpenAI path using the WS transport.

1. Start lunaroute: `eval $(lunaroute-server env)`.
2. Edit `~/.codex/config.toml`:

   ```toml
   [model_providers.openai]
   name = "OpenAI"
   base_url = "http://127.0.0.1:8081/v1"
   env_key = "OPENAI_API_KEY"
   wire_api = "responses"
   supports_websockets = true
   ```

3. Run a trivial Codex command, e.g. `codex "print hello world in rust"`.
4. Open the lunaroute UI at `http://127.0.0.1:8082` — verify:
   - The session shows up.
   - Tokens are non-zero.
   - Response text matches what Codex displayed.
5. Optionally, watch logs: `lunaroute-server logs -f` — should see `WS session started` then `WS session ended` for the request.
```

- [ ] **Step 3: Run full test suite one last time**

Run: `cargo test --workspace --no-fail-fast 2>&1 | tail -10`
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add README.md docs/plans/2026-04-16-codex-ws-smoke.md
git commit -m "docs: Codex WS smoke test runbook + README note"
```

---

## Final Verification

- [ ] **All tasks complete, all tests green**

Run: `cargo test --workspace --no-fail-fast 2>&1 | tail -10`
Expected: all pass.

- [ ] **Manual smoke test** (if you have Codex CLI installed)

Follow `docs/plans/2026-04-16-codex-ws-smoke.md`.

- [ ] **Review diff summary**

Run: `git log --oneline main..HEAD`
Expected: 8 commits, one per task.
