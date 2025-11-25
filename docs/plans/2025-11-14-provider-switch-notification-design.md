# Provider Switch Notification Design

**Date:** 2025-11-14
**Status:** Design Complete - Ready for Implementation
**Feature Branch:** `feature/provider-switch-notification`

## Overview

This feature adds user-facing notifications when LunaRoute switches providers due to rate limits, server errors, or circuit breaker events. The notification is injected as a prepended user message that instructs the LLM to inform the end user about the provider switch and continue with their request.

## Motivation

When LunaRoute automatically fails over to alternative providers, end users may notice:
- Different response styles between providers
- Slight delays during failover
- Different model capabilities (e.g., OpenAI GPT → Claude)

By proactively notifying users through the LLM's response, we:
1. Maintain transparency about service operation
2. Set appropriate expectations
3. Reduce confusion when responses differ from normal
4. Demonstrate system reliability (automatic failover working)

## User Requirements

From requirements gathering:
- **Injection method**: Prepend user message (compatible with both OpenAI and Claude APIs)
- **Triggers**: Rate limits (429), 5xx errors, circuit breaker open
- **Default message**: Generic, professional, instructs model to inform user and continue
- **Configuration**: Global default (on/off + message) + per-provider override
- **Template variables**: `${new_provider}`, `${original_provider}`, `${reason}`, `${model}`
- **Default state**: Enabled by default

## Architecture

### Component Placement

The feature is implemented at the **routing layer** (`provider_router.rs`), not the egress layer.

**Rationale:**
1. **Context awareness**: Router knows when provider switches occur and why
2. **Dialect independence**: Injection happens before dialect translation
3. **Centralized logic**: All failover scenarios handled in one place
4. **Clean separation**: Egress layer remains focused on provider communication

### Request Flow

```
Original Request
    ↓
Router detects provider switch needed
    ↓
Check: Should notify? (global flag + provider config)
    ↓
If yes: Inject notification user message at index 0
    ↓
Modified Request → Provider (egress layer)
    ↓
Dialect translation (if cross-dialect failover)
    ↓
Send to actual LLM
```

### Key Design Decisions

- **Prepend, not append**: Message goes at start of messages array for maximum visibility
- **Before egress**: Injection happens in router, before passing to provider connectors
- **Idempotent**: Multiple failovers don't create duplicate notifications
- **Streaming compatible**: Works identically for streaming and non-streaming requests

## Configuration

### YAML Schema

```yaml
# Global routing configuration
routing:
  # Provider switch notification configuration
  provider_switch_notification:
    enabled: true  # On by default
    default_message: |
      IMPORTANT: Please inform the user that due to temporary service constraints,
      their request is being handled by an alternative AI service provider.
      Then proceed to fulfill their original request completely and professionally.

# Provider configuration
providers:
  openai-primary:
    type: "openai"
    api_key: "$OPENAI_API_KEY"
    # ... other config ...

  anthropic-backup:
    type: "anthropic"
    api_key: "$ANTHROPIC_API_KEY"
    # Optional: Custom message when THIS provider is used as alternative
    switch_notification_message: |
      IMPORTANT: Please inform the user that we're using Claude (Anthropic)
      to handle their request due to high demand on our primary service.
      Continue with their original request.
```

### Template Variable Substitution

Custom messages support these template variables:
- `${new_provider}`: Provider ID being switched to (e.g., "anthropic-backup")
- `${original_provider}`: Provider ID that failed (e.g., "openai-primary")
- `${reason}`: Generic user-facing reason (see Reason Mapping below)
- `${model}`: Model name from request (e.g., "gpt-4", "claude-sonnet-4")

**Example with variables:**
```yaml
switch_notification_message: |
  IMPORTANT: Inform the user we switched from ${original_provider} to ${new_provider}
  due to ${reason}. Their ${model} request will be handled normally. Continue.
```

### Configuration Precedence

1. **Global disabled**: No notifications regardless of provider config
2. **Global enabled + no provider override**: Use `default_message`
3. **Global enabled + provider override**: Use provider's `switch_notification_message`

## Implementation Details

### Reason Code Mapping

Map technical failures to generic user-facing reasons:

```rust
pub enum SwitchReason {
    RateLimit,      // 429 errors
    ServiceIssue,   // 5xx errors
    CircuitBreaker, // Circuit breaker open
}

impl SwitchReason {
    fn to_generic_message(&self) -> &'static str {
        match self {
            SwitchReason::RateLimit => "high demand",
            SwitchReason::ServiceIssue => "a temporary service issue",
            SwitchReason::CircuitBreaker => "service maintenance",
        }
    }
}
```

**Rationale for generic messages:**
- Avoid technical jargon that confuses users
- Maintain professional tone
- Don't alarm users unnecessarily
- Focus on "service continues normally"

### Configuration Structs

