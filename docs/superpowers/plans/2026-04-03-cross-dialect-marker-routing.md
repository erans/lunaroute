# Cross-Dialect Marker Routing Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable LUNAROUTE marker routing from Anthropic-format requests to OpenAI-type providers (e.g., Claude Code → Kimi K2.5 on Cloudflare).

**Architecture:** When `messages_passthrough()` detects a cross-dialect marker, parse the Anthropic JSON into typed form, normalize, send via the OpenAI connector's `Provider` trait, and convert the response back to Anthropic format. All building blocks exist; this is wiring.

**Tech Stack:** Rust, axum, serde_json, lunaroute-core normalization, lunaroute-egress Provider trait

**Spec:** `docs/superpowers/specs/2026-04-03-cross-dialect-marker-routing-design.md`

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `crates/lunaroute-ingress/src/anthropic.rs` | Modify (lines 888-938, 1096) | Cross-dialect routing in passthrough handler, session recording provider field |
| `examples/configs/private/eran.yaml` | Modify (kimik25 section) | Add model override |

---

### Task 1: Add non-streaming cross-dialect routing

**Files:**
- Modify: `crates/lunaroute-ingress/src/anthropic.rs:888-938` (marker match block)
- Modify: `crates/lunaroute-ingress/src/anthropic.rs:1096` (session recording provider field)
- Test: `crates/lunaroute-ingress/src/anthropic.rs` (inline tests module at line 1870)

- [ ] **Step 1: Write failing test for cross-dialect non-streaming**

In `crates/lunaroute-ingress/src/anthropic.rs`, inside the existing `mod tests` block (line 1870), add a test that verifies Anthropic JSON can be parsed, normalized, sent through a mock OpenAI Provider, and converted back to Anthropic format. This tests the conversion pipeline, not the handler itself.

```rust
#[test]
fn test_cross_dialect_anthropic_to_normalized_roundtrip() {
    // Simulate what the cross-dialect path does:
    // 1. Raw Anthropic JSON → AnthropicMessagesRequest
    // 2. to_normalized() → NormalizedRequest
    // 3. from_normalized(NormalizedResponse) → AnthropicResponse

    let raw_json = serde_json::json!({
        "model": "@cf/moonshotai/kimi-k2.5",
        "messages": [
            {"role": "user", "content": "Hello from Claude Code"}
        ],
        "max_tokens": 1024,
        "stream": false
    });

    // Step 1: Parse raw JSON into typed Anthropic request
    let typed_req: AnthropicMessagesRequest = serde_json::from_value(raw_json).unwrap();
    assert_eq!(typed_req.model, "@cf/moonshotai/kimi-k2.5");

    // Step 2: Normalize
    let mut normalized = to_normalized(typed_req).unwrap();
    normalized.stream = false;
    assert_eq!(normalized.model, "@cf/moonshotai/kimi-k2.5");
    assert_eq!(normalized.messages.len(), 1);
    assert!(!normalized.stream);

    // Step 3: Simulate provider response and convert back
    let normalized_resp = NormalizedResponse {
        id: "chatcmpl-test".to_string(),
        model: "@cf/moonshotai/kimi-k2.5".to_string(),
        choices: vec![Choice {
            index: 0,
            message: Message {
                role: Role::Assistant,
                content: MessageContent::Text("Hello from Kimi".to_string()),
                name: None,
                tool_calls: vec![],
                tool_call_id: None,
            },
            finish_reason: Some(FinishReason::Stop),
        }],
        usage: Usage {
            prompt_tokens: 5,
            completion_tokens: 3,
            total_tokens: 8,
        },
        created: 1234567890,
        metadata: std::collections::HashMap::new(),
    };

    let anthropic_resp = from_normalized(normalized_resp);
    assert_eq!(anthropic_resp.model, "@cf/moonshotai/kimi-k2.5");
    assert_eq!(anthropic_resp.role, "assistant");
    assert_eq!(anthropic_resp.stop_reason, Some("end_turn".to_string()));
    // Verify content is preserved
    assert!(!anthropic_resp.content.is_empty());
    match &anthropic_resp.content[0] {
        AnthropicContent::Text { text } => assert_eq!(text, "Hello from Kimi"),
        _ => panic!("Expected text content"),
    }
}
```

