# Tool Call Failure Detection

**Status:** Implementation in Progress
**Feature Branch:** `feature/detect-tool-failures`
**Last Updated:** 2025-10-07

---

## Overview

This document specifies the implementation for detecting and tracking tool call failures in LunaRoute. Tool execution happens client-side after the LLM responds with tool calls, so we detect failures when the client sends back tool results in a follow-up request.

---

## Current State Analysis

### What Already Exists ✅

- `ToolCallRecorded` event with `success: Option<bool>` field (events.rs:53-63)
- `tool_calls` table in SQLite with `error_count` column (sqlite_writer.rs:295-311)
- `ToolUsageSummary` with `tool_error_count` field (events.rs:272-279)
- `ToolStats` with `error_count` per tool (events.rs:281-287)
- Metrics infrastructure for `tool_calls_total` (metrics.rs:252-258)

### What's Missing ❌

- Parsing of Anthropic's `is_error` field in `ToolResult` blocks
- Heuristic error detection for OpenAI tool messages
- Mapping `tool_call_id` → `tool_name` (needed to know which tool failed)
- Emission of `ToolCallRecorded` events with failure information
- Metrics for tool failures (`tool_result_failures_total`)

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│  Flow: Tool Call → Execution → Result → Detection          │
└─────────────────────────────────────────────────────────────┘

Request 1: Assistant calls tool
┌─────────┐      ┌──────────┐      ┌──────────┐
│ Client  │─────>│LunaRoute │─────>│ Provider │
└─────────┘      └──────────┘      └──────────┘
                      │
                      ├─> Record: tool_calls[{id, name}]
                      └─> Store mapping: id → name

Response 1: Tool calls returned
┌─────────┐      ┌──────────┐      ┌──────────┐
│ Client  │<─────│LunaRoute │<─────│ Provider │
└─────────┘      └──────────┘      └──────────┘
     │
     └─> Client executes tools locally

Request 2: Tool results sent back
┌─────────┐      ┌──────────┐      ┌──────────┐
│ Client  │─────>│LunaRoute │─────>│ Provider │
└─────────┘      └──────────┘      └──────────┘
   │                   │
   │                   ├─> Parse tool_result blocks
   │                   ├─> Check is_error field (Anthropic)
   │                   ├─> Lookup tool_call_id → name
   │                   ├─> Emit ToolCallRecorded event
   │                   └─> Record metrics
   │
   └─> Content includes:
       Anthropic: {type: "tool_result", is_error: true}
       OpenAI: {role: "tool", content: "Error: ..."}
```

---

## Implementation Phases

### Phase 1: Extend Core Types (Foundation)

#### 1.1 Update Anthropic ingress types

**File:** `crates/lunaroute-ingress/src/anthropic.rs`

```rust
// CURRENT (line 119-123):
ToolResult {
    tool_use_id: String,
    content: String,
}

// NEW:
ToolResult {
    tool_use_id: String,
    content: String,
    #[serde(default)]  // Optional for backward compatibility
    is_error: Option<bool>,
}
```

#### 1.2 Add ToolResult to normalized types

**File:** `crates/lunaroute-core/src/normalized.rs`

```rust
// Add new struct (after line 165):
/// Tool result from client execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// ID of the tool call this is a result for
    pub tool_call_id: String,

    /// Whether this tool execution failed
    pub is_error: bool,

    /// Result content (error message or success data)
    pub content: String,

    /// Optional: tool name if we can determine it
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

// Add to NormalizedRequest (line 8):
pub struct NormalizedRequest {
    // ... existing fields ...

    /// Tool results from previous execution (if this is a follow-up)
    #[serde(default)]
    pub tool_results: Vec<ToolResult>,

    // ... metadata ...
}
```

---

### Phase 2: Tool Call ID Tracking (Critical Infrastructure)

#### 2.1 Add tool call mapper

**File:** `crates/lunaroute-session/src/tool_mapper.rs` (NEW FILE)

```rust
//! Maps tool_call_id to tool_name for tracking results

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Tracks tool calls to map IDs to names when results arrive
pub struct ToolCallMapper {
    /// Map: tool_call_id → (tool_name, timestamp)
    mappings: HashMap<String, (String, Instant)>,
    /// TTL for mappings (default: 1 hour)
    ttl: Duration,
}

