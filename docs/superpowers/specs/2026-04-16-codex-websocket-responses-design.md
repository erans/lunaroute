# Codex CLI WebSocket Responses API Support

**Date:** 2026-04-16
**Status:** Draft
**Related:** [LUNAROUTE Marker-Based Provider Routing](2026-04-03-lunaroute-marker-routing-design.md), [Cross-Dialect Marker Routing](2026-04-03-cross-dialect-marker-routing-design.md)

## Problem

Codex CLI now speaks OpenAI's Responses API over WebSocket in addition to HTTP+SSE. When its provider is configured with `supports_websockets = true` (the default for the built-in `openai` provider in recent versions), it opens a WebSocket to `{openai_base_url}/responses` instead of issuing HTTP POSTs. Lunaroute only exposes the HTTP handler on `/responses` and `/v1/responses`, so the upgrade handshake fails and Codex either errors out or stalls.

The goal is to let Codex CLI point at lunaroute with the WebSocket transport enabled and have everything (session recording, LUNAROUTE markers, metrics, provider registry, `codex_auth`) work exactly as it does for HTTP.

## Solution

Accept the incoming WebSocket as a thin terminator and drive the existing HTTP Responses pipeline for every `response.create` message. Translate the upstream SSE stream back into WebSocket text frames that mirror the Responses streaming event model. No new egress code, no new WebSocket client — `reqwest` and the existing `OpenAIConnector` keep doing the work.

## Scope

**In scope:**

- WebSocket upgrade on `GET /v1/responses` and `GET /responses` (mirroring the current HTTP route pair).
- Full feature parity with the HTTP `/responses` passthrough: LUNAROUTE markers (same-dialect only), session recording, metrics, provider registry, `codex_auth`, header filtering, session ID extraction.
- Sequential in-flight semantics per connection (one response in flight at a time — matches the upstream contract Codex expects).
- Structured error frames on upstream or marker errors; connection stays open so Codex can send the next `response.create`.

**Out of scope:**

- True WebSocket→WebSocket transparent proxying (no WS client in egress).
- Local `previous_response_id` cache for `store=false` fast continuation. Lunaroute relies on `store=true` upstream; the `store=false` case loses WS-local cache benefit and this is documented.
- Cross-dialect marker routing from WebSocket ingress (same restriction as HTTP `/responses` today). A marker pointing at an Anthropic provider returns the same "cross-dialect requires normalized mode" error as HTTP.
- 60-minute hard cap on client ↔ lunaroute connection. Upstream enforces its own limits on each HTTP call; there's no reason to impose one on the client hop.

## Wire Protocol (what Codex speaks)

Reference: OpenAI Responses API WebSocket Mode — `wss://api.openai.com/v1/responses`.

- **Upgrade:** standard HTTP GET with `Upgrade: websocket`. Client sends `Authorization: Bearer …` and other OpenAI headers on the upgrade request.
- **Client → server frames:** JSON text frames shaped like
  ```json
  { "type": "response.create", "response": { /* Responses API create body */ } }
  ```
  plus optional `previous_response_id` and `input` fields inside `response`.
- **Server → client frames:** JSON text frames matching the Responses SSE event model (`response.created`, `response.output_text.delta`, `response.completed`, `response.error`, `response.failed`, …), one event per frame.
- **Transport-only fields** (`stream`, `background`) are ignored; the WS is inherently streaming and foreground.
- **Sequential:** a single connection runs one response at a time. The client is expected to wait for a terminal event (`response.completed` / `response.failed` / error frame) before sending the next `response.create`. Lunaroute enforces this by awaiting completion on the read-loop.

## Data Flow

```
Codex CLI
  → WS upgrade on /v1/responses (or /responses)
    → axum WebSocket handler (new)
      loop:
        → recv text frame (response.create)
          → extract response body + merge with upgrade-time headers
          → set stream = true internally
          → call process_responses_request(state, headers, body):
            ← marker detection → provider override (same-dialect only)
            ← session recording (Started, RequestRecorded)
            ← upstream call via OpenAIConnector
            ← SSE parsing (eventsource_stream)
            → yields Stream<SseEvent { event, data }>
          → for each SseEvent: send Message::Text(data) as WS frame
          → on terminal event (see "Terminal events" below): record ResponseCompleted, return to recv
      on client close: cancel in-flight upstream stream, close WS cleanly
```