- [ ] **Step 2: Run test to verify it passes**

This is a pure conversion test that exercises existing code — it should pass immediately since `to_normalized()` and `from_normalized()` already exist.

Run: `cargo test -p lunaroute-ingress -- test_cross_dialect_anthropic_to_normalized_roundtrip -v`
Expected: PASS

- [ ] **Step 3: Add cross-dialect variable and modify marker match block**

In `crates/lunaroute-ingress/src/anthropic.rs`, at line 888, add a new variable alongside the existing `override_connector` and `marker_provider_name`:

```rust
let mut override_connector: Option<Arc<lunaroute_egress::anthropic::AnthropicConnector>> = None;
let mut cross_dialect_connector: Option<Arc<lunaroute_egress::openai::OpenAIConnector>> = None;
let mut marker_provider_name: Option<String> = None;
```

Then replace lines 895-904 (the error block) with cross-dialect handling:

```rust
if entry.connector_type != crate::ProviderType::Anthropic {
    // Cross-dialect: Anthropic request → OpenAI provider
    if let Some(ref connector) = entry.openai_connector {
        tracing::info!(
            "LUNAROUTE marker: cross-dialect routing to OpenAI provider '{}', model_override={:?}",
            name,
            entry.model_override
        );
        cross_dialect_connector = Some(connector.clone());
        marker_provider_name = Some(name.clone());

        // Apply model override
        if let Some(ref model) = entry.model_override {
            req["model"] = serde_json::Value::String(model.clone());
        }
    } else {
        tracing::warn!(
            "LUNAROUTE marker '{}' targets OpenAI provider but no OpenAI connector available",
            name
        );
    }
} else {
    // Same-dialect: existing Anthropic→Anthropic logic (unchanged)
    if let Some(ref connector) = entry.anthropic_connector {
```

Note: the existing same-dialect code (lines 906-918) stays inside the `else` block. The closing braces need to be adjusted.

- [ ] **Step 4: Add cross-dialect early return for non-streaming**

After the session recording block (after line 1227) and before the existing `if is_streaming {` block (line 1229), add the cross-dialect early return:

```rust
// Cross-dialect routing: Anthropic request → OpenAI provider via normalization
// Note: Provider trait is already imported at file scope (line 21)
if let Some(ref cd_connector) = cross_dialect_connector {
    let typed_req: AnthropicMessagesRequest = serde_json::from_value(req.clone())
        .map_err(|e| IngressError::InvalidRequest(
            format!("Failed to parse request for cross-dialect routing: {}", e)
        ))?;

    if is_streaming {
        // Streaming cross-dialect handled in Task 2
        todo!("Streaming cross-dialect not yet implemented");
    } else {
        let mut normalized = to_normalized(typed_req)?;
        normalized.stream = false;

        let normalized_resp = cd_connector.send(normalized).await
            .map_err(|e| IngressError::ProviderError(e.to_string()))?;

        let anthropic_resp = from_normalized(normalized_resp);
        return Ok(Json(anthropic_resp).into_response());
    }
}
```

- [ ] **Step 5: Fix session recording provider field**

At line 1096, change the hardcoded provider string:

```rust
// Before:
provider: "anthropic".to_string(),

// After:
provider: marker_provider_name.as_deref().unwrap_or("anthropic").to_string(),
```