```rust
// In crates/lunaroute-routing/src/config.rs or new notifications.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSwitchNotificationConfig {
    /// Enable/disable notifications globally
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Default message template
    #[serde(default = "default_notification_message")]
    pub default_message: String,
}

fn default_enabled() -> bool {
    true
}

fn default_notification_message() -> String {
    "IMPORTANT: Please inform the user that due to temporary service constraints, \
     their request is being handled by an alternative AI service provider. \
     Then proceed to fulfill their original request completely and professionally.".to_string()
}
```

### Provider Config Extension

```rust
// In crates/lunaroute-egress/src/openai.rs
pub struct OpenAIConfig {
    // ... existing fields ...

    /// Optional custom notification message when this provider is used as alternative
    pub switch_notification_message: Option<String>,
}

// In crates/lunaroute-egress/src/anthropic.rs
pub struct AnthropicConfig {
    // ... existing fields ...

    /// Optional custom notification message when this provider is used as alternative
    pub switch_notification_message: Option<String>,
}
```

### Message Injection Logic

In `provider_router.rs`, when switching providers:

```rust
// After detecting we need to switch providers
if should_inject_notification(
    &notification_config,
    &original_provider_id,
    &new_provider_id,
    &request,
) {
    let notification_message = build_notification_message(
        &original_provider_id,
        &new_provider_id,
        &switch_reason,
        &request.model,
        &provider_config,
        &global_config,
    );

    // Prepend to messages array (index 0)
    request.messages.insert(0, Message {
        role: Role::User,
        content: MessageContent::Text(notification_message),
        name: None,
        tool_calls: vec![],
        tool_call_id: None,
    });
}
```

### Idempotency Guard

Prevent multiple notifications if cascading through multiple fallbacks:

```rust
/// Check if notification was already injected
fn has_notification(request: &NormalizedRequest) -> bool {
    request.messages.first()
        .and_then(|m| match &m.content {
            MessageContent::Text(t) => Some(t.starts_with("IMPORTANT:")),
            _ => None,
        })
        .unwrap_or(false)
}
```

### Template Substitution Function

```rust
fn substitute_template_variables(
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

### Router State Extension

```rust
pub struct Router {
    // ... existing fields ...
    notification_config: Option<ProviderSwitchNotificationConfig>,
}
```

## Testing Strategy

### Unit Tests

**File:** `crates/lunaroute-routing/tests/notification_tests.rs`

```rust
#[test]
fn test_template_variable_substitution() {
    // Verify all template variables are correctly replaced
    // Test edge cases: missing variables, special characters
}

#[test]
fn test_notification_message_prepended() {
    // Verify message is inserted at index 0
    // Verify message structure (role, content)
}

#[test]
fn test_idempotency_guard() {
    // Multiple switches don't create multiple notifications
    // Existing notification is detected and preserved
}

#[test]
fn test_disabled_config() {
    // When globally disabled, no injection happens
}

#[test]
fn test_per_provider_override() {
    // Provider-specific message takes precedence over default
}

#[test]
fn test_reason_mapping() {
    // Verify each SwitchReason maps to appropriate generic message
}

#[test]
fn test_empty_messages_array() {
    // Handles request with no existing messages
}
```

### Integration Tests

**File:** `crates/lunaroute-integration-tests/tests/switch_notification_integration.rs`

```rust
#[tokio::test]
async fn test_rate_limit_switch_with_notification() {
    // Setup: Primary returns 429, alternative returns 200
    // Verify: Notification injected, request succeeds
    // Verify: Message contains rate limit reason
}

#[tokio::test]
async fn test_5xx_error_switch_with_notification() {
    // Setup: Primary returns 503, alternative returns 200
    // Verify: Notification with "service issue" reason
}

#[tokio::test]
async fn test_circuit_breaker_notification() {
    // Setup: Circuit breaker open for primary
    // Verify: Notification with "maintenance" reason
}

#[tokio::test]
async fn test_cross_dialect_notification() {
    // Setup: OpenAI primary fails → Anthropic alternative
    // Verify: Notification injected before dialect translation
    // Verify: Response in OpenAI format
}

#[tokio::test]
async fn test_custom_provider_message() {
    // Setup: Provider has custom switch_notification_message
    // Verify: Custom message used instead of default
}

#[tokio::test]
async fn test_notification_disabled() {
    // Setup: Global config enabled=false
    // Verify: No notification injected despite switch
}

#[tokio::test]
async fn test_streaming_with_notification() {
    // Setup: Streaming request with provider switch
    // Verify: Notification in context, streaming works
}