impl ToolCallMapper {
    pub fn new() -> Self {
        Self::with_ttl(Duration::from_secs(3600))
    }

    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            mappings: HashMap::new(),
            ttl,
        }
    }

    /// Record a tool call from a response
    pub fn record_call(&mut self, tool_call_id: String, tool_name: String) {
        self.cleanup_expired();
        self.mappings.insert(tool_call_id, (tool_name, Instant::now()));
    }

    /// Look up tool name from result ID
    pub fn lookup(&self, tool_call_id: &str) -> Option<String> {
        self.mappings.get(tool_call_id)
            .filter(|(_, ts)| ts.elapsed() < self.ttl)
            .map(|(name, _)| name.clone())
    }

    /// Clean up expired entries
    fn cleanup_expired(&mut self) {
        self.mappings.retain(|_, (_, ts)| ts.elapsed() < self.ttl);
    }
}
```

#### 2.2 Integrate into SessionRecorder

**File:** `crates/lunaroute-session/src/recorder.rs`

```rust
pub struct SessionRecorder {
    // ... existing fields ...

    /// Maps tool call IDs to tool names
    tool_mapper: Arc<RwLock<ToolCallMapper>>,
}
```

---

### Phase 3: Parse Tool Results (Detection Logic)

#### 3.1 Anthropic parsing

**File:** `crates/lunaroute-ingress/src/anthropic.rs`

```rust
// In to_normalized() function (around line 344-402):
let (text_content, tool_calls, tool_call_id, is_tool_error) = match content {
    AnthropicMessageContent::Blocks(blocks) => {
        let mut text_parts = Vec::new();
        let mut tool_calls_vec = Vec::new();
        let mut tool_result_id = None;
        let mut tool_error = None;  // NEW

        for block in blocks {
            match block {
                AnthropicContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,  // NEW
                } => {
                    tool_result_id = Some(tool_use_id);
                    tool_error = is_error;  // Capture error flag
                    text_parts.push(content);
                }
                // ... other blocks ...
            }
        }

        (text_parts.join("\n"), tool_calls_vec, tool_result_id, tool_error)
    }
    // ...
};

// Then populate tool_results in NormalizedRequest:
let tool_results = if let (Some(id), Some(is_err)) = (tool_call_id.as_ref(), is_tool_error) {
    vec![ToolResult {
        tool_call_id: id.clone(),
        is_error: is_err,
        content: text_content.clone(),
        tool_name: None,  // Filled in later by SessionRecorder
    }]
} else {
    vec![]
};
```

#### 3.2 OpenAI parsing

**File:** `crates/lunaroute-ingress/src/openai.rs`

```rust
// Heuristic error detection for OpenAI tool messages
fn detect_tool_error(content: &str) -> bool {
    let lower = content.to_lowercase();

    // Conservative keywords indicating errors
    let error_patterns = [
        "error:",
        "failed:",
        "exception:",
        "traceback:",
        "not found:",
        "invalid:",
        "cannot ",
        "unable to",
    ];

    // Check if content starts with error pattern (first 100 chars)
    let prefix = lower.chars().take(100).collect::<String>();
    error_patterns.iter().any(|pattern| prefix.contains(pattern))
}