The `listener` field on line 1097 stays `"anthropic"`.

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p lunaroute-ingress`
Expected: Compiles with no errors (may have a warning about the `todo!()` in streaming path)

- [ ] **Step 7: Run all existing tests**

Run: `cargo test -p lunaroute-ingress`
Expected: All existing tests pass. The new test from Step 1 passes.

- [ ] **Step 8: Commit**

```bash
git add crates/lunaroute-ingress/src/anthropic.rs
git commit -m "feat: add non-streaming cross-dialect marker routing (Anthropic→OpenAI)"
```

---

### Task 2: Add streaming cross-dialect routing

**Files:**
- Modify: `crates/lunaroute-ingress/src/anthropic.rs` (replace the `todo!()` from Task 1)
- Test: `crates/lunaroute-ingress/src/anthropic.rs` (inline tests module)

- [ ] **Step 1: Write failing test for streaming conversion**

Add a test in the `mod tests` block that verifies `NormalizedStreamEvent` events convert to Anthropic SSE events correctly (this exercises `stream_event_to_anthropic_events`):

```rust
#[test]
fn test_cross_dialect_stream_event_conversion() {
    use lunaroute_core::normalized::Delta;

    // Simulate what the streaming cross-dialect path does:
    // NormalizedStreamEvent (from OpenAI connector) → Anthropic SSE events

    let mut content_block_started = false;

    // Test a text delta event (struct variant with index + delta fields)
    let event = NormalizedStreamEvent::Delta {
        index: 0,
        delta: Delta {
            role: None,
            content: Some("Hello from Kimi".to_string()),
        },
    };

    let anthropic_events = stream_event_to_anthropic_events(
        event,
        "msg_test123",
        "@cf/moonshotai/kimi-k2.5",
        &mut content_block_started,
    );

    // Should produce: ContentBlockStart + ContentBlockDelta
    assert!(content_block_started);
    assert!(anthropic_events.len() >= 2);
}
```

- [ ] **Step 2: Run test to verify it passes**

This exercises existing `stream_event_to_anthropic_events()` which already works.

Run: `cargo test -p lunaroute-ingress -- test_cross_dialect_stream_event_conversion -v`
Expected: PASS

- [ ] **Step 3: Replace the `todo!()` with streaming cross-dialect implementation**

In `crates/lunaroute-ingress/src/anthropic.rs`, find the `todo!("Streaming cross-dialect not yet implemented")` from Task 1 and replace the entire streaming branch:

```rust
if is_streaming {
    let mut normalized = to_normalized(typed_req)?;
    normalized.stream = true;

    let event_stream = cd_connector.stream(normalized).await
        .map_err(|e| IngressError::ProviderError(e.to_string()))?;

    let stream_id = Arc::new(format!("msg_{}", uuid::Uuid::new_v4().simple()));
    let model_arc = Arc::new(model.clone());

    let sse_stream = event_stream
        .scan(false, move |content_block_started, result| {
            let stream_id = Arc::clone(&stream_id);
            let model = Arc::clone(&model_arc);
            let events = match result {
                Ok(event) => stream_event_to_anthropic_events(
                    event,
                    stream_id.as_str(),
                    model.as_str(),
                    content_block_started,
                ),
                Err(e) => {
                    tracing::error!("Cross-dialect stream error: {}", e);
                    vec![]
                }
            };
            futures::future::ready(Some(
                futures::stream::iter(events.into_iter().map(Ok)),
            ))
        })
        .flatten();

    return Ok(Sse::new(sse_stream)
        .keep_alive(
            KeepAlive::new()
                .interval(std::time::Duration::from_secs(
                    state.sse_keepalive_interval_secs,
                ))
                .text(""),
        )
        .into_response());
}
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p lunaroute-ingress`
Expected: Compiles cleanly, no `todo!()` warning

- [ ] **Step 5: Run all tests**

Run: `cargo test -p lunaroute-ingress`
Expected: All tests pass

- [ ] **Step 6: Commit**

```bash
git add crates/lunaroute-ingress/src/anthropic.rs
git commit -m "feat: add streaming cross-dialect marker routing (Anthropic→OpenAI)"
```

---

### Task 3: Add model override to eran.yaml config

**Files:**
- Modify: `examples/configs/private/eran.yaml` (kimik25 section)

- [ ] **Step 1: Add model override to kimik25 provider config**

In `examples/configs/private/eran.yaml`, add the `model` field to the kimik25 provider (after the `base_url` line):

```yaml
  kimik25:
    provider_type: "openai"
    enabled: true
    api_key: "${CLOUDFLARE_API_KEY}"
    base_url: "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/ai/v1"
    model: "@cf/moonshotai/kimi-k2.5"
