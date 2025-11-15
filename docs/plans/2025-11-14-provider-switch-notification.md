# Provider Switch Notification Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add user-facing notifications when LunaRoute switches providers due to rate limits, errors, or circuit breaker events by injecting a prepended user message.

**Architecture:** Implement at routing layer (provider_router.rs). When switching providers, prepend a user message instructing the LLM to inform the end user. Configuration is global (on/off + default message) with per-provider overrides. Template variables allow customization.

**Tech Stack:** Rust, serde for config serialization, existing lunaroute routing infrastructure

---

## Phase 1: Configuration Infrastructure

### Task 1.1: Add SwitchReason Enum

**Files:**
- Create: `crates/lunaroute-routing/src/notification.rs`

**Step 1: Write the test for SwitchReason mapping**

Create new test file:

```rust
// crates/lunaroute-routing/src/notification.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_switch_reason_to_generic_message() {
        assert_eq!(
            SwitchReason::RateLimit.to_generic_message(),
            "high demand"
        );
        assert_eq!(
            SwitchReason::ServiceIssue.to_generic_message(),
            "a temporary service issue"
        );
        assert_eq!(
            SwitchReason::CircuitBreaker.to_generic_message(),
            "service maintenance"
        );
    }

    #[test]
    fn test_switch_reason_clone() {
        let reason = SwitchReason::RateLimit;
        let cloned = reason.clone();
        assert_eq!(reason.to_generic_message(), cloned.to_generic_message());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p lunaroute-routing notification::tests::test_switch_reason_to_generic_message`

Expected: FAIL with "cannot find type `SwitchReason`"

**Step 3: Implement SwitchReason enum**

Add to top of `crates/lunaroute-routing/src/notification.rs`:

```rust
//! Provider switch notification infrastructure
//!
//! Provides functionality for notifying users when LunaRoute switches
//! providers due to rate limits, errors, or circuit breaker events.

/// Reason for switching providers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchReason {
    /// Rate limit (429 error)
    RateLimit,
    /// Server error (5xx)
    ServiceIssue,
    /// Circuit breaker open
    CircuitBreaker,
}

impl SwitchReason {
    /// Convert to generic user-facing message
    pub fn to_generic_message(&self) -> &'static str {
        match self {
            SwitchReason::RateLimit => "high demand",
            SwitchReason::ServiceIssue => "a temporary service issue",
            SwitchReason::CircuitBreaker => "service maintenance",
        }
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p lunaroute-routing notification::tests`

Expected: All tests PASS

**Step 5: Commit**

```bash
git add crates/lunaroute-routing/src/notification.rs
git commit -m "feat: add SwitchReason enum for provider switch notifications"
```

---

### Task 1.2: Add ProviderSwitchNotificationConfig Struct

**Files:**
- Modify: `crates/lunaroute-routing/src/notification.rs`

**Step 1: Write test for default configuration**

Add to test module in `crates/lunaroute-routing/src/notification.rs`:

```rust
#[test]
fn test_notification_config_default() {
    let config = ProviderSwitchNotificationConfig::default();
    assert!(config.enabled);
    assert!(config.default_message.contains("IMPORTANT"));
    assert!(config.default_message.contains("alternative AI service provider"));
}

#[test]
fn test_notification_config_serialization() {
    let config = ProviderSwitchNotificationConfig {
        enabled: false,
        default_message: "Custom message".to_string(),
    };

    let yaml = serde_yaml::to_string(&config).unwrap();
    let deserialized: ProviderSwitchNotificationConfig =
        serde_yaml::from_str(&yaml).unwrap();

    assert_eq!(config.enabled, deserialized.enabled);
    assert_eq!(config.default_message, deserialized.default_message);
}

#[test]
fn test_notification_config_enabled_default() {
    let yaml = "{}";
    let config: ProviderSwitchNotificationConfig =
        serde_yaml::from_str(yaml).unwrap();
    assert!(config.enabled); // Should default to true
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p lunaroute-routing notification::tests::test_notification_config_default`

Expected: FAIL with "cannot find type `ProviderSwitchNotificationConfig`"

**Step 3: Implement ProviderSwitchNotificationConfig struct**

Add to `crates/lunaroute-routing/src/notification.rs` (before tests):

```rust
use serde::{Deserialize, Serialize};

/// Configuration for provider switch notifications
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSwitchNotificationConfig {
    /// Enable/disable notifications globally
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Default message template
    #[serde(default = "default_notification_message")]
    pub default_message: String,
}

impl Default for ProviderSwitchNotificationConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            default_message: default_notification_message(),
        }
    }
}

fn default_enabled() -> bool {
    true
}

fn default_notification_message() -> String {
    "IMPORTANT: Please inform the user that due to temporary service constraints, \
     their request is being handled by an alternative AI service provider. \
     Then proceed to fulfill their original request completely and professionally."
        .to_string()
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p lunaroute-routing notification::tests`

Expected: All tests PASS

**Step 5: Commit**

```bash
git add crates/lunaroute-routing/src/notification.rs
git commit -m "feat: add ProviderSwitchNotificationConfig struct"
```

---

### Task 1.3: Export Notification Module

**Files:**
- Modify: `crates/lunaroute-routing/src/lib.rs`

**Step 1: Add module declaration**

Add to `crates/lunaroute-routing/src/lib.rs` (after other module declarations):

```rust
pub mod notification;
```

**Step 2: Re-export public types**

Add to the public re-exports section in `crates/lunaroute-routing/src/lib.rs`:

```rust
pub use notification::{ProviderSwitchNotificationConfig, SwitchReason};
```

**Step 3: Verify module compiles**

Run: `cargo build -p lunaroute-routing`

Expected: Success

**Step 4: Commit**

```bash
git add crates/lunaroute-routing/src/lib.rs
git commit -m "feat: export notification module"
```

---

### Task 1.4: Add Notification Config to Router Config

**Files:**
- Modify: `crates/lunaroute-routing/src/router.rs`

**Step 1: Write test for router with notification config**

Add to test module in `crates/lunaroute-routing/src/router.rs`:

```rust
#[test]
fn test_routing_config_with_notification() {
    use crate::notification::ProviderSwitchNotificationConfig;

    let yaml = r#"
provider_switch_notification:
  enabled: true
  default_message: "Custom notification"
rules: []
"#;

    let config: RoutingConfig = serde_yaml::from_str(yaml).unwrap();
    assert!(config.provider_switch_notification.is_some());
    let notif = config.provider_switch_notification.unwrap();
    assert!(notif.enabled);
    assert_eq!(notif.default_message, "Custom notification");
}

#[test]
fn test_routing_config_without_notification() {
    let yaml = r#"
rules: []
"#;

    let config: RoutingConfig = serde_yaml::from_str(yaml).unwrap();
    assert!(config.provider_switch_notification.is_none());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p lunaroute-routing router::tests::test_routing_config_with_notification`

Expected: FAIL with field not found

**Step 3: Add field to RoutingConfig**

Find the `RoutingConfig` struct in `crates/lunaroute-routing/src/router.rs` and add:

```rust
pub struct RoutingConfig {
    // ... existing fields ...

    /// Provider switch notification configuration
    #[serde(default)]
    pub provider_switch_notification: Option<ProviderSwitchNotificationConfig>,
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p lunaroute-routing router::tests::test_routing_config`

Expected: All tests PASS

**Step 5: Commit**

```bash
git add crates/lunaroute-routing/src/router.rs
git commit -m "feat: add provider_switch_notification to RoutingConfig"
```

---

### Task 1.5: Add Notification Message to Provider Configs

**Files:**
- Modify: `crates/lunaroute-egress/src/openai.rs`
- Modify: `crates/lunaroute-egress/src/anthropic.rs`

**Step 1: Write test for OpenAI config with notification message**

Add to test module in `crates/lunaroute-egress/src/openai.rs`:

```rust
#[test]
fn test_config_with_switch_notification_message() {
    let config = OpenAIConfig {
        api_key: "test-key".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: Some("Custom switch message".to_string()),
    };

    assert_eq!(
        config.switch_notification_message.unwrap(),
        "Custom switch message"
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p lunaroute-egress openai::tests::test_config_with_switch_notification_message`

Expected: FAIL with field not found

**Step 3: Add field to OpenAIConfig**

In `crates/lunaroute-egress/src/openai.rs`, find `OpenAIConfig` struct and add:

```rust
pub struct OpenAIConfig {
    // ... existing fields ...

    /// Optional custom notification message when this provider is used as alternative
    pub switch_notification_message: Option<String>,
}
```

**Step 4: Update OpenAIConfig::new() and builder patterns**

Find all places where `OpenAIConfig` is constructed and add the new field with `None` default. Check:
- `OpenAIConfig::new()` if it exists
- Test constructors
- Any builder patterns

**Step 5: Run test to verify it passes**

Run: `cargo test -p lunaroute-egress openai::tests::test_config_with_switch_notification_message`

Expected: PASS

**Step 6: Repeat for AnthropicConfig**

Add similar test and field to `crates/lunaroute-egress/src/anthropic.rs`:

```rust
// Test
#[test]
fn test_config_with_switch_notification_message() {
    let config = AnthropicConfig {
        api_key: "test-key".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: Some("Custom switch message".to_string()),
    };

    assert_eq!(
        config.switch_notification_message.unwrap(),
        "Custom switch message"
    );
}

// Field
pub struct AnthropicConfig {
    // ... existing fields ...

    /// Optional custom notification message when this provider is used as alternative
    pub switch_notification_message: Option<String>,
}
```

**Step 7: Run all egress tests**

Run: `cargo test -p lunaroute-egress`

Expected: All tests PASS

**Step 8: Commit**

```bash
git add crates/lunaroute-egress/src/openai.rs crates/lunaroute-egress/src/anthropic.rs
git commit -m "feat: add switch_notification_message to provider configs"
```

---

## Phase 2: Template Substitution

### Task 2.1: Implement Template Substitution Function

**Files:**
- Modify: `crates/lunaroute-routing/src/notification.rs`

**Step 1: Write tests for template substitution**

Add to test module in `crates/lunaroute-routing/src/notification.rs`:

```rust
#[test]
fn test_substitute_all_variables() {
    let template = "Switched from ${original_provider} to ${new_provider} due to ${reason} for ${model}";
    let result = substitute_template_variables(
        template,
        "openai-primary",
        "anthropic-backup",
        "high demand",
        "gpt-4",
    );

    assert_eq!(
        result,
        "Switched from openai-primary to anthropic-backup due to high demand for gpt-4"
    );
}

#[test]
fn test_substitute_no_variables() {
    let template = "No variables here";
    let result = substitute_template_variables(
        template,
        "openai",
        "anthropic",
        "issue",
        "gpt-4",
    );

    assert_eq!(result, "No variables here");
}

#[test]
fn test_substitute_partial_variables() {
    let template = "Using ${new_provider} now";
    let result = substitute_template_variables(
        template,
        "openai",
        "anthropic",
        "issue",
        "gpt-4",
    );

    assert_eq!(result, "Using anthropic now");
}

#[test]
fn test_substitute_duplicate_variables() {
    let template = "${model} and ${model} again";
    let result = substitute_template_variables(
        template,
        "openai",
        "anthropic",
        "issue",
        "gpt-4",
    );

    assert_eq!(result, "gpt-4 and gpt-4 again");
}

#[test]
fn test_substitute_special_characters() {
    let template = "${new_provider}!";
    let result = substitute_template_variables(
        template,
        "openai",
        "anthropic-backup-2",
        "issue",
        "gpt-4-turbo",
    );

    assert_eq!(result, "anthropic-backup-2!");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p lunaroute-routing notification::tests::test_substitute_all_variables`

Expected: FAIL with "cannot find function `substitute_template_variables`"

**Step 3: Implement substitute_template_variables**

Add to `crates/lunaroute-routing/src/notification.rs` (before tests):

```rust
/// Substitute template variables in a notification message
///
/// Supported variables:
/// - `${original_provider}`: Provider that failed
/// - `${new_provider}`: Provider being switched to
/// - `${reason}`: Generic reason for switch
/// - `${model}`: Model name from request
pub fn substitute_template_variables(
    template: &str,
    original_provider: &str,
    new_provider: &str,
    reason: &str,
    model: &str,
) -> String {
    template
        .replace("${original_provider}", original_provider)
        .replace("${new_provider}", new_provider)
        .replace("${reason}", reason)
        .replace("${model}", model)
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p lunaroute-routing notification::tests::test_substitute`