// In parsing code:
if msg.role == "tool" {
    let is_error = detect_tool_error(&content_text);
    tool_results.push(ToolResult {
        tool_call_id: msg.tool_call_id.unwrap(),
        is_error,
        content: content_text,
        tool_name: None,
    });
}
```

---

### Phase 4: Record Tool Results (Events & Storage)

#### 4.1 Emit events from SessionRecorder

**File:** `crates/lunaroute-session/src/recorder.rs`

```rust
// When recording a request with tool_results:
pub async fn record_request(&self, request: &NormalizedRequest) {
    // ... existing recording ...

    // Record tool results
    for result in &request.tool_results {
        // Look up tool name from previous call
        let tool_name = self.tool_mapper.read().await
            .lookup(&result.tool_call_id)
            .unwrap_or_else(|| "unknown".to_string());

        // Emit ToolCallRecorded event
        let event = SessionEvent::ToolCallRecorded {
            session_id: self.session_id.clone(),
            request_id: self.request_id.clone(),
            timestamp: Utc::now(),
            tool_name: tool_name.clone(),
            tool_call_id: result.tool_call_id.clone(),
            execution_time_ms: None,  // Unknown (client-side)
            input_size_bytes: 0,  // From previous call (TODO)
            output_size_bytes: Some(result.content.len()),
            success: Some(!result.is_error),  // ← KEY FIELD
        };

        self.event_tx.send(event).await;
    }
}

// When recording a response with tool_calls:
pub async fn record_response(&self, response: &NormalizedResponse) {
    // ... existing recording ...

    // Track tool calls for future lookup
    for choice in &response.choices {
        for tool_call in &choice.message.tool_calls {
            self.tool_mapper.write().await.record_call(
                tool_call.id.clone(),
                tool_call.function.name.clone(),
            );
        }
    }
}
```

#### 4.2 Update SQLite writer

**File:** `crates/lunaroute-session/src/sqlite_writer.rs`

```rust
// The tool_calls table already has error_count column!
// Just need to increment it when writing ToolCallRecorded events:

async fn handle_tool_call_recorded(&self, event: ToolCallRecorded) {
    sqlx::query(
        r#"
        INSERT INTO tool_calls (
            session_id, request_id, model_name, tool_name,
            call_count, error_count, created_at
        )
        VALUES (?, ?, ?, ?, 1, ?, CURRENT_TIMESTAMP)
        ON CONFLICT(session_id, request_id, tool_name) DO UPDATE SET
            call_count = call_count + 1,
            error_count = error_count + ?
        "#,
    )
    .bind(&event.session_id)
    .bind(&event.request_id)
    .bind(&model_name)
    .bind(&event.tool_name)
    .bind(if event.success == Some(false) { 1 } else { 0 })  // error_count
    .bind(if event.success == Some(false) { 1 } else { 0 })  // for UPDATE
    .execute(&self.pool)
    .await?;
}
```

---

### Phase 5: Metrics (Observability)

#### 5.1 Add metrics

**File:** `crates/lunaroute-observability/src/metrics.rs`

```rust
// Add to Metrics struct (after line 65):
/// Tool result failures
pub tool_result_failures_total: CounterVec,

// In Metrics::new() (after line 252):
let tool_result_failures_total = CounterVec::new(
    Opts::new(
        "lunaroute_tool_result_failures_total",
        "Total number of failed tool executions detected",
    ),
    &["provider", "model", "tool_name"],
)?;

// Register it (after line 392):
registry.register(Box::new(tool_result_failures_total.clone()))?;

// Add recording method (after line 545):
/// Record a tool result failure
pub fn record_tool_failure(&self, provider: &str, model: &str, tool_name: &str) {
    self.tool_result_failures_total
        .with_label_values(&[provider, model, tool_name])
        .inc();

    // Also increment total tool calls
    self.tool_calls_total
        .with_label_values(&[provider, model, tool_name])
        .inc();
}
```

#### 5.2 Call from SessionRecorder

```rust
// In record_request() where we emit ToolCallRecorded:
if result.is_error {
    if let Some(metrics) = &self.metrics {
        metrics.record_tool_failure(
            &self.provider,
            &self.model,
            &tool_name,
        );
    }
}
```

---

### Phase 6: JSONL Enhancement (No Changes Needed)

The existing `ToolCallRecorded` event already goes to JSONL with the `success` field:

```json
{
  "type": "tool_call_recorded",
  "session_id": "...",
  "tool_name": "get_weather",
  "tool_call_id": "call_abc123",
  "success": false,
  "execution_time_ms": null,
  "output_size_bytes": 156
}
```

---

## Database Schema (No Changes Needed)

The existing schema already supports everything:

```sql
-- Existing table structure (already perfect!):
CREATE TABLE tool_calls (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    request_id TEXT,
    model_name TEXT,
    tool_name TEXT NOT NULL,
    call_count INTEGER DEFAULT 1,
    avg_execution_time_ms INTEGER,
    error_count INTEGER DEFAULT 0,  -- ← Already exists!
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
);
```

### Useful Queries

```sql
-- Which tools fail most often?
SELECT tool_name, SUM(error_count) as failures, SUM(call_count) as total,
       ROUND(100.0 * SUM(error_count) / SUM(call_count), 2) as failure_rate_pct
