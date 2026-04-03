# LUNAROUTE Marker-Based Provider Routing

## Overview

Add support for `[LUNAROUTE:provider_name]` markers embedded in request bodies to dynamically override which provider handles a request in passthrough mode. This enables Claude Code users to switch providers on-the-fly via a `#!provider` command without restarting sessions or changing configuration.

## Motivation

Currently, passthrough mode routes requests based on which endpoint they arrive on (OpenAI format -> OpenAI connector, Anthropic format -> Anthropic connector). There is no way for the end user to dynamically redirect a request to a different provider at request time. This feature allows users to type `#!sonnet` in Claude Code, which injects a `[LUNAROUTE:sonnet]` marker into the request body via a hook, and the proxy routes that request to the provider named `sonnet` in the config.

## User-Facing Behavior

### What the user types in chat

- `#!sonnet` — route subsequent requests to the provider named "sonnet"
- `#!sonnet rewrite this function` — route this request to "sonnet", inline with a message
- `#!clear` — stop overriding, return to default routing

### What the proxy sees

The marker arrives as a `<system-reminder>` block injected by Claude Code's `additionalContext` hook mechanism:

```json
{
  "model": "claude-opus-4-20250514",
  "messages": [
    {
      "role": "user",
      "content": [
        {
          "type": "text",
          "text": "...the user's message..."
        },
        {
          "type": "text",
          "text": "<system-reminder>\n[LUNAROUTE:sonnet]\n</system-reminder>"
        }
      ]
    }
  ]
}
```

## Design

### 1. Marker Detection & Extraction

A new module `crates/lunaroute-ingress/src/marker.rs` with these public types and functions:

```rust
/// Result of marker extraction
pub enum MarkerResult {
    /// A provider override marker was found
    Provider(String),
    /// A "clear" marker was found — strip and route normally
    Clear,
    /// No marker found
    None,
}

/// Scan a request body (serde_json::Value) for [LUNAROUTE:xxx] marker.
/// Searches all text content blocks in the messages array.
/// Returns MarkerResult::Provider(name), MarkerResult::Clear, or MarkerResult::None.
/// If multiple markers exist, returns the first and logs a warning.
pub fn extract_marker(req: &serde_json::Value) -> MarkerResult

/// Remove [LUNAROUTE:xxx] marker text from the request body.
/// - Standalone content block (text is only the system-reminder with marker): remove entire block.
/// - Inline in a larger text block: regex-replace the marker (and surrounding system-reminder tags).
/// - String content (not array): regex-replace within the string.
/// After stripping, removes empty text blocks and empty messages.
pub fn strip_marker(req: &mut serde_json::Value)
```

**Extraction regex:** `\[LUNAROUTE:([a-zA-Z0-9._-]+)\]`

**Stripping regexes:**
- Standalone system-reminder block: `<system-reminder>\s*\[LUNAROUTE:[a-zA-Z0-9._-]+\]\s*</system-reminder>`
- Inline marker only: `\[LUNAROUTE:[a-zA-Z0-9._-]+\]`

The stripping logic tries the standalone regex first. If the entire text content matches, the whole content block is removed. Otherwise, the inline regex removes just the marker text.

The functions scan the `messages` array and handle both content formats:
- String content: `"content": "text with [LUNAROUTE:sonnet]"`
- Array content: `"content": [{"type": "text", "text": "..."}]`

This covers both OpenAI and Anthropic request shapes in passthrough mode.

### 2. Provider Registry & Model Override

A new type representing all available providers, built at startup:

```rust
pub enum ProviderType {
    OpenAI,
    Anthropic,
}

pub struct ProviderEntry {
    pub connector_type: ProviderType,
    pub openai_connector: Option<Arc<OpenAIConnector>>,
    pub anthropic_connector: Option<Arc<AnthropicConnector>>,
    pub model_override: Option<String>,
}

pub type ProviderRegistry = HashMap<String, ProviderEntry>;
```