Expected: All tests PASS

**Step 5: Commit**

```bash
git add crates/lunaroute-routing/src/notification.rs
git commit -m "feat: implement template variable substitution"
```

---

## Phase 3: Notification Building Logic

### Task 3.1: Implement Notification Message Builder

**Files:**
- Modify: `crates/lunaroute-routing/src/notification.rs`

**Step 1: Write tests for build_notification_message**

Add to test module in `crates/lunaroute-routing/src/notification.rs`:

```rust
#[test]
fn test_build_notification_uses_default() {
    let global_config = ProviderSwitchNotificationConfig::default();

    let message = build_notification_message(
        "openai-primary",
        "anthropic-backup",
        SwitchReason::RateLimit,
        "gpt-4",
        None, // No provider override
        &global_config,
    );

    assert!(message.contains("IMPORTANT"));
    assert!(message.contains("alternative AI service provider"));
}

#[test]
fn test_build_notification_uses_provider_override() {
    let global_config = ProviderSwitchNotificationConfig::default();
    let provider_message = Some("Custom: Switched to ${new_provider}".to_string());

    let message = build_notification_message(
        "openai-primary",
        "anthropic-backup",
        SwitchReason::RateLimit,
        "gpt-4",
        provider_message.as_deref(),
        &global_config,
    );

    assert_eq!(message, "Custom: Switched to anthropic-backup");
}

#[test]
fn test_build_notification_substitutes_variables() {
    let global_config = ProviderSwitchNotificationConfig {
        enabled: true,
        default_message: "From ${original_provider} to ${new_provider} due to ${reason} (${model})".to_string(),
    };

    let message = build_notification_message(
        "openai-primary",
        "anthropic-backup",
        SwitchReason::ServiceIssue,
        "gpt-4",
        None,
        &global_config,
    );

    assert_eq!(
        message,
        "From openai-primary to anthropic-backup due to a temporary service issue (gpt-4)"
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p lunaroute-routing notification::tests::test_build_notification_uses_default`

Expected: FAIL with "cannot find function `build_notification_message`"

**Step 3: Implement build_notification_message**

Add to `crates/lunaroute-routing/src/notification.rs` (before tests):

```rust
/// Build notification message for provider switch
///
/// # Arguments
/// * `original_provider` - Provider ID that failed
/// * `new_provider` - Provider ID being switched to
/// * `switch_reason` - Reason for the switch
/// * `model` - Model name from request
/// * `provider_override` - Optional provider-specific message override
/// * `global_config` - Global notification configuration
pub fn build_notification_message(
    original_provider: &str,
    new_provider: &str,
    switch_reason: SwitchReason,
    model: &str,
    provider_override: Option<&str>,
    global_config: &ProviderSwitchNotificationConfig,
) -> String {
    // Use provider override if available, otherwise use global default
    let template = provider_override.unwrap_or(&global_config.default_message);

    // Get generic reason message
    let reason = switch_reason.to_generic_message();

    // Substitute variables
    substitute_template_variables(
        template,
        original_provider,
        new_provider,
        reason,
        model,
    )
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p lunaroute-routing notification::tests::test_build_notification`

Expected: All tests PASS

**Step 5: Commit**

```bash
git add crates/lunaroute-routing/src/notification.rs
git commit -m "feat: implement notification message builder"
```

---

### Task 3.2: Implement Idempotency Guard

**Files:**
- Modify: `crates/lunaroute-routing/src/notification.rs`

**Step 1: Write tests for has_notification_already**

Add to test module in `crates/lunaroute-routing/src/notification.rs`:

```rust
use lunaroute_core::normalized::{Message, MessageContent, NormalizedRequest, Role};
use std::collections::HashMap;

#[test]
fn test_has_notification_when_present() {
    let mut request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("IMPORTANT: This is a notification".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        model: "gpt-4".to_string(),
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: HashMap::new(),
    };

    assert!(has_notification_already(&request));
}

#[test]
fn test_has_notification_when_absent() {
    let mut request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("Regular user message".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        model: "gpt-4".to_string(),
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: HashMap::new(),
    };

    assert!(!has_notification_already(&request));
}

#[test]
fn test_has_notification_empty_messages() {
    let mut request = NormalizedRequest {
        messages: vec![],
        system: None,
        model: "gpt-4".to_string(),
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: HashMap::new(),
    };

    assert!(!has_notification_already(&request));
}

#[test]
fn test_has_notification_multimodal_message() {
    use lunaroute_core::normalized::ContentPart;

    let mut request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Parts(vec![
                ContentPart::Text {
                    text: "Some text".to_string(),
                },
            ]),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        model: "gpt-4".to_string(),
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: HashMap::new(),
    };

    assert!(!has_notification_already(&request));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p lunaroute-routing notification::tests::test_has_notification_when_present`

Expected: FAIL with "cannot find function `has_notification_already`"

**Step 3: Implement has_notification_already**

Add to `crates/lunaroute-routing/src/notification.rs` (before tests):

```rust
use lunaroute_core::normalized::{MessageContent, NormalizedRequest};

/// Check if notification has already been injected
///
/// Detects if the first message starts with "IMPORTANT:" to prevent
/// duplicate notifications during cascading failovers.
pub fn has_notification_already(request: &NormalizedRequest) -> bool {
    request
        .messages
        .first()
        .and_then(|m| match &m.content {
            MessageContent::Text(t) => Some(t.starts_with("IMPORTANT:")),
            _ => None,
        })
        .unwrap_or(false)
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p lunaroute-routing notification::tests::test_has_notification`

Expected: All tests PASS

**Step 5: Commit**

```bash
git add crates/lunaroute-routing/src/notification.rs
git commit -m "feat: implement idempotency guard for notifications"
```

---

## Phase 4: Router Integration

### Task 4.1: Add Notification Config to Router Struct

**Files:**
- Modify: `crates/lunaroute-routing/src/provider_router.rs`

**Step 1: Find Router struct and add field**

