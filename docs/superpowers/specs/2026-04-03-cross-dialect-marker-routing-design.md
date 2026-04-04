# Cross-Dialect Marker Routing

**Date:** 2026-04-03
**Status:** Draft
**Extends:** [LUNAROUTE Marker-Based Provider Routing](2026-04-03-lunaroute-marker-routing-design.md)

## Problem

LUNAROUTE marker routing currently rejects cross-dialect requests — e.g., an Anthropic-format request (from Claude Code) targeting an OpenAI-type provider (like Kimi K2.5 on Cloudflare). The original spec explicitly scoped this out. Now it's needed.

## Solution

When `messages_passthrough()` detects a marker targeting a provider whose dialect doesn't match the request format, instead of returning an error, translate the request through the existing normalization pipeline and use the target provider's `Provider` trait implementation.

All conversion pieces already exist. No new modules or egress changes needed.

## Scope

**In scope:** Anthropic-format request → OpenAI-type provider (the immediate need).

**Future work:** OpenAI-format request → Anthropic-type provider (symmetric case in `chat_completions_passthrough()`). Same pattern but not needed yet.

## Data Flow

```
Claude Code (Anthropic format, /v1/messages)
  → messages_passthrough()
    → extract session_id from metadata (raw JSON, before parse)
    → extract LUNAROUTE marker → provider name
    → detect connector_type == OpenAI (cross-dialect)
    → strip marker from request
    → apply model_override to raw JSON
    → [session recording: provider = marker provider name, listener = "anthropic"]
    → parse raw JSON → AnthropicMessagesRequest (serde_json::from_value)
    → to_normalized() → NormalizedRequest
    → force NormalizedRequest.stream to match chosen path
    → openai_connector.send() or .stream() via Provider trait
    → NormalizedResponse / NormalizedStreamEvent stream
    → from_normalized() or stream_event_to_anthropic_events()
  → Client (Anthropic format response)
```

## Design Decisions

**Authentication:** The cross-dialect path uses the target provider's configured auth (e.g., `CLOUDFLARE_API_KEY` from the registry), not client-provided headers. The client authenticates to LunaRoute; LunaRoute authenticates to the upstream provider. Client headers like `x-api-key` and `Authorization` are not forwarded in cross-dialect mode. This is intentional — the client's Anthropic API key is meaningless to Cloudflare.

**Metadata preservation:** The `AnthropicMessagesRequest` struct does not have a `metadata` field, so client metadata is dropped during typed parsing. This is acceptable because: (a) session_id extraction from `metadata.user_id` happens from raw JSON *before* the cross-dialect branch, so session recording works; (b) the target OpenAI provider doesn't understand Anthropic metadata anyway.

**HTTP status codes:** Successful cross-dialect responses always return HTTP 200 (since `Provider::send()` returns a typed `NormalizedResponse`, not raw status codes). Upstream 4xx/5xx errors are caught by `Provider::send()`/`stream()` and surface as `IngressError::ProviderError`. This differs from same-dialect passthrough which forwards the upstream status code directly.

**Feature coverage:** The normalization pipeline handles text messages, tool_use, and tool_result. Advanced Anthropic features (extended thinking, cache control hints, images) may not round-trip cleanly. This is acceptable for the initial use case (routing to Kimi K2.5 which only supports text/tools).

**`stream` field consistency:** Before calling `Provider::send()`, force `normalized.stream = false`. Before calling `Provider::stream()`, force `normalized.stream = true`. This prevents mismatches between the chosen code path and the request body sent to the upstream.

## Changes

### 1. `crates/lunaroute-ingress/src/anthropic.rs` — `messages_passthrough()`

**Modified function structure (pseudocode):**

```
messages_passthrough():
  extract marker
  match marker:
    Provider(name) =>
      lookup in registry
      if entry.connector_type == Anthropic:
        // same-dialect: existing logic (set override_connector)
      else if entry.connector_type == OpenAI:
        // cross-dialect: store openai_connector + set flag
        cross_dialect_connector = entry.openai_connector
        marker_provider_name = name
        apply model_override to raw JSON
      strip marker
    Clear => strip marker
    None => pass

  // Session ID extraction (unchanged, from raw JSON)
  // Header collection (unchanged, but only used for same-dialect path)

  extract model, is_streaming from raw JSON

  // Session recording (shared between paths)
  // provider field = marker_provider_name.unwrap_or("anthropic")
  // listener field = "anthropic" (always, since request arrived on /v1/messages)

  if cross_dialect_connector.is_some():
    // === CROSS-DIALECT PATH (returns early) ===
    parse req -> AnthropicMessagesRequest
    to_normalized() -> NormalizedRequest

    if is_streaming:
      normalized.stream = true
      connector.stream(normalized) -> NormalizedStreamEvent stream
      convert via stream_event_to_anthropic_events() -> Anthropic SSE
      return SSE response
    else:
      normalized.stream = false
      connector.send(normalized) -> NormalizedResponse
      from_normalized() -> AnthropicResponse
      return JSON response

  // === SAME-DIALECT PATH (existing code, unchanged) ===
  existing passthrough logic...
```