#[tokio::test]
async fn test_cascading_fallbacks() {
    // Setup: Primary fails, alt1 fails, alt2 succeeds
    // Verify: Only ONE notification (idempotency)
}
```

## Edge Cases

### 1. Empty Messages Array
**Scenario:** Request has no messages (rare edge case)
**Handling:** Create messages array with notification as only element

### 2. Streaming Requests
**Scenario:** Provider switch during streaming request
**Handling:** Notification injected normally; becomes part of prompt context

### 3. Multiple Consecutive Switches
**Scenario:** Cascading through multiple fallback providers
**Handling:** Idempotency guard prevents duplicate notifications

### 4. Missing Template Variables
**Scenario:** Custom template uses variable that's unavailable
**Handling:** Leave variable unreplaced (e.g., `${unknown}` stays as-is)
**Alternative:** Could replace with empty string or placeholder

### 5. Very Long Custom Messages
**Scenario:** Admin configures extremely long notification message
**Handling:** No artificial limit; trust configuration
**Documentation:** Recommend keeping under 200 tokens

### 6. Global Disabled, Provider Override
**Scenario:** Global config disabled but provider has custom message
**Handling:** Global flag takes precedence; no notification

### 7. Cross-Dialect System Prompt
**Scenario:** OpenAI request with system prompt fails over to Claude
**Handling:** Notification injected before translation; translator handles it correctly

### 8. Tool Calls in Progress
**Scenario:** Request contains tool calls/results
**Handling:** Notification prepended; tool context preserved

## API Compatibility

### OpenAI API
- ✅ Supports multiple `user` role messages
- ✅ Messages processed in order
- ✅ Prepended user message works naturally

### Claude API
- ✅ Supports `user` role in messages array
- ✅ Must alternate user/assistant roles
- ✅ Prepended user message compliant
- ⚠️ Claude uses separate `system` parameter (not a message role)
- ✅ Dialect translator handles this correctly

### Cross-Dialect Translation
When failing over from OpenAI → Claude:
1. Notification injected as `user` message in OpenAI format
2. Dialect translator converts request to Claude format
3. Notification remains as `user` message (compatible)
4. System messages handled via Claude's `system` parameter

## Security Considerations

### Prompt Injection Risks
**Risk:** Malicious user could craft requests to abuse notification feature
**Mitigation:**
- Notification always uses hardcoded prefix (`IMPORTANT:`)
- Template substitution is simple string replace (no code execution)
- Variables come from trusted internal state, not user input

### Information Disclosure
**Risk:** Notification reveals internal provider IDs
**Mitigation:**
- Provider IDs are configuration-defined (admin-controlled)
- Can use friendly names in config (e.g., "primary-service" instead of "openai-1234")
- Generic reasons don't expose technical details

### Message Tampering
**Risk:** User provides message that looks like notification
**Impact:** Minimal; model treats it as regular user message
**Note:** Idempotency guard uses `IMPORTANT:` prefix check (simple but effective)

## Performance Impact

### Negligible Overhead
- **String operations**: Template substitution (4 replacements) ~1-2μs
- **Message insertion**: `Vec::insert(0, item)` ~O(n) but n is small (typically <20 messages)
- **Idempotency check**: First message content check ~1μs
- **Total overhead**: <10μs per request with provider switch

### Memory Impact
- One additional `Message` struct per switched request
- Typical notification: ~200 bytes
- Negligible compared to LLM request/response sizes (typically 10KB+)

## Observability

### Logging
Add debug/info logs when notification injected:

```rust
tracing::info!(
    original_provider = %original_provider_id,
    new_provider = %new_provider_id,
    reason = ?switch_reason,
    "Injected provider switch notification"
);
```

### Metrics
Reuse existing metrics:
- `rate_limit_alternatives_used` already tracks provider switches
- `fallback_triggered` tracks fallback usage
- No new metrics needed for this feature

### Session Recording
- Notification appears in recorded messages (as first message)
- Easy to identify: starts with `IMPORTANT:`
- Helps debugging: shows which requests had provider switches

## Future Enhancements

Potential improvements for later iterations:

1. **Localization**: Support multiple languages for notifications
2. **Severity Levels**: Different message styles for critical vs non-critical switches
3. **User Preferences**: Per-user notification preferences (if multi-tenancy)
4. **Metrics Dashboard**: Track notification frequency, provider switch patterns
5. **A/B Testing**: Test different notification phrasings for user satisfaction
6. **Smart Suppression**: Don't notify if user unlikely to notice (e.g., simple queries)

## Implementation Plan

See separate implementation plan document: `2025-11-14-provider-switch-notification-plan.md`

## References

- **OpenAI API Messages**: Research confirmed multiple system messages supported
- **Claude API Messages**: Single system parameter, user/assistant messages only
- **Existing Features**:
  - Limits-alternative strategy: `docs/limits-alternative-routing-strategy.md`
  - Request body modifications: `crates/lunaroute-egress/src/openai.rs` (`prepend_messages`)
  - Circuit breaker: `crates/lunaroute-routing/src/circuit_breaker.rs`

## Changelog

- **2025-11-14**: Initial design document created
- **2025-11-14**: Design validated through collaborative brainstorming
- **2025-11-14**: Ready for implementation phase