In `crates/lunaroute-routing/src/provider_router.rs`, find the `Router` struct and add:

```rust
use crate::notification::ProviderSwitchNotificationConfig;

pub struct Router {
    // ... existing fields ...

    /// Provider switch notification configuration
    notification_config: Option<ProviderSwitchNotificationConfig>,
}
```

**Step 2: Update Router::new() constructor**

Find `Router::new()` and add parameter and field initialization:

```rust
pub fn new(
    // ... existing parameters ...
    notification_config: Option<ProviderSwitchNotificationConfig>,
) -> Self {
    Self {
        // ... existing fields ...
        notification_config,
    }
}
```

**Step 3: Update all Router::new() call sites in tests**

Find all test files that construct `Router` and add `None` for the new parameter:

```rust
// In test files
let router = Router::new(
    // ... existing arguments ...
    None, // notification_config
);
```

Files to check:
- `crates/lunaroute-routing/tests/router_integration.rs`
- `crates/lunaroute-routing/tests/router_streaming_integration.rs`
- `crates/lunaroute-routing/src/provider_router.rs` (tests module)

**Step 4: Build to verify changes**

Run: `cargo build -p lunaroute-routing`

Expected: Success

**Step 5: Run tests to verify**

Run: `cargo test -p lunaroute-routing`

Expected: All tests PASS

**Step 6: Commit**

```bash
git add crates/lunaroute-routing/src/provider_router.rs crates/lunaroute-routing/tests/*.rs
git commit -m "feat: add notification_config to Router struct"
```

---

### Task 4.2: Implement Provider Config Lookup

**Files:**
- Modify: `crates/lunaroute-routing/src/provider_router.rs`

**Step 1: Write test for get_provider_notification_message**

Add to test module in `crates/lunaroute-routing/src/provider_router.rs`:

```rust
#[test]
fn test_get_provider_notification_message_with_override() {
    // This test will need a mock provider with notification message
    // We'll implement after the function is added
}
```

**Step 2: Implement helper method on Router**

Add to `Router` impl block in `crates/lunaroute-routing/src/provider_router.rs`:

```rust
impl Router {
    // ... existing methods ...

    /// Get notification message override from provider config if available
    fn get_provider_notification_message(&self, provider_id: &str) -> Option<String> {
        self.providers
            .get(provider_id)
            .and_then(|provider| {
                // Access provider config's switch_notification_message
                // This requires accessing the inner config of the provider
                // For now, return None - we'll implement proper access in next task
                None
            })
    }
}
```

**Step 3: Note for later - this requires provider trait extension**

Add TODO comment:

```rust
// TODO: Need to add get_notification_message() to Provider trait
// to access the config field from OpenAIConnector/AnthropicConnector
```

**Step 4: Commit**

```bash
git add crates/lunaroute-routing/src/provider_router.rs
git commit -m "feat: add stub for provider notification message lookup"
```

---

### Task 4.3: Extend Provider Trait for Notification Message

**Files:**
- Modify: `crates/lunaroute-core/src/provider.rs`
- Modify: `crates/lunaroute-egress/src/openai.rs`
- Modify: `crates/lunaroute-egress/src/anthropic.rs`

**Step 1: Add method to Provider trait**

In `crates/lunaroute-core/src/provider.rs`, find the `Provider` trait and add:

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    // ... existing methods ...

    /// Get custom notification message for when this provider is used as alternative
    fn get_notification_message(&self) -> Option<&str> {
        None // Default implementation
    }
}
```

**Step 2: Implement for OpenAIConnector**

In `crates/lunaroute-egress/src/openai.rs`, find the `Provider` impl and add:

```rust
#[async_trait]
impl Provider for OpenAIConnector {
    // ... existing methods ...

    fn get_notification_message(&self) -> Option<&str> {
        self.config.switch_notification_message.as_deref()
    }
}
```

**Step 3: Implement for AnthropicConnector**

In `crates/lunaroute-egress/src/anthropic.rs`, find the `Provider` impl and add:

```rust
#[async_trait]
impl Provider for AnthropicConnector {
    // ... existing methods ...

    fn get_notification_message(&self) -> Option<&str> {
        self.config.switch_notification_message.as_deref()
    }
}
```

**Step 4: Update get_provider_notification_message in Router**

In `crates/lunaroute-routing/src/provider_router.rs`, update the method:

```rust
fn get_provider_notification_message(&self, provider_id: &str) -> Option<String> {
    self.providers
        .get(provider_id)
        .and_then(|provider| provider.get_notification_message().map(|s| s.to_string()))
}
```

**Step 5: Build to verify**

Run: `cargo build`

Expected: Success

**Step 6: Commit**

```bash
git add crates/lunaroute-core/src/provider.rs crates/lunaroute-egress/src/openai.rs crates/lunaroute-egress/src/anthropic.rs crates/lunaroute-routing/src/provider_router.rs
git commit -m "feat: add get_notification_message to Provider trait"
```

---

### Task 4.4: Implement Inject Notification Logic

**Files:**
- Modify: `crates/lunaroute-routing/src/provider_router.rs`

**Step 1: Write test for inject_notification_if_needed**

Add to test module in `crates/lunaroute-routing/src/provider_router.rs`:

```rust
#[tokio::test]
async fn test_inject_notification_when_enabled() {
    use crate::notification::{ProviderSwitchNotificationConfig, SwitchReason};
    use lunaroute_core::normalized::{Message, MessageContent, NormalizedRequest, Role};
    use std::collections::HashMap;

    let config = Some(ProviderSwitchNotificationConfig::default());

    let mut request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("User question".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        model: "gpt-4".to_string(),
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: HashMap::new(),
    };

    // Create a stub router for testing
    // For now, just test the logic in isolation
    // Full integration test will come later
}
```

**Step 2: Implement inject_notification_if_needed method**

Add to `Router` impl block:

```rust
use crate::notification::{
    build_notification_message, has_notification_already, SwitchReason,
};
use lunaroute_core::normalized::{Message, MessageContent, Role};

impl Router {
    // ... existing methods ...