**Current code being replaced (lines 895-904):**
```rust
if entry.connector_type != crate::ProviderType::Anthropic {
    return Err(IngressError::InvalidRequest(format!(
        "LUNAROUTE marker targets provider '{}' ({:?}) but request uses Anthropic format. Cross-dialect routing requires normalized mode.",
        name, entry.connector_type
    )));
}
```

**Non-streaming cross-dialect path:**
```rust
let typed_req: AnthropicMessagesRequest = serde_json::from_value(req.clone())
    .map_err(|e| IngressError::InvalidRequest(
        format!("Failed to parse request for cross-dialect routing: {}", e)
    ))?;

let mut normalized = to_normalized(typed_req)?;
normalized.stream = false;

let normalized_resp = cross_dialect_connector.send(normalized).await
    .map_err(|e| IngressError::ProviderError(e.to_string()))?;

let anthropic_resp = from_normalized(normalized_resp);
return Ok(Json(anthropic_resp).into_response());
```

**Streaming cross-dialect path:**
```rust
let typed_req: AnthropicMessagesRequest = serde_json::from_value(req.clone())
    .map_err(|e| IngressError::InvalidRequest(
        format!("Failed to parse request for cross-dialect routing: {}", e)
    ))?;

let mut normalized = to_normalized(typed_req)?;
normalized.stream = true;

let event_stream = cross_dialect_connector.stream(normalized).await
    .map_err(|e| IngressError::ProviderError(e.to_string()))?;

// model here is the already-overridden model from raw JSON extraction
let stream_id = Arc::new(format!("msg_{}", uuid::Uuid::new_v4().simple()));
let model_arc = Arc::new(model.clone());

let sse_stream = event_stream
    .scan(false, move |content_block_started, result| {
        let stream_id = Arc::clone(&stream_id);
        let model = Arc::clone(&model_arc);
        let events = match result {
            Ok(event) => stream_event_to_anthropic_events(
                event, stream_id.as_str(), model.as_str(), content_block_started,
            ),
            Err(e) => {
                tracing::error!("Cross-dialect stream error: {}", e);
                vec![]
            }
        };
        futures::future::ready(Some(futures::stream::iter(events.into_iter().map(Ok))))
    })
    .flatten();

return Ok(Sse::new(sse_stream).into_response());
```

**Session recording:** The session recording code block (lines 1064-1227) runs before the cross-dialect branch. The `provider` field in `SessionEvent::Started` should use the marker provider name when present:

```rust
provider: marker_provider_name.clone().unwrap_or_else(|| "anthropic".to_string()),
listener: "anthropic".to_string(),
```

**Error handling:** When `Provider::send()` or `Provider::stream()` returns an error (upstream 4xx/5xx), it surfaces as `IngressError::ProviderError`. The existing `IngressError` → HTTP response conversion in the error handler produces a JSON error response. This is sufficient for v1 — the client gets an error with context about what went wrong. Translating upstream error codes (429, 500) into Anthropic-format `{"type": "error", "error": {...}}` is a future improvement.

### 2. Config: `eran.yaml` — add model override

```yaml
kimik25:
    provider_type: "openai"
    enabled: true
    api_key: "${CLOUDFLARE_API_KEY}"
    base_url: "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/ai/v1"
    model: "@cf/moonshotai/kimi-k2.5"
```

The `model` field becomes the `model_override` in the registry entry. Without it, the client's model name (e.g., `claude-sonnet-4-20250514`) would be sent to Cloudflare, which wouldn't recognize it.

## What Stays the Same

- Default passthrough behavior (Anthropic→Anthropic, OpenAI→OpenAI) — zero cost, untouched
- Provider registry structure — no changes
- Egress modules — no changes (Provider trait methods already public)
- Normalized mode handlers — untouched
- Config parsing — `model` field already maps to `model_override` in the registry

## Performance

- Cross-dialect marker requests: ~1ms normalization overhead (parse + convert + serialize)
- Normal same-dialect requests: zero additional cost (existing passthrough path unchanged)
- Normalization is per-request, not per-chunk. Streaming chunks flow through the Provider trait's stream implementation which already handles SSE parsing efficiently.

## Testing

1. **Round-trip:** Anthropic JSON → normalize → OpenAI JSON preserves message content, tools, system prompt
2. **Integration (non-streaming):** Anthropic-format request with LUNAROUTE marker targeting OpenAI provider → response in Anthropic format
3. **Integration (streaming):** Same with `"stream": true` → valid Anthropic SSE events
4. **Model override:** Verify the model sent to upstream is `model_override`, not client's original model
5. **Error path:** Provider returns error → client gets sensible error response
6. **Tool use round-trip:** Anthropic tool_use/tool_result → normalized → OpenAI function_call/tool_calls and back
7. **Session recording:** Events record correct provider name ("kimik25") and listener ("anthropic")
8. **Regression:** Existing same-dialect passthrough tests still pass
