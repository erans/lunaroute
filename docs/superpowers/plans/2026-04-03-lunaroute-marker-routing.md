# LUNAROUTE Marker-Based Provider Routing Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable dynamic provider override via `[LUNAROUTE:provider_name]` markers in request bodies, so users can switch providers on-the-fly in Claude Code.

**Architecture:** A new `marker.rs` module in `lunaroute-ingress` handles extraction and stripping. Config gains `extra` providers with `provider_type` and `model` fields. At startup, a `ProviderRegistry` is built from all providers. Both passthrough handlers check for markers early, swap connectors, and strip markers before forwarding.

**Tech Stack:** Rust, serde_json, regex, axum, tokio

**Spec:** `docs/superpowers/specs/2026-04-03-lunaroute-marker-routing-design.md`

---

## File Structure

| File | Purpose |
|------|---------|
| `crates/lunaroute-ingress/src/marker.rs` | **New** — `MarkerResult` enum, `extract_marker()`, `strip_marker()` |
| `crates/lunaroute-ingress/src/lib.rs` | Add `pub mod marker;` |
| `crates/lunaroute-ingress/Cargo.toml` | Add `regex` dependency |
| `crates/lunaroute-ingress/src/openai.rs` | Add `provider_registry` to state, marker logic in handler |
| `crates/lunaroute-ingress/src/anthropic.rs` | Add `provider_registry` to state, marker logic in handler |
| `crates/lunaroute-ingress/src/multi_dialect.rs` | Thread `ProviderRegistry` through to passthrough routers |
| `crates/lunaroute-server/src/config.rs` | Add fields to `ProviderSettings` and `ProvidersConfig` |
| `crates/lunaroute-server/src/main.rs` | Build `ProviderRegistry`, pass to ingress |
| `config.example.yaml` | Add marker routing example |

---

### Task 1: Marker Extraction Module — `extract_marker()`

**Files:**
- Create: `crates/lunaroute-ingress/src/marker.rs`
- Modify: `crates/lunaroute-ingress/src/lib.rs:9-16`
- Modify: `crates/lunaroute-ingress/Cargo.toml:17-34`

- [ ] **Step 1: Add `regex` to lunaroute-ingress Cargo.toml**

Add to `[dependencies]` in `crates/lunaroute-ingress/Cargo.toml`:
```toml
regex = { workspace = true }
```

- [ ] **Step 2: Add `pub mod marker;` to lib.rs**

Add after the existing module declarations in `crates/lunaroute-ingress/src/lib.rs`:
```rust
pub mod marker;
```

- [ ] **Step 3: Write failing tests for `extract_marker`**

Create `crates/lunaroute-ingress/src/marker.rs` with the `MarkerResult` enum and tests only:

```rust
use regex::Regex;
use std::sync::LazyLock;

/// Regex to extract the provider name from a [LUNAROUTE:xxx] marker
static MARKER_EXTRACT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[LUNAROUTE:([a-zA-Z0-9._-]+)\]").unwrap());

/// Result of scanning a request body for a LUNAROUTE marker
#[derive(Debug, Clone, PartialEq)]
pub enum MarkerResult {
    /// A provider override marker was found
    Provider(String),
    /// A "clear" marker was found — strip and route normally
    Clear,
    /// No marker found
    None,
}

/// Scan a request body (serde_json::Value) for [LUNAROUTE:xxx] marker.
/// Searches the LAST user message only — old markers from previous turns
/// may persist in conversation history and must be ignored.
/// Returns the first marker found in the last user message.
/// Logs a warning if multiple markers exist.
pub fn extract_marker(req: &serde_json::Value) -> MarkerResult {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_marker_from_system_reminder_block() {
        let req = json!({
            "model": "claude-opus-4-20250514",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "hello world"},
                    {"type": "text", "text": "<system-reminder>\n[LUNAROUTE:sonnet]\n</system-reminder>"}
                ]
            }]
        });
        assert_eq!(extract_marker(&req), MarkerResult::Provider("sonnet".to_string()));
    }

    #[test]
    fn test_extract_marker_clear() {
        let req = json!({
            "model": "claude-opus-4-20250514",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "<system-reminder>\n[LUNAROUTE:clear]\n</system-reminder>"}
                ]
            }]
        });
        assert_eq!(extract_marker(&req), MarkerResult::Clear);
    }

    #[test]
    fn test_extract_marker_none() {
        let req = json!({
            "model": "claude-opus-4-20250514",
            "messages": [{
                "role": "user",
                "content": [{"type": "text", "text": "hello world"}]
            }]
        });
        assert_eq!(extract_marker(&req), MarkerResult::None);
    }

    #[test]
    fn test_extract_marker_string_content() {
        let req = json!({
            "model": "gpt-4",
            "messages": [{
                "role": "user",
                "content": "rewrite this [LUNAROUTE:gpt4o] using streams"
            }]
        });
        assert_eq!(extract_marker(&req), MarkerResult::Provider("gpt4o".to_string()));
    }

    #[test]
    fn test_extract_marker_no_messages() {
        let req = json!({"model": "gpt-4"});
        assert_eq!(extract_marker(&req), MarkerResult::None);
    }

    #[test]
    fn test_extract_marker_dots_and_dashes() {
        let req = json!({
            "messages": [{
                "role": "user",
                "content": "[LUNAROUTE:my-provider.v2]"
            }]
        });
        assert_eq!(extract_marker(&req), MarkerResult::Provider("my-provider.v2".to_string()));
    }

    #[test]
    fn test_extract_marker_multiple_returns_first() {
        let req = json!({
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "[LUNAROUTE:sonnet]"},
                    {"type": "text", "text": "[LUNAROUTE:gpt4o]"}
                ]
            }]
        });
        assert_eq!(extract_marker(&req), MarkerResult::Provider("sonnet".to_string()));
    }

    #[test]
    fn test_extract_marker_ignores_old_messages() {
        // Old marker in first user message, no marker in last user message
        let req = json!({
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "old request"},
                        {"type": "text", "text": "<system-reminder>\n[LUNAROUTE:sonnet]\n</system-reminder>"}
                    ]
                },
                {"role": "assistant", "content": "response"},
                {
                    "role": "user",
                    "content": [{"type": "text", "text": "new request without marker"}]
                }
            ]
        });
        assert_eq!(extract_marker(&req), MarkerResult::None);
    }

    #[test]
    fn test_extract_marker_uses_last_user_message() {
        // Old marker in first message, different marker in last
        let req = json!({
            "messages": [
                {
                    "role": "user",
                    "content": "<system-reminder>\n[LUNAROUTE:sonnet]\n</system-reminder>"
                },
                {"role": "assistant", "content": "response"},
                {
                    "role": "user",
                    "content": "<system-reminder>\n[LUNAROUTE:gpt4o]\n</system-reminder>"
                }
            ]
        });
        assert_eq!(extract_marker(&req), MarkerResult::Provider("gpt4o".to_string()));
    }
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p lunaroute-ingress marker`
Expected: FAIL — `not yet implemented`