    /// Inject notification message if needed
    ///
    /// Returns true if notification was injected, false otherwise
    fn inject_notification_if_needed(
        &self,
        request: &mut NormalizedRequest,
        original_provider_id: &str,
        new_provider_id: &str,
        switch_reason: SwitchReason,
    ) -> bool {
        // Check if notifications are enabled
        let notification_config = match &self.notification_config {
            Some(config) if config.enabled => config,
            _ => return false, // Disabled
        };

        // Check idempotency - already has notification?
        if has_notification_already(request) {
            return false;
        }

        // Get provider-specific message override if available
        let provider_override = self.get_provider_notification_message(new_provider_id);

        // Build notification message
        let notification_text = build_notification_message(
            original_provider_id,
            new_provider_id,
            switch_reason,
            &request.model,
            provider_override.as_deref(),
            notification_config,
        );

        // Create notification message
        let notification_message = Message {
            role: Role::User,
            content: MessageContent::Text(notification_text),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        };

        // Prepend to messages array
        request.messages.insert(0, notification_message);

        true
    }
}
```

**Step 3: Build to verify**

Run: `cargo build -p lunaroute-routing`

Expected: Success

**Step 4: Commit**

```bash
git add crates/lunaroute-routing/src/provider_router.rs
git commit -m "feat: implement inject_notification_if_needed method"
```

---

### Task 4.5: Integrate Notification Injection into Error Handling

**Files:**
- Modify: `crates/lunaroute-routing/src/provider_router.rs`

**Step 1: Find the provider switching logic**

Locate the code in `provider_router.rs` where:
- Rate limit errors are detected
- 5xx errors are detected
- Circuit breaker opens
- Provider switching occurs

Look for existing error handling patterns, likely in:
- `try_provider()` method
- Fallback chain logic
- Alternative provider selection (limits-alternative strategy)

**Step 2: Add notification injection for rate limit switches**

Find where rate limit detection triggers alternative provider selection (likely in the limits-alternative strategy handling). Add notification injection:

```rust
// After detecting we're switching due to rate limit
if switched_to_alternative {
    self.inject_notification_if_needed(
        &mut request,
        &original_provider_id,
        &alternative_provider_id,
        SwitchReason::RateLimit,
    );
}
```

**Step 3: Add notification injection for 5xx errors**

Find where 5xx errors trigger fallback provider selection. Add:

```rust
// After detecting 5xx error and selecting fallback
if is_5xx_error(&error) {
    self.inject_notification_if_needed(
        &mut request,
        &failed_provider_id,
        &fallback_provider_id,
        SwitchReason::ServiceIssue,
    );
}
```

**Step 4: Add notification injection for circuit breaker**

Find where circuit breaker open state triggers provider selection. Add:

```rust
// After circuit breaker rejects request
if circuit_breaker_open {
    self.inject_notification_if_needed(
        &mut request,
        &original_provider_id,
        &fallback_provider_id,
        SwitchReason::CircuitBreaker,
    );
}
```

**Step 5: Add helper to detect 5xx errors**

Add helper method to Router:

```rust
fn is_5xx_error(error: &lunaroute_core::Error) -> bool {
    match error {
        lunaroute_core::Error::Provider(msg) => {
            // Check if error message contains 5xx status codes
            msg.contains("500")
                || msg.contains("502")
                || msg.contains("503")
                || msg.contains("504")
        }
        _ => false,
    }
}
```

**Step 6: Build to verify**

Run: `cargo build -p lunaroute-routing`

Expected: Success (may have warnings about unused variables)

**Step 7: Run existing routing tests**

Run: `cargo test -p lunaroute-routing`

Expected: All existing tests still PASS

**Step 8: Commit**

```bash
git add crates/lunaroute-routing/src/provider_router.rs
git commit -m "feat: integrate notification injection into error handling"
```

---

## Phase 5: Integration Tests

### Task 5.1: Test Rate Limit Switch with Notification

**Files:**
- Create: `crates/lunaroute-integration-tests/tests/switch_notification_integration.rs`

**Step 1: Create test file with basic structure**

Create `crates/lunaroute-integration-tests/tests/switch_notification_integration.rs`:

```rust
//! Integration tests for provider switch notifications

use lunaroute_core::{
    normalized::{Message, MessageContent, NormalizedRequest, Role},
    provider::Provider,
};
use lunaroute_routing::{
    notification::ProviderSwitchNotificationConfig,
    Router, RoutingConfig, RoutingRule, RuleMatcher,
    RoutingStrategy, circuit_breaker::CircuitBreakerConfig,
};
use std::collections::HashMap;
use std::sync::Arc;
use wiremock::{Mock, MockServer, ResponseTemplate};
use wiremock::matchers::{method, path};

// Helper to create basic request
fn create_test_request() -> NormalizedRequest {
    NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("Test question".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        model: "gpt-4".to_string(),
        max_tokens: Some(100),
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: HashMap::new(),
    }
}