Each entry holds both connector options but only one will be populated (matching `connector_type`). Both are kept as `Option` to accommodate future cross-dialect support.

Added as `Option<Arc<ProviderRegistry>>` to both `OpenAIPassthroughState` and `PassthroughState`.

#### Config changes

The current `ProvidersConfig` is a struct with hardcoded `openai` and `anthropic` fields. To support arbitrary named providers, add an `extra` field:

```rust
pub struct ProvidersConfig {
    pub openai: Option<ProviderSettings>,
    pub anthropic: Option<ProviderSettings>,
    /// Additional named providers for marker-based routing
    #[serde(default, flatten)]
    pub extra: HashMap<String, ProviderSettings>,
}
```

**Serde flatten caveats:** `#[serde(flatten)]` will consume all unknown keys into the HashMap. To prevent issues:
- Add a post-deserialization validation step that rejects extra entries with keys `"openai"` or `"anthropic"` (prevents double-parsing with the typed fields).
- Add validation that rejects entries missing a `provider_type` field.
- The `Default` impl must be updated to include `extra: HashMap::new()`.
- All existing `ProviderSettings` literals (e.g. in `merge_env()`) must be updated to include the new `model: None` and `provider_type: None` fields.

Each provider gains two new optional fields:

```rust
pub struct ProviderSettings {
    // ... existing fields ...
    /// Provider dialect type. Required for extra providers.
    /// Inferred for the built-in "openai" and "anthropic" keys.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,  // "openai" or "anthropic"

    /// Model ID override. When this provider is targeted via marker,
    /// the request body's model field is rewritten to this value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}
```

Example config:

```yaml
providers:
  anthropic:
    enabled: true
    api_key: "${ANTHROPIC_API_KEY}"
  sonnet:
    enabled: true
    provider_type: "anthropic"
    api_key: "${ANTHROPIC_API_KEY}"
    model: "claude-sonnet-4-20250514"
  gpt4o:
    enabled: true
    provider_type: "openai"
    api_key: "${OPENAI_API_KEY}"
    model: "gpt-4o"
```

At startup, the server builds the `ProviderRegistry` from all providers (built-in + extra). The built-in `openai` and `anthropic` providers have their type inferred from the key name. Extra providers require an explicit `provider_type` field. For each extra provider, the startup code constructs a new connector instance (`OpenAIConnector` or `AnthropicConnector`) using the provider's `api_key`, `base_url`, and `http_client` settings — the same construction path used for the built-in providers.

When a marker routes to `sonnet`, the `model` field in the request body is rewritten to `claude-sonnet-4-20250514`.

### 3. Integration into Passthrough Handlers

Both `chat_completions_passthrough` (OpenAI) and `messages_passthrough` (Anthropic) gain this flow early in the function, before model extraction.

**Note:** The handler's `req` binding must be made mutable (`let mut req = req;` after the `Json(req)` extraction) since both `strip_marker` and model override mutate the body.

```
1. Parse JSON body (existing)
2. let mut req = req;
3. match extract_marker(&req):
   MarkerResult::Provider(name) =>
     a. Look up name in registry
     b. If found + same dialect: select that provider's connector + apply model_override
     c. If found + different dialect: return HTTP 400
     d. strip_marker(&mut req)
     e. If not found: log warning, strip_marker(&mut req), proceed with default
   MarkerResult::Clear =>
     a. strip_marker(&mut req), use default connector
   MarkerResult::None =>
     a. proceed normally
4. Extract model name (existing — now reads potentially-overridden model)
5. Session recording (existing — captures actual routed model/provider)
6. Send to provider via selected connector
```

The connector swap applies to **both streaming and non-streaming** `send_passthrough` / `stream_passthrough` call sites. Since cross-dialect routing returns HTTP 400, the overridden connector is always the same type as the handler's default connector, so the call signatures and return types naturally match (OpenAI handler swaps in an OpenAI connector, Anthropic handler swaps in an Anthropic connector):