```

- [ ] **Step 2: Verify full project builds**

Run: `cargo build`
Expected: Successful build

- [ ] **Step 3: Commit**

```bash
git add examples/configs/private/eran.yaml
git commit -m "config: add model override for kimik25 Cloudflare provider"
```

---

### Task 4: Add tool use round-trip test

**Files:**
- Test: `crates/lunaroute-ingress/src/anthropic.rs` (inline tests module)

- [ ] **Step 1: Write test for tool use normalization round-trip**

```rust
#[test]
fn test_cross_dialect_tool_use_roundtrip() {
    // Anthropic tool_use → normalized → back to Anthropic
    let raw_json = serde_json::json!({
        "model": "@cf/moonshotai/kimi-k2.5",
        "messages": [
            {"role": "user", "content": "What's the weather?"}
        ],
        "max_tokens": 1024,
        "stream": false,
        "tools": [{
            "name": "get_weather",
            "description": "Get weather for a location",
            "input_schema": {
                "type": "object",
                "properties": {
                    "location": {"type": "string"}
                },
                "required": ["location"]
            }
        }]
    });

    let typed_req: AnthropicMessagesRequest = serde_json::from_value(raw_json).unwrap();
    let normalized = to_normalized(typed_req).unwrap();

    // Verify tool was normalized
    assert_eq!(normalized.tools.len(), 1);
    assert_eq!(normalized.tools[0].function.name, "get_weather");

    // Simulate provider response with tool call
    let normalized_resp = NormalizedResponse {
        id: "chatcmpl-test".to_string(),
        model: "@cf/moonshotai/kimi-k2.5".to_string(),
        choices: vec![Choice {
            index: 0,
            message: Message {
                role: Role::Assistant,
                content: MessageContent::Text(String::new()),
                name: None,
                tool_calls: vec![ToolCall {
                    id: "call_abc123".to_string(),
                    tool_type: "function".to_string(),
                    function: FunctionCall {
                        name: "get_weather".to_string(),
                        arguments: r#"{"location":"San Francisco"}"#.to_string(),
                    },
                }],
                tool_call_id: None,
            },
            finish_reason: Some(FinishReason::ToolCalls),
        }],
        usage: Usage {
            prompt_tokens: 20,
            completion_tokens: 10,
            total_tokens: 30,
        },
        created: 1234567890,
        metadata: std::collections::HashMap::new(),
    };

    let anthropic_resp = from_normalized(normalized_resp);
    assert_eq!(anthropic_resp.stop_reason, Some("tool_use".to_string()));

    // Find the tool_use block in content
    let tool_use = anthropic_resp.content.iter().find(|c| {
        matches!(c, AnthropicContent::ToolUse { .. })
    });
    assert!(tool_use.is_some(), "Expected tool_use block in response");

    if let Some(AnthropicContent::ToolUse { id, name, input }) = tool_use {
        assert_eq!(id, "call_abc123");
        assert_eq!(name, "get_weather");
        assert_eq!(input["location"], "San Francisco");
    }
}
```

- [ ] **Step 2: Run test**

Run: `cargo test -p lunaroute-ingress -- test_cross_dialect_tool_use_roundtrip -v`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/lunaroute-ingress/src/anthropic.rs
git commit -m "test: add cross-dialect tool use round-trip test"
```

---

### Task 5: Regression — verify existing tests pass

- [ ] **Step 1: Run full test suite**

Run: `cargo test`
Expected: All tests pass

- [ ] **Step 2: Verify build in release mode**

Run: `cargo build --release`
Expected: Clean build