FROM tool_calls
GROUP BY tool_name
ORDER BY failures DESC;

-- Tool failure rate by model
SELECT model_name, tool_name, SUM(error_count), SUM(call_count)
FROM tool_calls
GROUP BY model_name, tool_name
HAVING SUM(error_count) > 0;

-- Sessions with tool failures
SELECT s.session_id, s.model_used, t.tool_name, t.error_count
FROM sessions s
JOIN tool_calls t ON s.session_id = t.session_id
WHERE t.error_count > 0
ORDER BY s.started_at DESC;
```

---

## Testing Strategy

### Unit Tests

- `test_anthropic_tool_result_error_detection` - Parse Anthropic is_error field
- `test_openai_tool_message_error_heuristic` - Detect OpenAI error messages
- `test_tool_mapper_lookup` - Tool ID → name mapping
- `test_tool_mapper_expiry` - TTL cleanup

### Integration Tests

- `test_tool_failure_recorded_to_sqlite` - End-to-end SQLite recording
- `test_tool_failure_metrics_emitted` - Prometheus metrics verification
- `test_tool_failure_in_jsonl` - JSONL event format

### Real API Tests

- `test_anthropic_tool_failure_detection_real` - Live Anthropic API
- `test_openai_tool_failure_heuristic_real` - Live OpenAI API

---

## Edge Cases & Considerations

### 1. Tool name lookup failures

**Problem:** tool_call_id not in mapper (expired or missing)
**Solution:** Use "unknown" as tool_name, log warning

### 2. OpenAI false positives

**Problem:** Tool returns "Error code: 200" (legitimate content)
**Mitigation:** Only check first 100 chars, use conservative keywords
**Future:** Add config option to disable heuristic

### 3. Multiple tool results in one request

**Handled:** `tool_results: Vec<ToolResult>` supports multiple

### 4. Streaming tool calls

**Current:** Tool calls reassembled from deltas
**No changes needed** - detection happens on follow-up request

### 5. Backward compatibility

- **JSONL:** New events ignored by old parsers ✅
- **SQLite:** error_count already exists, defaults to 0 ✅
- **Metrics:** New metrics are additive ✅
- **API:** `is_error` field is `Option<bool>` ✅

---

## Success Metrics

After implementation, we'll have:

1. **Per-tool failure rates** (in database & Prometheus)
2. **Historical trend** of tool reliability
3. **Model comparison** (which models fail tools more?)
4. **Provider comparison** (Anthropic vs OpenAI tool reliability)
5. **Actionable data** for tool improvements

---

## Implementation Order

```
Phase 1 → Phase 2 → Phase 3 → Phase 4 → Phase 5
(2 hours)  (1 hour)   (3 hours)  (2 hours)  (1 hour)
   ↓          ↓          ↓          ↓         ↓
 Types → Tool Mapper → Parsing → Recording → Metrics

Total: ~9 hours of focused development
```

---

## Key Insight

Most infrastructure already exists! We mainly need to:

1. Add `is_error` field to Anthropic `ToolResult` parsing
2. Add heuristic error detection for OpenAI
3. Create tool call ID → name mapper
4. Wire up existing events to be emitted
5. Add metrics for tool failures

The database schema, events, and JSONL format already support everything we need. We're just filling in the detection and tracking logic!