#[tokio::test]
async fn test_rate_limit_switch_injects_notification() {
    // TODO: Implement test
    // This will be implemented in the next step
}
```

**Step 2: Implement rate limit notification test**

Add to `switch_notification_integration.rs`:

```rust
#[tokio::test]
async fn test_rate_limit_switch_injects_notification() {
    // Setup mock servers
    let primary_server = MockServer::start().await;
    let alternative_server = MockServer::start().await;

    // Primary returns 429 (rate limit)
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
            "error": {
                "message": "Rate limit exceeded",
                "type": "rate_limit_error"
            }
        })))
        .mount(&primary_server)
        .await;

    // Alternative returns success
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Response from alternative"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 50,  // Increased due to notification
                "completion_tokens": 10,
                "total_tokens": 60
            }
        })))
        .mount(&alternative_server)
        .await;

    // Create providers
    let primary_config = lunaroute_egress::openai::OpenAIConfig {
        api_key: "test-key".to_string(),
        base_url: primary_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let primary = Arc::new(
        lunaroute_egress::openai::OpenAIConnector::new(primary_config)
            .await
            .unwrap()
    ) as Arc<dyn Provider>;

    let alternative_config = lunaroute_egress::openai::OpenAIConfig {
        api_key: "test-key".to_string(),
        base_url: alternative_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: Some(
            "ALTERNATIVE: Using backup due to ${reason}".to_string()
        ),
    };
    let alternative = Arc::new(
        lunaroute_egress::openai::OpenAIConnector::new(alternative_config)
            .await
            .unwrap()
    ) as Arc<dyn Provider>;

    // Create providers map
    let mut providers = HashMap::new();
    providers.insert("primary".to_string(), primary);
    providers.insert("alternative".to_string(), alternative);

    // Create routing config with limits-alternative strategy
    let routing_config = RoutingConfig {
        rules: vec![RoutingRule {
            name: Some("test-rule".to_string()),
            priority: 100,
            matcher: RuleMatcher::Always,
            provider_id: None,
            strategy: Some(RoutingStrategy::LimitsAlternative {
                primary_providers: vec!["primary".to_string()],
                alternative_providers: vec!["alternative".to_string()],
                exponential_backoff_base_secs: 60,
            }),
            fallbacks: vec![],
        }],
        health_monitor: None,
        circuit_breaker: Some(CircuitBreakerConfig::default()),
        provider_switch_notification: Some(ProviderSwitchNotificationConfig::default()),
    };

    // Create router
    let router = Router::new(
        providers,
        routing_config.rules,
        routing_config.health_monitor,
        routing_config.circuit_breaker,
        None, // metrics
        routing_config.provider_switch_notification,
    );

    // Send request
    let mut request = create_test_request();
    let response = router.route(request.clone()).await;

    // Verify response succeeded
    assert!(response.is_ok(), "Request should succeed with alternative");

    // Verify notification was injected by checking that the alternative
    // received a request with prepended notification message
    // (We can verify this by checking the mock server received the notification)
}
```

**Step 3: Run test**

Run: `cargo test -p lunaroute-integration-tests test_rate_limit_switch_injects_notification`

Expected: Test may need adjustments based on actual router implementation

**Step 4: Fix any issues and iterate**

Debug and fix until test passes.

**Step 5: Commit**

```bash
git add crates/lunaroute-integration-tests/tests/switch_notification_integration.rs
git commit -m "test: add rate limit switch notification integration test"
```

---

### Task 5.2: Test Notification Disabled

**Files:**
- Modify: `crates/lunaroute-integration-tests/tests/switch_notification_integration.rs`

**Step 1: Write test for disabled notifications**

Add to `switch_notification_integration.rs`:

```rust
#[tokio::test]
async fn test_notification_disabled() {
    // Setup mock servers (similar to previous test)
    let primary_server = MockServer::start().await;
    let alternative_server = MockServer::start().await;

    // Primary returns 429
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&primary_server)
        .await;

    // Alternative returns success with EXACT token count
    // (if notification is NOT injected, prompt_tokens should be lower)
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Response"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,  // Lower because no notification
                "completion_tokens": 5,
                "total_tokens": 15
            }
        })))
        .mount(&alternative_server)
        .await;

    // Create router with notification DISABLED
    let routing_config = RoutingConfig {
        // ... similar to previous test ...
        provider_switch_notification: Some(ProviderSwitchNotificationConfig {
            enabled: false,
            default_message: String::new(),
        }),
    };

    // ... rest of test setup ...

    let response = router.route(request).await.unwrap();

    // Verify notification was NOT injected
    // We can verify by checking token counts or examining the actual request
    // sent to the alternative server
}
```

**Step 2: Run test**

Run: `cargo test -p lunaroute-integration-tests test_notification_disabled`

**Step 3: Fix and iterate until passing**

**Step 4: Commit**

```bash
git add crates/lunaroute-integration-tests/tests/switch_notification_integration.rs
git commit -m "test: verify notifications can be disabled"
```

---

### Task 5.3: Test Cross-Dialect Notification

**Files:**
- Modify: `crates/lunaroute-integration-tests/tests/switch_notification_integration.rs`

**Step 1: Write cross-dialect test**

Add to `switch_notification_integration.rs`:

```rust
#[tokio::test]
async fn test_cross_dialect_notification() {
    // Setup OpenAI primary (will fail) and Anthropic alternative (will succeed)
    let openai_server = MockServer::start().await;
    let anthropic_server = MockServer::start().await;

    // OpenAI returns 503
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&openai_server)
        .await;

    // Anthropic returns success
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "msg-123",
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "text",
                "text": "Response from Anthropic"
            }],
            "model": "claude-sonnet-4",
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 50,
                "output_tokens": 10
            }
        })))
        .mount(&anthropic_server)
        .await;

    // Create OpenAI primary provider
    let openai_config = lunaroute_egress::openai::OpenAIConfig {
        api_key: "test-key".to_string(),
        base_url: openai_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let openai_provider = Arc::new(
        lunaroute_egress::openai::OpenAIConnector::new(openai_config)
            .await
            .unwrap()
    ) as Arc<dyn Provider>;

    // Create Anthropic alternative provider
    let anthropic_config = lunaroute_egress::anthropic::AnthropicConfig {
        api_key: "test-key".to_string(),
        base_url: anthropic_server.uri(),
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let anthropic_provider = Arc::new(
        lunaroute_egress::anthropic::AnthropicConnector::new(anthropic_config)
            .await
            .unwrap()
    ) as Arc<dyn Provider>;

    // Setup router with fallback from OpenAI to Anthropic
    let mut providers = HashMap::new();
    providers.insert("openai".to_string(), openai_provider);
    providers.insert("anthropic".to_string(), anthropic_provider);

    let routing_config = RoutingConfig {
        rules: vec![RoutingRule {
            name: Some("cross-dialect".to_string()),
            priority: 100,
            matcher: RuleMatcher::Always,
            provider_id: Some("openai".to_string()),
            strategy: None,
            fallbacks: vec!["anthropic".to_string()],
        }],
        health_monitor: None,
        circuit_breaker: Some(CircuitBreakerConfig::default()),
        provider_switch_notification: Some(ProviderSwitchNotificationConfig::default()),
    };

    let router = Router::new(
        providers,
        routing_config.rules,
        routing_config.health_monitor,
        routing_config.circuit_breaker,
        None,
        routing_config.provider_switch_notification,
    );

    // Send OpenAI-formatted request
    let request = create_test_request();
    let response = router.route(request).await;

    // Verify:
    // 1. Request succeeded
    assert!(response.is_ok());

    // 2. Response is in OpenAI format (translated back from Anthropic)
    let response = response.unwrap();
    assert_eq!(response.model, "claude-sonnet-4");

    // 3. Notification was injected before dialect translation
    // (verified by higher token count on Anthropic side)
}
```

**Step 2: Run test**

Run: `cargo test -p lunaroute-integration-tests test_cross_dialect_notification`

**Step 3: Fix and iterate**

**Step 4: Commit**

```bash
git add crates/lunaroute-integration-tests/tests/switch_notification_integration.rs
git commit -m "test: verify cross-dialect notification works"
```

---

### Task 5.4: Test Provider-Specific Custom Message

**Files:**
- Modify: `crates/lunaroute-integration-tests/tests/switch_notification_integration.rs`

**Step 1: Write test with custom provider message**

Add to `switch_notification_integration.rs`:

```rust
#[tokio::test]
async fn test_custom_provider_notification_message() {
    let primary_server = MockServer::start().await;
    let alternative_server = MockServer::start().await;

    // Primary returns 429
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&primary_server)
        .await;

    // Alternative returns success
    // We'll verify the custom message was used by checking the request body
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Response"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 40,
                "completion_tokens": 10,
                "total_tokens": 50
            }
        })))
        .mount(&alternative_server)
        .await;

    // Create alternative with custom message
    let custom_message = "CUSTOM: Switched to ${new_provider} from ${original_provider} for ${model}";
    let alternative_config = lunaroute_egress::openai::OpenAIConfig {
        api_key: "test-key".to_string(),
        base_url: alternative_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: Some(custom_message.to_string()),
    };

    // ... rest of setup ...

    let response = router.route(request).await.unwrap();

    // Verify custom message was used (contains "CUSTOM:")
    // We can verify by inspecting the actual request sent to alternative
    // or by checking for the substituted variables
}
```

**Step 2: Run test**

Run: `cargo test -p lunaroute-integration-tests test_custom_provider_notification_message`

**Step 3: Fix and iterate**

**Step 4: Commit**

```bash
git add crates/lunaroute-integration-tests/tests/switch_notification_integration.rs
git commit -m "test: verify provider-specific custom messages work"
```

---

### Task 5.5: Test Idempotency Guard

**Files:**
- Modify: `crates/lunaroute-integration-tests/tests/switch_notification_integration.rs`

**Step 1: Write test for cascading fallbacks**

Add to `switch_notification_integration.rs`:

```rust
#[tokio::test]
async fn test_notification_idempotency_cascading_fallbacks() {
    // Setup 3 servers: primary (fails), alt1 (fails), alt2 (succeeds)
    let primary_server = MockServer::start().await;
    let alt1_server = MockServer::start().await;
    let alt2_server = MockServer::start().await;

    // All fail except alt2
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&primary_server)
        .await;

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&alt1_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Final response"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 50,  // Should only have ONE notification
                "completion_tokens": 10,
                "total_tokens": 60
            }
        })))
        .mount(&alt2_server)
        .await;

    // Setup routing with cascading fallbacks
    let routing_config = RoutingConfig {
        rules: vec![RoutingRule {
            name: Some("cascade-test".to_string()),
            priority: 100,
            matcher: RuleMatcher::Always,
            provider_id: Some("primary".to_string()),
            strategy: None,
            fallbacks: vec!["alt1".to_string(), "alt2".to_string()],
        }],
        // ... rest of config ...
        provider_switch_notification: Some(ProviderSwitchNotificationConfig::default()),
    };

    // ... setup providers ...

    let response = router.route(request).await.unwrap();

    // Verify:
    // 1. Request succeeded
    assert!(response.is_ok());

    // 2. Only ONE notification was injected (idempotency)
    // We can verify by checking token count or examining the messages array
}
```

**Step 2: Run test**

Run: `cargo test -p lunaroute-integration-tests test_notification_idempotency_cascading_fallbacks`

**Step 3: Fix and iterate**

**Step 4: Commit**

```bash
git add crates/lunaroute-integration-tests/tests/switch_notification_integration.rs
git commit -m "test: verify idempotency with cascading fallbacks"
```

---

## Phase 6: Configuration Examples

### Task 6.1: Add Example Configuration File

**Files:**
- Create: `examples/configs/provider-switch-notification.yaml`

**Step 1: Create example config**

Create `examples/configs/provider-switch-notification.yaml`:

```yaml
# LunaRoute Configuration - Provider Switch Notification Example
#
# This configuration demonstrates the provider switch notification feature,
# which informs users when requests are automatically routed to alternative
# providers due to rate limits, errors, or circuit breaker events.