## Code Layout

1. **Extract shared core.** Refactor the body of `responses_passthrough()` in `crates/lunaroute-ingress/src/openai.rs` into a reusable function:
   ```rust
   pub(crate) async fn process_responses_request(
       state: Arc<OpenAIPassthroughState>,
       headers: HeaderMap,
       body: Bytes,
   ) -> Result<ResponseEventStream, IngressError>;
   ```
   where `ResponseEventStream` is `impl Stream<Item = Result<SseEvent, StreamError>>` and each `SseEvent` carries `{ event: String, data: String }` — exactly what `eventsource_stream` already produces inside the current HTTP handler. This matches the existing pipeline (which also parses SSE to typed events for metrics extraction before re-emitting), so there's no new parse/serialize overhead.

   - **HTTP handler** wraps each `SseEvent` in `axum::response::sse::Event::default().event(&e.event).data(&e.data)` — behavior is byte-equivalent to today.
   - **WS handler** sends `Message::Text(e.data)` — just the JSON data payload. The Responses WS wire format puts the event name inside the JSON's `type` field, so the SSE `event:` line is redundant and dropped.

   The shared core handles: header filtering, session ID extraction, marker detection, upstream call via the configured `OpenAIConnector`, session recording (Started / RequestRecorded / ResponseCompleted), streaming metrics, and tool-call mapping. No logic changes — just code motion + a cleaner seam.

2. **New WS handler.** Add `crates/lunaroute-ingress/src/responses_ws.rs` (~150–250 lines) with:
   ```rust
   pub async fn responses_ws_handler(
       ws: WebSocketUpgrade,
       State(state): State<Arc<OpenAIPassthroughState>>,
       headers: HeaderMap,
   ) -> Response;

   async fn run_ws_session(
       socket: WebSocket,
       state: Arc<OpenAIPassthroughState>,
       upgrade_headers: HeaderMap,
   );
   ```
   `run_ws_session` owns the read loop, enforces sequential in-flight, and serializes events back to frames.

3. **Wire up routes.** In `passthrough_router()`:
   ```rust
   .route("/v1/responses", post(responses_passthrough).get(responses_ws_handler))
   .route("/responses",   post(responses_passthrough).get(responses_ws_handler))
   ```
   axum routes a GET-with-`Upgrade: websocket` header through `responses_ws_handler`; plain GETs will 400 from the handler (reject non-upgrade GETs).

4. **Frame parsing.** Small helpers:
   - `parse_client_frame(text: &str) -> Result<ClientEvent, FrameError>` — decode `response.create` (and reject unsupported types with a structured error frame back).
   - `serialize_server_event(event: &ResponsesEvent) -> String` — already JSON, just `serde_json::to_string`.
   - Non-text frames (binary, ping/pong) pass through axum's built-in keep-alive; we only act on `Message::Text`. Close frames end the session.

## Behavior Details

### Upgrade handshake

- Handler accepts the upgrade and captures the `HeaderMap`. Headers are filtered through the existing `allowed_headers` list (`authorization`, `content-type`, `accept`, `user-agent`, `openai-beta`, `openai-organization`, `x-request-id`) plus session ID aliases (`session_id`, `session-id`, `x-session-id`).
- If `Authorization` is missing, accept the upgrade anyway (matches HTTP behavior — the connector will surface the upstream 401). Log a warning, same as HTTP.

### Request construction

For each `response.create` frame:

- Build a byte body from `event.response` (the inner create payload), force `"stream": true` if not already present (transport-only field — upstream HTTP needs it).
- Use the upgrade-time filtered headers as the per-request header set. Codex only authenticates once per connection, so the Authorization from the upgrade carries through.

### Session recording

Identical to HTTP:

- Per-connection: generate or reuse client-supplied session ID on the first frame; subsequent frames reuse it.
- Per-frame: new `request_id`, emit `Started` + `RequestRecorded` on frame receipt, `ResponseCompleted` on terminal event.
- Tool-result extraction from `input` array works unchanged (same code path).

### Error handling