- [ ] **Step 5: Implement `extract_marker`**

Replace the `todo!()` body in `extract_marker`:

```rust
pub fn extract_marker(req: &serde_json::Value) -> MarkerResult {
    let mut found: Vec<String> = Vec::new();

    if let Some(messages) = req.get("messages").and_then(|m| m.as_array()) {
        // Find the last user message (iterate from the end)
        let last_user_msg = messages.iter().rev().find(|msg| {
            msg.get("role").and_then(|r| r.as_str()) == Some("user")
        });

        if let Some(msg) = last_user_msg {
            // Handle array content: [{"type": "text", "text": "..."}]
            if let Some(content_arr) = msg.get("content").and_then(|c| c.as_array()) {
                for block in content_arr {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        if let Some(caps) = MARKER_EXTRACT_RE.captures(text) {
                            found.push(caps[1].to_string());
                        }
                    }
                }
            }
            // Handle string content: "content": "text..."
            else if let Some(text) = msg.get("content").and_then(|c| c.as_str()) {
                if let Some(caps) = MARKER_EXTRACT_RE.captures(text) {
                    found.push(caps[1].to_string());
                }
            }
        }
    }

    if found.len() > 1 {
        tracing::warn!(
            "Multiple LUNAROUTE markers found: {:?}. Using first: {}",
            found, found[0]
        );
    }

    match found.into_iter().next() {
        Some(name) if name.eq_ignore_ascii_case("clear") => MarkerResult::Clear,
        Some(name) => MarkerResult::Provider(name),
        None => MarkerResult::None,
    }
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p lunaroute-ingress marker`
Expected: All 6 tests PASS

- [ ] **Step 7: Commit**

```bash
git add crates/lunaroute-ingress/src/marker.rs crates/lunaroute-ingress/src/lib.rs crates/lunaroute-ingress/Cargo.toml
git commit -m "feat: add LUNAROUTE marker extraction (extract_marker)"
```

---

### Task 2: Marker Stripping — `strip_marker()`

**Files:**
- Modify: `crates/lunaroute-ingress/src/marker.rs`

- [ ] **Step 1: Write failing tests for `strip_marker`**

Add to `crates/lunaroute-ingress/src/marker.rs`, after `extract_marker`:

```rust
/// Regex matching a standalone system-reminder block containing only a LUNAROUTE marker
static STANDALONE_STRIP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)^\s*<system-reminder>\s*\[LUNAROUTE:[a-zA-Z0-9._-]+\]\s*</system-reminder>\s*$")
        .unwrap()
});

/// Regex matching just the marker text (for inline stripping)
static INLINE_STRIP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[LUNAROUTE:[a-zA-Z0-9._-]+\]").unwrap()
});

/// Remove [LUNAROUTE:xxx] marker text from the request body.
/// Operates on the LAST user message only (matching extract_marker's scope).
/// - Standalone content block (entire text is system-reminder with marker): remove block.
/// - Inline in a larger text block: regex-replace the marker text.
/// - String content: regex-replace within the string.
/// After stripping, removes empty text blocks and messages with empty content arrays.
pub fn strip_marker(req: &mut serde_json::Value) {
    todo!()
}
```

Add these tests to the `mod tests` block:

```rust
    #[test]
    fn test_strip_standalone_system_reminder_block() {
        let mut req = json!({
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "hello world"},
                    {"type": "text", "text": "<system-reminder>\n[LUNAROUTE:sonnet]\n</system-reminder>"}
                ]
            }]
        });
        strip_marker(&mut req);
        let content = req["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["text"], "hello world");
    }

    #[test]
    fn test_strip_inline_marker_from_text() {
        let mut req = json!({
            "messages": [{
                "role": "user",
                "content": "rewrite this [LUNAROUTE:sonnet] using streams"
            }]
        });
        strip_marker(&mut req);
        assert_eq!(req["messages"][0]["content"], "rewrite this  using streams");
    }

    #[test]
    fn test_strip_inline_marker_from_content_block() {
        let mut req = json!({
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "rewrite [LUNAROUTE:sonnet] this"}
                ]
            }]
        });
        strip_marker(&mut req);
        let content = req["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["text"], "rewrite  this");
    }

    #[test]
    fn test_strip_removes_empty_blocks_and_messages() {
        let mut req = json!({
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "<system-reminder>\n[LUNAROUTE:sonnet]\n</system-reminder>"}
                ]
            }]
        });
        strip_marker(&mut req);
        // Content array should be empty after block removal, message should be removed
        let messages = req["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 0);
    }

    #[test]
    fn test_strip_no_marker_is_noop() {
        let mut req = json!({
            "messages": [{
                "role": "user",
                "content": [{"type": "text", "text": "hello world"}]
            }]
        });
        let original = req.clone();
        strip_marker(&mut req);
        assert_eq!(req, original);
    }
```

- [ ] **Step 2: Run tests to verify new tests fail**

Run: `cargo test -p lunaroute-ingress marker`
Expected: New strip tests FAIL, extract tests still PASS

- [ ] **Step 3: Implement `strip_marker`**

Replace the `todo!()` body:

```rust
pub fn strip_marker(req: &mut serde_json::Value) {
    let Some(messages) = req.get_mut("messages").and_then(|m| m.as_array_mut()) else {
        return;
    };

    // Find the index of the last user message
    let last_user_idx = messages.iter().rposition(|msg| {
        msg.get("role").and_then(|r| r.as_str()) == Some("user")
    });

    let Some(idx) = last_user_idx else { return };
    let msg = &mut messages[idx];

    // Handle array content
    if let Some(content_arr) = msg.get_mut("content").and_then(|c| c.as_array_mut()) {
        content_arr.retain(|block| {
            if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                // If the entire block is a standalone system-reminder with marker, remove it
                if STANDALONE_STRIP_RE.is_match(text) {
                    return false;
                }
            }
            true
        });

        // For remaining blocks, strip inline markers
        for block in content_arr.iter_mut() {
            if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                let stripped = INLINE_STRIP_RE.replace_all(text, "").to_string();
                if stripped != text {
                    block["text"] = serde_json::Value::String(stripped);
                }
            }
        }

        // Remove blocks that became empty
        content_arr.retain(|block| {
            block.get("text").and_then(|t| t.as_str()).map_or(true, |t| !t.trim().is_empty())
        });
    }
    // Handle string content
    else if let Some(text) = msg.get("content").and_then(|c| c.as_str()) {
        let stripped = INLINE_STRIP_RE.replace_all(text, "").to_string();
        if stripped != text {
            msg["content"] = serde_json::Value::String(stripped);
        }
    }

    // Remove the message if content array is now empty
    if let Some(arr) = messages[idx].get("content").and_then(|c| c.as_array()) {
        if arr.is_empty() {
            messages.remove(idx);
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p lunaroute-ingress marker`
Expected: All 11 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/lunaroute-ingress/src/marker.rs
git commit -m "feat: add LUNAROUTE marker stripping (strip_marker)"
```

---

### Task 3: Config Changes — `ProviderSettings` and `ProvidersConfig`

**Files:**
- Modify: `crates/lunaroute-server/src/config.rs:57-90` (ProvidersConfig and Default), `92-122` (ProviderSettings), `501-524` (merge_env ProviderSettings literals)

- [ ] **Step 1: Add `model` and `provider_type` fields to `ProviderSettings`**

In `crates/lunaroute-server/src/config.rs`, add to the `ProviderSettings` struct (after `codex_auth` field, around line 121):

```rust
    /// Provider dialect type (e.g., "openai" or "anthropic").
    /// Required for extra providers. Inferred for built-in "openai" and "anthropic" keys.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,

    /// Model ID override. When targeted via LUNAROUTE marker,
    /// the request body's model field is rewritten to this value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
```

- [ ] **Step 2: Add `extra` field to `ProvidersConfig`**

In `crates/lunaroute-server/src/config.rs`, add to `ProvidersConfig` struct (around line 60):

```rust
    /// Additional named providers for marker-based routing
    #[serde(default, flatten)]
    pub extra: std::collections::HashMap<String, ProviderSettings>,