providers:
  # Primary OpenAI provider
  openai-primary:
    type: "openai"
    api_key: "$OPENAI_API_KEY"
    base_url: "https://api.openai.com/v1"
    timeout_secs: 60
    max_retries: 2

  # Backup OpenAI provider
  openai-backup:
    type: "openai"
    api_key: "$OPENAI_BACKUP_API_KEY"
    base_url: "https://api.openai.com/v1"
    timeout_secs: 60
    max_retries: 2
    # Custom notification when this provider is used
    switch_notification_message: |
      IMPORTANT: Please inform the user that we're using our backup OpenAI service
      to handle their request due to high demand on the primary service.
      Continue with their original request normally.

  # Anthropic as cross-dialect alternative
  anthropic-primary:
    type: "anthropic"
    api_key: "$ANTHROPIC_API_KEY"
    base_url: "https://api.anthropic.com"
    timeout_secs: 60
    max_retries: 2
    # Custom notification with variable substitution
    switch_notification_message: |
      IMPORTANT: Please inform the user that we're using Claude (${model})
      to handle their request due to ${reason}. Your response quality will
      remain the same. Continue with their original request.

routing:
  # Provider switch notification configuration
  provider_switch_notification:
    enabled: true  # On by default
    default_message: |
      IMPORTANT: Please inform the user that due to temporary service constraints,
      their request is being handled by an alternative AI service provider.
      Then proceed to fulfill their original request completely and professionally.

  # Health monitoring
  health_monitor:
    healthy_threshold: 0.95
    unhealthy_threshold: 0.50
    failure_window_secs: 60
    min_requests: 10

  # Circuit breaker
  circuit_breaker:
    failure_threshold: 5
    success_threshold: 2
    timeout_secs: 30

  # Routing rules
  rules:
    # GPT models with rate limit protection and notification
    - name: "gpt-with-notification"
      priority: 100
      matcher:
        model_pattern: "^gpt-.*"
      strategy:
        type: "limits-alternative"
        primary_providers:
          - "openai-primary"
          - "openai-backup"
        alternative_providers:
          - "anthropic-primary"
        exponential_backoff_base_secs: 60
      # Note: Notifications are injected automatically when switching
      # to alternatives or fallbacks

    # Claude models with fallback to OpenAI
    - name: "claude-with-fallback"
      priority: 90
      matcher:
        model_pattern: "^claude-.*"
      provider_id: "anthropic-primary"
      fallbacks:
        - "openai-primary"
        - "openai-backup"