| Situation | Lunaroute response | Connection |
| --- | --- | --- |
| Unsupported client frame type | Send `{ "type": "error", "error": { "code": "unsupported_event_type", "message": … } }` | Stay open |
| Malformed JSON | Send structured error frame | Stay open |
| Marker targets non-OpenAI provider | Send error frame (same message as HTTP cross-dialect rejection) | Stay open |
| Upstream 4xx/5xx | Translate to `response.failed` / `response.error` event; preserve upstream body in `error` | Stay open |
| Upstream timeout / network error | Send error frame | Stay open |
| Client close | Cancel in-flight upstream stream, close WS | — |

### Concurrency

One task per WebSocket connection, running the read loop. No spawning per frame; the loop awaits the event stream to completion before the next `recv`. This gives us free sequential-in-flight semantics without queues or locks.

### Terminal events

A response is considered complete (read-loop can resume) when any of these arrive, or the upstream stream ends:

- `response.completed` — normal success.
- `response.failed` — model-side failure; payload includes the error.
- `response.incomplete` — truncated (e.g., max_output_tokens). Treated as terminal; Codex handles it.
- `response.cancelled` — if we later support `response.cancel` frames from the client.
- Upstream stream end without any of the above → synthetic `response.error` frame is emitted, then the loop resumes.

## Configuration

No new config knobs. WS is enabled whenever passthrough mode is (it's a second transport on the same routes). If we need an opt-out later, a `passthrough.websocket_enabled: bool` in `config.yaml` defaulting to `true` is the shape.

## Testing

### Unit tests

- `parse_client_frame` — accepts valid `response.create`, rejects unknown `type`, rejects malformed JSON.
- `serialize_server_event` — round-trips each Responses event variant we care about.
- Header filtering for the upgrade path (reuses existing helper, just proves the WS path hits it).

### Integration tests

New integration test in `crates/lunaroute-integration-tests`:

- Spin up lunaroute with a wiremock upstream that serves `/v1/responses` SSE.
- Open WS with `tokio-tungstenite` to `ws://localhost:<port>/v1/responses`.
- Send a `response.create`; assert the WS frames received match the upstream SSE events.
- Send a second `response.create` on the same connection; assert sequential processing.
- Assert session events (Started, RequestRecorded, ResponseCompleted) are emitted for each frame.
- Assert LUNAROUTE marker routes to an overridden provider (same-dialect) and the correct upstream is hit.
- Assert error frame when the mock upstream returns 500.

### Manual smoke

Documented in the plan (not the spec): configure Codex CLI with `openai_base_url = "http://127.0.0.1:8081/v1"` and `supports_websockets = true`, confirm end-to-end completion + session shows in the UI.

## Observability

New metrics (via the existing `Metrics` facade):

- `lunaroute_ws_connections_total{endpoint="responses"}` — counter.
- `lunaroute_ws_connection_duration_seconds{endpoint="responses"}` — histogram.
- `lunaroute_ws_frames_received_total{endpoint="responses", type="response.create"}` — counter.
- `lunaroute_ws_frames_sent_total{endpoint="responses", terminal="true|false"}` — counter.

Session events are the existing variants, so the UI needs no changes.

## Migration / Compatibility

- Existing HTTP `/responses` and `/v1/responses` behavior is unchanged. The refactor only moves code; the public handler signature is identical.
- Clients that upgrade to WebSocket start getting frame responses; clients that POST keep getting SSE. Codex picks based on `supports_websockets`.
- No config file changes required for existing users.

## Risks

- **Refactor regression.** Moving session-recording / marker / metrics code into `process_responses_request` is mechanical but large. Mitigated by the existing HTTP integration tests passing unchanged.
- **Frame format drift.** OpenAI may add new client message types (`response.cancel`?). Initial scope rejects unknowns with a structured error; extend as Codex starts sending them.
- **Back-pressure.** If upstream emits events faster than the WS can send (unlikely over localhost), the send-side buffering in `tokio-tungstenite` handles it; we don't need explicit flow control. Documented but not tested.
- **`store=false` + fast continuation.** A user who explicitly sets `store=false` and relies on WS-local cache loses that optimization. Documented. True WS→WS is the escape hatch if this ever matters.

## Open Questions

None blocking. The `supports_websockets` default, exact connection cap, and cancel-frame support can evolve as Codex's behavior evolves; nothing here locks us in.