```

- [ ] **Step 3: Update `Default` impl for `ProvidersConfig`**

Update the `Default` impl (around line 63) to include `extra`:
```rust
impl Default for ProvidersConfig {
    fn default() -> Self {
        Self {
            openai: Some(ProviderSettings {
                api_key: None,
                base_url: None,
                enabled: true,
                http_client: None,
                request_headers: None,
                request_body: None,
                response_body: None,
                codex_auth: None,
                provider_type: None,
                model: None,
            }),
            anthropic: Some(ProviderSettings {
                api_key: None,
                base_url: None,
                enabled: true,
                http_client: None,
                request_headers: None,
                request_body: None,
                response_body: None,
                codex_auth: None,
                provider_type: None,
                model: None,
            }),
            extra: std::collections::HashMap::new(),
        }
    }
}
```

- [ ] **Step 4: Update all `ProviderSettings` literals in `merge_env()` and tests**

In `merge_env()`, update the two `ProviderSettings` struct literals at lines ~501 and ~515 to include the new fields:
```rust
provider_type: None,
model: None,
```

Also update any `ProviderSettings` literals in test functions throughout `config.rs` and `main.rs` — the compiler will flag all of them. Add `provider_type: None, model: None` to each.

- [ ] **Step 5: Run build to verify compilation**

Run: `cargo build -p lunaroute-server`
Expected: Compiles with no errors. If there are other `ProviderSettings` literals elsewhere that need updating, the compiler will show them.

- [ ] **Step 6: Add config validation for extra providers**

Add a validation method to `ProvidersConfig` in `crates/lunaroute-server/src/config.rs`:

```rust
impl ProvidersConfig {
    /// Validate extra provider entries
    pub fn validate_extra_providers(&self) -> Result<(), String> {
        for (name, settings) in &self.extra {
            if name == "openai" || name == "anthropic" {
                return Err(format!(
                    "Extra provider '{}' conflicts with built-in provider name",
                    name
                ));
            }
            if settings.provider_type.is_none() {
                return Err(format!(
                    "Extra provider '{}' requires a 'provider_type' field (\"openai\" or \"anthropic\")",
                    name
                ));
            }
            match settings.provider_type.as_deref() {
                Some("openai") | Some("anthropic") => {}
                Some(other) => {
                    return Err(format!(
                        "Extra provider '{}' has invalid provider_type '{}' (must be \"openai\" or \"anthropic\")",
                        name, other
                    ));
                }
                None => unreachable!(), // Checked above
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 7: Write tests for config validation**

Add to the test module in `crates/lunaroute-server/src/config.rs` (or create if needed):

```rust
#[cfg(test)]
mod provider_config_tests {
    use super::*;

    #[test]
    fn test_extra_provider_valid() {
        let config = ProvidersConfig {
            extra: [("sonnet".to_string(), ProviderSettings {
                api_key: Some("key".to_string()),
                base_url: None,
                enabled: true,
                http_client: None,
                request_headers: None,
                request_body: None,
                response_body: None,
                codex_auth: None,
                provider_type: Some("anthropic".to_string()),
                model: Some("claude-sonnet-4-20250514".to_string()),
            })].into_iter().collect(),
            ..Default::default()
        };
        assert!(config.validate_extra_providers().is_ok());
    }

    #[test]
    fn test_extra_provider_missing_type() {
        let config = ProvidersConfig {
            extra: [("sonnet".to_string(), ProviderSettings {
                api_key: Some("key".to_string()),
                base_url: None,
                enabled: true,
                http_client: None,
                request_headers: None,
                request_body: None,
                response_body: None,
                codex_auth: None,
                provider_type: None,
                model: None,
            })].into_iter().collect(),
            ..Default::default()
        };
        assert!(config.validate_extra_providers().is_err());
    }

    #[test]
    fn test_extra_provider_conflicts_with_builtin() {
        let config = ProvidersConfig {
            extra: [("openai".to_string(), ProviderSettings {
                api_key: Some("key".to_string()),
                base_url: None,
                enabled: true,
                http_client: None,
                request_headers: None,
                request_body: None,
                response_body: None,
                codex_auth: None,
                provider_type: Some("openai".to_string()),
                model: None,
            })].into_iter().collect(),
            ..Default::default()
        };
        assert!(config.validate_extra_providers().is_err());
    }

    #[test]
    fn test_yaml_deserialization_with_extra_providers() {
        let yaml = r#"
openai:
  enabled: true
  api_key: "sk-test"
anthropic:
  enabled: true
  api_key: "sk-ant-test"
sonnet:
  enabled: true
  provider_type: "anthropic"
  api_key: "sk-ant-test"
  model: "claude-sonnet-4-20250514"
"#;
        let config: ProvidersConfig = serde_yaml::from_str(yaml).expect("should deserialize");
        assert!(config.openai.is_some());
        assert!(config.anthropic.is_some());
        assert!(config.extra.contains_key("sonnet"));
        let sonnet = &config.extra["sonnet"];
        assert_eq!(sonnet.provider_type.as_deref(), Some("anthropic"));
        assert_eq!(sonnet.model.as_deref(), Some("claude-sonnet-4-20250514"));
    }
}
```

- [ ] **Step 8: Run tests**

Run: `cargo test -p lunaroute-server provider_config_tests`
Expected: All 3 tests PASS

- [ ] **Step 9: Commit**

```bash
git add crates/lunaroute-server/src/config.rs
git commit -m "feat: add provider_type, model fields and extra providers to config"
```

---

### Task 4: Provider Registry Type

**Files:**
- Create: `crates/lunaroute-ingress/src/provider_registry.rs`
- Modify: `crates/lunaroute-ingress/src/lib.rs`

- [ ] **Step 1: Create the `ProviderRegistry` type**

Create `crates/lunaroute-ingress/src/provider_registry.rs`:

```rust
use lunaroute_egress::{anthropic::AnthropicConnector, openai::OpenAIConnector};
use std::collections::HashMap;
use std::sync::Arc;

/// Which dialect a provider speaks
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderType {
    OpenAI,
    Anthropic,
}

/// A named provider entry in the registry
#[derive(Debug, Clone)]
pub struct ProviderEntry {
    pub connector_type: ProviderType,
    pub openai_connector: Option<Arc<OpenAIConnector>>,
    pub anthropic_connector: Option<Arc<AnthropicConnector>>,
    pub model_override: Option<String>,
}

/// Registry of all named providers, built at startup from config
pub type ProviderRegistry = HashMap<String, ProviderEntry>;
```

- [ ] **Step 2: Add `pub mod provider_registry;` to lib.rs**

Add to `crates/lunaroute-ingress/src/lib.rs`:
```rust
pub mod provider_registry;
```

And add to the `pub use` block:
```rust
pub use provider_registry::{ProviderEntry, ProviderRegistry, ProviderType};
```

- [ ] **Step 3: Verify compilation**

Run: `cargo build -p lunaroute-ingress`
Expected: Compiles

- [ ] **Step 4: Commit**

```bash
git add crates/lunaroute-ingress/src/provider_registry.rs crates/lunaroute-ingress/src/lib.rs
git commit -m "feat: add ProviderRegistry type for marker-based routing"
```

---

### Task 5: Build Provider Registry at Startup

**Files:**
- Modify: `crates/lunaroute-server/src/main.rs:740-840` (connector construction area), `1042-1100` (passthrough router construction)

- [ ] **Step 1: Build ProviderRegistry after connector construction**

In `crates/lunaroute-server/src/main.rs`, after the built-in providers are constructed (after the Anthropic provider block, around line 850), add code to build the registry:

```rust
    // Build ProviderRegistry for marker-based routing
    let mut provider_registry = lunaroute_ingress::ProviderRegistry::new();

    // Add built-in providers
    if let Some(ref connector) = openai_connector {
        provider_registry.insert("openai".to_string(), lunaroute_ingress::ProviderEntry {
            connector_type: lunaroute_ingress::ProviderType::OpenAI,
            openai_connector: Some(connector.clone()),
            anthropic_connector: None,
            model_override: config.providers.openai.as_ref().and_then(|p| p.model.clone()),
        });
    }
    if let Some(ref connector) = anthropic_connector {
        provider_registry.insert("anthropic".to_string(), lunaroute_ingress::ProviderEntry {
            connector_type: lunaroute_ingress::ProviderType::Anthropic,
            openai_connector: None,
            anthropic_connector: Some(connector.clone()),
            model_override: config.providers.anthropic.as_ref().and_then(|p| p.model.clone()),
        });
    }

    // Validate and build extra providers
    config.providers.validate_extra_providers()
        .map_err(|e| anyhow::anyhow!("Invalid provider config: {}", e))?;

    for (name, settings) in &config.providers.extra {
        if !settings.enabled {
            info!("  Extra provider '{}': disabled, skipping", name);
            continue;
        }

        let provider_type_str = settings.provider_type.as_deref().unwrap(); // validated above
        let api_key = settings.api_key.clone().unwrap_or_default();

        match provider_type_str {
            "anthropic" => {
                let base_url = settings.base_url.clone()
                    .unwrap_or_else(|| "https://api.anthropic.com".to_string());
                let client_config = settings.http_client.as_ref()
                    .map(|c| c.to_http_client_config())
                    .unwrap_or_default();
                let connector_config = lunaroute_egress::anthropic::AnthropicConfig {
                    api_key,
                    base_url,
                    api_version: "2023-06-01".to_string(),
                    client_config,
                    switch_notification_message: None,
                };
                let conn = lunaroute_egress::anthropic::AnthropicConnector::new(connector_config)?;
                info!("  Extra provider '{}': anthropic, model_override={:?}", name, settings.model);
                provider_registry.insert(name.clone(), lunaroute_ingress::ProviderEntry {
                    connector_type: lunaroute_ingress::ProviderType::Anthropic,
                    openai_connector: None,
                    anthropic_connector: Some(Arc::new(conn)),
                    model_override: settings.model.clone(),
                });
            }
            "openai" => {
                let base_url = settings.base_url.clone()
                    .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
                let client_config = settings.http_client.as_ref()
                    .map(|c| c.to_http_client_config())
                    .unwrap_or_default();
                let mut connector_config = lunaroute_egress::openai::OpenAIConfig {
                    api_key,
                    base_url,
                    organization: None,
                    client_config,
                    custom_headers: None,
                    request_body_config: None,
                    response_body_config: None,
                    codex_auth: None,
                    switch_notification_message: None,
                };
                if let Some(headers_config) = &settings.request_headers {
                    connector_config.custom_headers = Some(headers_config.headers.clone());
                }
                let conn = lunaroute_egress::openai::OpenAIConnector::new(connector_config).await?;
                info!("  Extra provider '{}': openai, model_override={:?}", name, settings.model);
                provider_registry.insert(name.clone(), lunaroute_ingress::ProviderEntry {
                    connector_type: lunaroute_ingress::ProviderType::OpenAI,
                    openai_connector: Some(Arc::new(conn)),
                    anthropic_connector: None,
                    model_override: settings.model.clone(),
                });
            }
            _ => unreachable!(), // validated above
        }
    }

    let provider_registry = Arc::new(provider_registry);
    if !provider_registry.is_empty() {
        info!("📋 Provider registry: {} providers ({} for marker routing)",
            provider_registry.len(),
            config.providers.extra.len()
        );
    }
```

- [ ] **Step 2: Verify build**

Run: `cargo build -p lunaroute-server`
Expected: Compiles. The `provider_registry` variable is defined but not yet passed to routers — that's the next task.

- [ ] **Step 3: Commit**

```bash
git add crates/lunaroute-server/src/main.rs
git commit -m "feat: build ProviderRegistry from config at startup"
```

---

### Task 6: Thread Provider Registry into Passthrough Handlers

**Files:**
- Modify: `crates/lunaroute-ingress/src/anthropic.rs:862-870` (PassthroughState), `1775-1797` (passthrough_router fn)
- Modify: `crates/lunaroute-ingress/src/openai.rs:1920-1928` (OpenAIPassthroughState), `1931-1958` (passthrough_router fn)
- Modify: `crates/lunaroute-ingress/src/multi_dialect.rs:54-92` (passthrough_router fn)
- Modify: `crates/lunaroute-server/src/main.rs:1042-1100` (router construction call sites)

- [ ] **Step 1: Add `provider_registry` to `PassthroughState` (anthropic.rs)**

In `crates/lunaroute-ingress/src/anthropic.rs`, add to `PassthroughState` struct (around line 868):
```rust
    pub provider_registry: Option<Arc<crate::ProviderRegistry>>,
```

Update `passthrough_router` function signature (line 1775) to accept the registry:
```rust
pub fn passthrough_router(
    connector: Arc<lunaroute_egress::anthropic::AnthropicConnector>,
    stats_tracker: Option<Arc<dyn crate::types::SessionStatsTracker>>,
    metrics: Option<Arc<lunaroute_observability::Metrics>>,
    session_store: Option<Arc<dyn SessionStore>>,
    sse_keepalive_interval_secs: u64,
    sse_keepalive_enabled: bool,
    provider_registry: Option<Arc<crate::ProviderRegistry>>,
) -> Router {
```

Add `provider_registry` to the `PassthroughState` construction (line 1783):
```rust
    let state = Arc::new(PassthroughState {
        connector,
        stats_tracker,
        metrics,
        session_store,
        tool_call_mapper: Arc::new(lunaroute_session::ToolCallMapper::new()),
        sse_keepalive_interval_secs,
        sse_keepalive_enabled,
        provider_registry,
    });
```

- [ ] **Step 2: Add `provider_registry` to `OpenAIPassthroughState` (openai.rs)**

Same pattern in `crates/lunaroute-ingress/src/openai.rs`:

Add to `OpenAIPassthroughState` (line 1928):
```rust
    pub provider_registry: Option<Arc<crate::ProviderRegistry>>,
```

Update `passthrough_router` signature (line 1931) to accept the registry and pass it into state construction.

- [ ] **Step 3: Update `multi_dialect::passthrough_router`**

In `crates/lunaroute-ingress/src/multi_dialect.rs`, update the function signature (line 54) to accept `provider_registry: Option<Arc<crate::ProviderRegistry>>` and pass it through to both inner `passthrough_router` calls.

- [ ] **Step 4: Update call sites in main.rs**

In `crates/lunaroute-server/src/main.rs`, update all three `passthrough_router` call sites (around lines 1047, 1064, 1092) to pass `Some(provider_registry.clone())` as the last argument.

- [ ] **Step 5: Verify build**

Run: `cargo build -p lunaroute-server`
Expected: Compiles. The registry is threaded through but not yet used in handlers.

- [ ] **Step 6: Commit**

```bash
git add crates/lunaroute-ingress/src/anthropic.rs crates/lunaroute-ingress/src/openai.rs crates/lunaroute-ingress/src/multi_dialect.rs crates/lunaroute-server/src/main.rs
git commit -m "feat: thread ProviderRegistry into passthrough handlers"
```

---

### Task 7: Integrate Marker Logic into Anthropic Passthrough Handler

**Files:**
- Modify: `crates/lunaroute-ingress/src/anthropic.rs:875-1440` (messages_passthrough)

This is the core integration. The marker extraction, provider lookup, model override, and connector swap all happen here.

- [ ] **Step 1: Add marker detection early in `messages_passthrough`**

In `crates/lunaroute-ingress/src/anthropic.rs`, in `messages_passthrough` (starts at line 875), add after `let start_time` and header processing but before model extraction. Make `req` mutable and add marker handling:

After `Json(req): Json<serde_json::Value>` extraction, add:
```rust
    let mut req = req;

    // LUNAROUTE marker detection — check for provider override
    let marker_result = crate::marker::extract_marker(&req);
    let mut override_connector: Option<Arc<lunaroute_egress::anthropic::AnthropicConnector>> = None;
    let mut marker_provider_name: Option<String> = None;

    match &marker_result {
        crate::marker::MarkerResult::Provider(name) => {
            if let Some(registry) = &state.provider_registry {
                if let Some(entry) = registry.get(name) {
                    if entry.connector_type != crate::ProviderType::Anthropic {
                        tracing::warn!("LUNAROUTE marker '{}' targets {:?} provider but request uses Anthropic format", name, entry.connector_type);
                        return Err(IngressError::InvalidRequest(format!(
                            "LUNAROUTE marker targets provider '{}' ({:?}) but request uses Anthropic format. Cross-dialect routing requires normalized mode.",
                            name, entry.connector_type
                        )));
                    }
                    if let Some(ref connector) = entry.anthropic_connector {
                        tracing::info!("LUNAROUTE marker: routing to provider '{}', model_override={:?}", name, entry.model_override);
                        override_connector = Some(connector.clone());
                        marker_provider_name = Some(name.clone());

                        // Apply model override
                        if let Some(ref model) = entry.model_override {
                            req["model"] = serde_json::Value::String(model.clone());
                        }
                    }
                } else {
                    tracing::warn!("LUNAROUTE marker references unknown provider '{}', using default", name);
                }
            }
            crate::marker::strip_marker(&mut req);
        }
        crate::marker::MarkerResult::Clear => {
            crate::marker::strip_marker(&mut req);
        }
        crate::marker::MarkerResult::None => {}
    }
```

- [ ] **Step 2: Update connector references in the handler**

Find the two places where `state.connector` is used for sending requests:

1. **Streaming** (around line 1161): `state.connector.stream_passthrough(req, passthrough_headers)`
2. **Non-streaming** (around line 1437): `state.connector.send_passthrough(req, passthrough_headers)`

Replace each with:
```rust
    let connector = override_connector.as_ref().unwrap_or(&state.connector);
    connector.stream_passthrough(req, passthrough_headers)
```
and:
```rust
    let connector = override_connector.as_ref().unwrap_or(&state.connector);
    connector.send_passthrough(req, passthrough_headers)
```

- [ ] **Step 3: Add marker info to session tags**

In the `SessionEvent::Started` block (around line 1015-1040), update `session_tags`:

```rust
    let mut session_tags = vec![];
    if let Some(ref provider_name) = marker_provider_name {
        session_tags.push(format!("lunaroute:{}", provider_name));
        if let Some(registry) = &state.provider_registry {
            if let Some(entry) = registry.get(provider_name) {
                if let Some(ref model) = entry.model_override {
                    session_tags.push(format!("model_override:{}", model));
                }
            }
        }
    }
```

Then use `session_tags` instead of `vec![]` in the `V2Metadata` struct.

- [ ] **Step 4: Verify build**

Run: `cargo build -p lunaroute-server`
Expected: Compiles

- [ ] **Step 5: Commit**

```bash
git add crates/lunaroute-ingress/src/anthropic.rs
git commit -m "feat: integrate LUNAROUTE marker routing into Anthropic passthrough handler"
```

---

### Task 8: Integrate Marker Logic into OpenAI Passthrough Handler

**Files:**
- Modify: `crates/lunaroute-ingress/src/openai.rs:1962-2340` (chat_completions_passthrough)

Same pattern as Task 7 but for the OpenAI handler. Key differences:
- Uses `OpenAIConnector` instead of `AnthropicConnector`
- Checks for `ProviderType::OpenAI`
- `send_passthrough` and `stream_passthrough` have different return types than Anthropic

- [ ] **Step 1: Add marker detection early in `chat_completions_passthrough`**

Same pattern as Task 7 Step 1, but with OpenAI types:
```rust
    let mut req = req;

    let marker_result = crate::marker::extract_marker(&req);
    let mut override_connector: Option<Arc<lunaroute_egress::openai::OpenAIConnector>> = None;
    let mut marker_provider_name: Option<String> = None;

    match &marker_result {
        crate::marker::MarkerResult::Provider(name) => {
            if let Some(registry) = &state.provider_registry {
                if let Some(entry) = registry.get(name) {
                    if entry.connector_type != crate::ProviderType::OpenAI {
                        tracing::warn!("LUNAROUTE marker '{}' targets {:?} provider but request uses OpenAI format", name, entry.connector_type);
                        return Err(IngressError::InvalidRequest(format!(
                            "LUNAROUTE marker targets provider '{}' ({:?}) but request uses OpenAI format. Cross-dialect routing requires normalized mode.",
                            name, entry.connector_type
                        )));
                    }
                    if let Some(ref connector) = entry.openai_connector {
                        tracing::info!("LUNAROUTE marker: routing to provider '{}', model_override={:?}", name, entry.model_override);
                        override_connector = Some(connector.clone());
                        marker_provider_name = Some(name.clone());
                        if let Some(ref model) = entry.model_override {
                            req["model"] = serde_json::Value::String(model.clone());
                        }
                    }
                } else {
                    tracing::warn!("LUNAROUTE marker references unknown provider '{}', using default", name);
                }
            }
            crate::marker::strip_marker(&mut req);
        }
        crate::marker::MarkerResult::Clear => {
            crate::marker::strip_marker(&mut req);
        }
        crate::marker::MarkerResult::None => {}
    }
```

- [ ] **Step 2: Update connector references**

Find `state.connector` usages for streaming and non-streaming sends. Replace with:
```rust
    let connector = override_connector.as_ref().unwrap_or(&state.connector);
```

- [ ] **Step 3: Add marker info to session tags**

Same pattern as Task 7 Step 3 — update the `session_tags: vec![]` in the `Started` event.

**Note:** The `responses_passthrough` handler also has `session_tags: vec![]` but is out of scope per the spec (`/v1/responses` uses a different body format).

- [ ] **Step 4: Verify build**

Run: `cargo build -p lunaroute-server`
Expected: Compiles

- [ ] **Step 5: Run full test suite**

Run: `cargo test`
Expected: All existing tests pass

- [ ] **Step 6: Commit**

```bash
git add crates/lunaroute-ingress/src/openai.rs
git commit -m "feat: integrate LUNAROUTE marker routing into OpenAI passthrough handler"
```

---

### Task 9: Update Config Example

**Files:**
- Modify: `config.example.yaml`

- [ ] **Step 1: Add marker routing example to config.example.yaml**

Add a new commented section to `config.example.yaml` after the existing routing rules section:

```yaml
# Extra providers for marker-based routing (LUNAROUTE markers)
# Users can type #!sonnet in Claude Code to route to this provider.
# The marker [LUNAROUTE:sonnet] is injected via additionalContext hooks.
#
# providers:
#   anthropic:
#     enabled: true
#     api_key: "${ANTHROPIC_API_KEY}"
#   sonnet:
#     enabled: true
#     provider_type: "anthropic"
#     api_key: "${ANTHROPIC_API_KEY}"
#     model: "claude-sonnet-4-20250514"
#   gpt4o:
#     enabled: true
#     provider_type: "openai"
#     api_key: "${OPENAI_API_KEY}"
#     model: "gpt-4o"
```

- [ ] **Step 2: Commit**

```bash
git add config.example.yaml
git commit -m "docs: add marker routing example to config.example.yaml"
```

---

### Task 10: Integration Test

**Files:**
- Modify: `crates/lunaroute-ingress/src/marker.rs` (add integration-style tests)

- [ ] **Step 1: Add end-to-end marker flow tests**

Add to the test module in `marker.rs` — these test the full extract + strip flow:

```rust
    #[test]
    fn test_full_flow_extract_then_strip() {
        let mut req = json!({
            "model": "claude-opus-4-20250514",
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "rewrite this function"},
                        {"type": "text", "text": "<system-reminder>\n[LUNAROUTE:sonnet]\n</system-reminder>"}
                    ]
                }
            ]
        });

        // Extract
        let result = extract_marker(&req);
        assert_eq!(result, MarkerResult::Provider("sonnet".to_string()));

        // Strip
        strip_marker(&mut req);

        // Verify marker is gone
        assert_eq!(extract_marker(&req), MarkerResult::None);

        // Verify user message is preserved
        let content = req["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["text"], "rewrite this function");
    }

    #[test]
    fn test_full_flow_clear_marker() {
        let mut req = json!({
            "model": "claude-opus-4-20250514",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "hello"},
                    {"type": "text", "text": "<system-reminder>\n[LUNAROUTE:clear]\n</system-reminder>"}
                ]
            }]
        });

        assert_eq!(extract_marker(&req), MarkerResult::Clear);
        strip_marker(&mut req);
        assert_eq!(extract_marker(&req), MarkerResult::None);

        let content = req["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["text"], "hello");
    }

    #[test]
    fn test_model_override_applied() {
        let mut req = json!({
            "model": "claude-opus-4-20250514",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "hello"},
                    {"type": "text", "text": "<system-reminder>\n[LUNAROUTE:sonnet]\n</system-reminder>"}
                ]
            }]
        });

        // Simulate what the handler does: extract, override model, strip
        if let MarkerResult::Provider(_) = extract_marker(&req) {
            req["model"] = serde_json::Value::String("claude-sonnet-4-20250514".to_string());
        }
        strip_marker(&mut req);

        assert_eq!(req["model"], "claude-sonnet-4-20250514");
        assert_eq!(extract_marker(&req), MarkerResult::None);
    }
```

- [ ] **Step 2: Run all tests**

Run: `cargo test -p lunaroute-ingress marker`
Expected: All tests PASS (should be 14 total)

- [ ] **Step 3: Run full test suite**

Run: `cargo test`
Expected: All tests PASS

- [ ] **Step 4: Commit**

```bash
git add crates/lunaroute-ingress/src/marker.rs
git commit -m "test: add integration-style tests for LUNAROUTE marker flow"
```