```rust
let connector = if let Some(override_entry) = &provider_override {
    // Same dialect guaranteed — cross-dialect returned 400 above
    override_entry.anthropic_connector.as_ref().unwrap()
} else {
    &state.connector
};
// Non-streaming:
let response_result = connector.send_passthrough(req, passthrough_headers).await;
// Streaming:
let stream = connector.stream_passthrough(req, passthrough_headers).await;
```

### 4. Marker Stripping Strategy

Three cases for stripping:

1. **Standalone content block** — text is only `<system-reminder>\n[LUNAROUTE:xxx]\n</system-reminder>`. Remove the entire content block from the array.
2. **Inline in a larger text block** — regex-replace the `[LUNAROUTE:xxx]` pattern (and surrounding `<system-reminder>` tags if present), leaving the rest intact.
3. **String content** — when `"content"` is a plain string, regex-replace within it.

After stripping: remove empty text blocks, remove messages with empty content arrays.

### 5. Session Recording & Observability

- **`session_tags`** — populate the existing `session_tags: Vec<String>` field in `SessionMetadata` with `["lunaroute:sonnet"]` when a marker override is active.
- **Model tracking** — `model_requested` in `SessionEvent::Started` records the original model from the client. Add `"model_override:claude-sonnet-4-20250514"` to `session_tags` to capture the overridden model.
- **Logging** — `tracing::info!` when a marker is detected: provider name, model override value.
- **Metrics** — defer to follow-up. Use session tags for initial visibility.

### 6. Error Handling

| Scenario | Behavior |
|----------|----------|
| Unknown provider name | Log warning, proceed with default connector |
| Cross-dialect mismatch (e.g. Anthropic endpoint + OpenAI provider) | Return HTTP 400 with clear error message |
| Provider disabled (`enabled: false`) | Treat as unknown — log warning, fall through to default |
| Malformed marker (regex doesn't match) | No marker detected, request proceeds normally |
| Multiple markers in body | Use first match, log warning about extras |

### 7. Files Changed

| File | Change |
|------|--------|
| `crates/lunaroute-ingress/src/marker.rs` | **New** — `extract_marker()`, `strip_marker()`, `MarkerResult` enum, regex logic, unit tests |
| `crates/lunaroute-ingress/src/lib.rs` | Add `pub mod marker;` |
| `crates/lunaroute-ingress/src/openai.rs` | Add `provider_registry` to `OpenAIPassthroughState`, marker detection + connector swap in `chat_completions_passthrough` |
| `crates/lunaroute-ingress/src/anthropic.rs` | Add `provider_registry` to `PassthroughState`, marker detection + connector swap in `messages_passthrough` |
| `crates/lunaroute-ingress/src/multi_dialect.rs` | Pass `ProviderRegistry` through to both passthrough routers |
| `crates/lunaroute-server/src/config.rs` | Add `model: Option<String>` and `provider_type: Option<String>` to `ProviderSettings`; add `extra: HashMap` to `ProvidersConfig` |
| `crates/lunaroute-server/src/main.rs` (or connector build site) | Build `ProviderRegistry` from config at startup, pass to ingress |
| `config.example.yaml` | Add example showing marker-targeted provider with model override |

## Out of Scope

- **Cross-dialect translation** — routing an Anthropic-format request to an OpenAI provider (or vice versa) requires normalization. This is a future enhancement; for now, cross-dialect marker routing returns an error.
- **Sticky sessions** — the marker is per-request. Client-side hooks handle persistence (injecting the marker on every request until `#!clear`).
- **Client-side hook implementation** — the `#!sonnet` → `[LUNAROUTE:sonnet]` injection is handled by Claude Code hooks, not by the proxy.
- **Metrics labels** — dedicated Prometheus labels for marker-routed requests are deferred.
- **`/v1/responses` endpoint** — the OpenAI Responses API (`responses_passthrough`) uses a different body format (`input` array instead of `messages`). Marker support for this endpoint is deferred to a follow-up.
- **`/v1/models` endpoint** — unaffected by marker routing. Markers only appear in message request bodies.