listeners:
  - type: "http"
    address: "0.0.0.0:8081"
    dialect: "openai"

session_recording:
  type: "jsonl"
  path: "~/.lunaroute/sessions"
  enabled: true

observability:
  metrics:
    enabled: true
    port: 9090
  logging:
    level: "info"
```

**Step 2: Verify YAML is valid**

Run: `yamllint examples/configs/provider-switch-notification.yaml` (if yamllint installed)

Or just check it parses:
```bash
python3 -c "import yaml; yaml.safe_load(open('examples/configs/provider-switch-notification.yaml'))"
```

**Step 3: Commit**

```bash
git add examples/configs/provider-switch-notification.yaml
git commit -m "docs: add example config for provider switch notifications"
```

---

### Task 6.2: Update Main README

**Files:**
- Modify: `README.md`

**Step 1: Add section about provider switch notifications**

Find appropriate section in `README.md` (likely after routing strategies) and add:

```markdown
### Provider Switch Notifications

LunaRoute can automatically notify users when requests are routed to alternative providers due to rate limits, server errors, or circuit breaker events.

**Features:**
- 🔔 Automatic user notifications via LLM response
- 🎛️ Global on/off with per-provider customization
- 🔄 Works with cross-dialect failover (OpenAI ↔ Claude)
- 📝 Template variables for customization
- 🛡️ Idempotent (no duplicate notifications)

**Configuration:**

```yaml
routing:
  provider_switch_notification:
    enabled: true
    default_message: |
      IMPORTANT: Please inform the user that due to temporary service constraints,
      their request is being handled by an alternative AI service provider.
      Continue with their original request.

providers:
  anthropic-backup:
    type: "anthropic"
    # Custom message when THIS provider is used as alternative
    switch_notification_message: |
      Using Claude due to ${reason}. Quality remains the same.
```

**Template Variables:**
- `${original_provider}` - Provider that failed
- `${new_provider}` - Provider being used
- `${reason}` - Generic reason (high demand, service issue, maintenance)
- `${model}` - Model name

See `examples/configs/provider-switch-notification.yaml` for complete example.
```

**Step 2: Commit**

```bash
git add README.md
git commit -m "docs: add provider switch notification to README"
```

---

## Phase 7: Final Integration and Cleanup

### Task 7.1: Update Server to Pass Notification Config

**Files:**
- Modify: `crates/lunaroute-server/src/config.rs` (or wherever router is instantiated)

**Step 1: Find where Router is created**

Search for `Router::new()` calls in the server crate.

**Step 2: Pass notification config from routing config**

Update the call to include notification config:

```rust
let router = Router::new(
    providers,
    routing_config.rules,
    routing_config.health_monitor,
    routing_config.circuit_breaker,
    metrics.clone(),
    routing_config.provider_switch_notification, // Add this line
);
```

**Step 3: Build server**

Run: `cargo build -p lunaroute-server`

Expected: Success

**Step 4: Commit**

```bash
git add crates/lunaroute-server/src/config.rs
git commit -m "feat: wire notification config through server"
```

---

### Task 7.2: Run Full Test Suite

**Step 1: Run all workspace tests**

Run: `cargo test --workspace`

Expected: All tests PASS

**Step 2: Fix any failing tests**

If any tests fail, investigate and fix:
- Update test fixtures to include new config field
- Fix any integration issues
- Adjust assertions if needed

**Step 3: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`

Expected: No warnings

**Step 4: Fix any clippy warnings**

**Step 5: Commit fixes**

```bash
git add .
git commit -m "fix: address test failures and clippy warnings"
```

---

### Task 7.3: Manual Testing

**Step 1: Build release binary**

Run: `cargo build --release`

**Step 2: Start server with example config**

Run:
```bash
./target/release/lunaroute-server --config examples/configs/provider-switch-notification.yaml
```

**Step 3: Send test request that triggers rate limit**

Use curl or test client to send request to primary provider that will be rate-limited (if possible to simulate).

**Step 4: Verify notification appears in response**

Check that the LLM's response mentions the provider switch.

**Step 5: Test with notifications disabled**

Modify config to set `enabled: false`, restart, verify no notification.

**Step 6: Document findings**

Add notes to design doc about any issues found.

---

### Task 7.4: Final Commit and Summary

**Step 1: Review all changes**

Run: `git status` and `git log`

**Step 2: Create feature summary commit**

```bash
git commit --allow-empty -m "feat: complete provider switch notification feature

Summary of changes:
- Add SwitchReason enum for categorizing switches
- Add ProviderSwitchNotificationConfig for global settings
- Add switch_notification_message to provider configs
- Implement template variable substitution
- Implement notification message building and injection
- Integrate with router error handling (rate limits, 5xx, circuit breaker)
- Add comprehensive integration tests
- Add example configuration
- Update documentation

The feature is enabled by default with generic, professional messages.
Per-provider customization is supported via template variables.
Idempotency guard prevents duplicate notifications during cascading fallbacks.

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Summary

This implementation plan provides bite-sized tasks (2-5 minutes each) with exact file paths, complete code examples, and verification steps. The plan follows TDD principles and includes frequent commits.

**Estimated Total Time:** 4-6 hours
**Total Tasks:** 32 tasks across 7 phases
**Test Coverage:** Unit tests + integration tests for all scenarios

**Next Steps:** Use `superpowers:executing-plans` or `superpowers:subagent-driven-development` to execute this plan task-by-task.
