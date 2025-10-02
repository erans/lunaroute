# Async Multi-Writer Session Recording - Usage Guide

## Overview

The async multi-writer session recording system provides:
- **Zero request blocking**: All recording happens in background workers
- **Dual storage**: JSONL files for complete data + SQLite for queryable metadata
- **Configurable writers**: Enable/disable JSONL and SQLite independently
- **Batched writes**: Efficient I/O with configurable batch sizes
- **Schema versioning**: SQLite schema version tracking (v1)

## Configuration

### YAML Configuration

```yaml
session_recording:
  # Master switch - disables everything if false
  enabled: true

  # JSONL writer (full session data)
  jsonl:
    enabled: true
    directory: "~/.lunaroute/sessions"

  # SQLite writer (queryable metadata)
  sqlite:
    enabled: true
    path: "~/.lunaroute/sessions.db"
    max_connections: 5

  # Background worker settings
  worker:
    batch_size: 100           # Events to buffer before flush
    batch_timeout_ms: 100     # Max time to wait before flush
    channel_buffer_size: 10000  # Max events in channel (backpressure)
```

### Configuration Options

**Supported Combinations:**
- ✅ Both JSONL and SQLite enabled
- ✅ Only JSONL enabled
- ✅ Only SQLite enabled (requires `sqlite-writer` feature)
- ✅ Both disabled (recording off)

**Master Switch:**
- `enabled: false` disables all recording regardless of writer settings
- `enabled: true` allows individual writers to be enabled/disabled

## Usage Example

### Building from Configuration

```rust
use lunaroute_session::{build_from_config, SessionRecordingConfig};

// Load config from YAML
let config: SessionRecordingConfig = /* ... */;

// Build recorder (async)
let recorder = build_from_config(&config).await?;

// Use in your application
if let Some(recorder) = recorder {
    // Record events (non-blocking)
    recorder.record_event(SessionEvent::Started { /* ... */ });
    recorder.record_event(SessionEvent::ResponseRecorded { /* ... */ });
    recorder.record_event(SessionEvent::Completed { /* ... */ });
}
```

### Manual Builder

```rust
use lunaroute_session::{RecorderBuilder, JsonlWriter, SqliteWriter};
use std::sync::Arc;

let recorder = RecorderBuilder::new()
    .add_writer(Arc::new(JsonlWriter::new("/tmp/sessions".into())))
    .add_writer(Arc::new(SqliteWriter::new("/tmp/sessions.db").await?))
    .batch_size(100)
    .batch_timeout_ms(100)
    .build();
```

## Event Types

All events include `session_id` and `request_id` for correlation:

### Started
```rust
SessionEvent::Started {
    session_id: String,
    request_id: String,
    timestamp: DateTime<Utc>,
    model_requested: String,
    provider: String,
    listener: String,
    metadata: SessionMetadata,
}
```

### ResponseRecorded
```rust
SessionEvent::ResponseRecorded {
    session_id: String,
    request_id: String,
    timestamp: DateTime<Utc>,
    response_text: String,
    response_json: Value,
    model_used: String,
    stats: ResponseStats {
        provider_latency_ms: u64,
        post_processing_ms: f64,
        total_proxy_overhead_ms: f64,
        tokens: TokenStats { /* ... */ },
        tool_calls: Vec<ToolCallStats>,
        response_size_bytes: usize,
        content_blocks: usize,
        has_refusal: bool,
    },
}
```

### Completed
```rust
SessionEvent::Completed {
    session_id: String,
    request_id: String,
    timestamp: DateTime<Utc>,
    success: bool,
    error: Option<String>,
    finish_reason: Option<String>,
    final_stats: FinalSessionStats { /* ... */ },
}
```

## SQLite Schema (Version 1)

### Key Tables

**schema_version**: Schema version tracking
```sql
CREATE TABLE schema_version (
    version INTEGER PRIMARY KEY
);
```

**sessions**: Core session metadata
- Includes: `session_id`, `request_id`, `model_used`, `tokens`, `latency`
- Indexes:
  - `idx_sessions_created`: `created_at DESC`
  - `idx_sessions_provider`: `provider, created_at DESC`
  - `idx_sessions_model`: `model_used, created_at DESC`
  - `idx_sessions_request_id`: `request_id`
  - `idx_sessions_provider_model`: `provider, model_used, created_at DESC`

**session_stats**: Detailed stats per session
- Includes: `session_id`, `request_id`, `model_name`, timing/token stats
- Indexes:
  - `idx_session_stats_session`: `session_id`
  - `idx_session_stats_model`: `model_name, created_at DESC`
  - `idx_session_stats_session_time`: `session_id, created_at DESC`

**tool_calls**: Tool usage tracking
- Includes: `session_id`, `request_id`, `model_name`, `tool_name`, `call_count`
- Indexes:
  - `idx_tool_calls_model`: `model_name, created_at DESC`
  - `idx_tool_calls_session`: `session_id`
  - `idx_tool_calls_name`: `tool_name, created_at DESC`

### Query Examples

```sql
-- High thinking token usage
SELECT session_id, model_used, thinking_tokens, response_text
FROM sessions
WHERE thinking_tokens > 10000
ORDER BY thinking_tokens DESC;

-- Daily usage by model
SELECT
    DATE(started_at) as date,
    model_used,
    COUNT(*) as requests,
    SUM(input_tokens) as total_input,
    SUM(output_tokens) as total_output
FROM sessions
WHERE started_at > datetime('now', '-7 days')
GROUP BY DATE(started_at), model_used;

-- Stats by model with request correlation
SELECT
    s.session_id,
    s.request_id,
    s.model_used,
    st.tokens_per_second,
    st.thinking_percentage
FROM sessions s
JOIN session_stats st ON s.session_id = st.session_id
WHERE st.model_name = 'claude-sonnet-4'
ORDER BY st.tokens_per_second DESC;
```

## JSONL Format

Each session creates a file: `~/.lunaroute/sessions/YYYY-MM-DD/session-id.jsonl`

Each line is a JSON event:
```json
{"type":"started","session_id":"abc-123","request_id":"req-456","timestamp":"2025-10-01T12:00:00Z","model_requested":"gpt-4",...}
{"type":"response_recorded","session_id":"abc-123","request_id":"req-456","tokens":{"input_tokens":100,"output_tokens":200},...}
{"type":"completed","session_id":"abc-123","request_id":"req-456","success":true,...}
```

### Query with jq

```bash
# Find sessions with high thinking tokens
jq 'select(.type == "completed" and .total_thinking > 10000)' \
  ~/.lunaroute/sessions/2025-10-01/*.jsonl

# Extract all session IDs
jq -r '.session_id' ~/.lunaroute/sessions/2025-10-01/*.jsonl | sort -u

# Count events by type
jq -r '.type' ~/.lunaroute/sessions/2025-10-01/*.jsonl | sort | uniq -c
```

## Feature Flags

### Building with SQLite Support

```toml
[dependencies]
lunaroute-session = { path = "../lunaroute-session", features = ["sqlite-writer"] }
```

### Building without SQLite (JSONL only)

```toml
[dependencies]
lunaroute-session = { path = "../lunaroute-session" }
```

## Performance

### Batching
- Default: 100 events or 100ms timeout
- Reduces I/O operations by batching writes
- Configurable via `worker.batch_size` and `worker.batch_timeout_ms`

### Overhead
- Event publishing: < 1μs (fire-and-forget)
- Background worker: Non-blocking, dedicated Tokio task
- File I/O: Batched and flushed asynchronously
- SQLite: WAL mode for concurrent reads during writes

### Resource Usage
- **Channel buffer**: Bounded to 10,000 events (prevents OOM)
- **Backpressure**: Events dropped with warning when buffer full
- **Database connections**: Pooled (default 5 connections)
- **File I/O**: Files opened per-write, no handle caching

### Security Features
- **Path traversal protection**: Session IDs sanitized (alphanumeric, `-`, `_` only)
- **SQL injection prevention**: Parameterized queries via SQLx
- **Bounded resources**: Channel size limit prevents memory exhaustion
- **Graceful shutdown**: Proper cleanup with `shutdown()` method

## Migration from V1

The new async system coexists with the existing `FileSessionRecorder`:

1. **Enable V2 in config**: Add `session_recording` section
2. **Choose writers**: Enable JSONL and/or SQLite
3. **Update integration**: Use `build_from_config()` in server startup
4. **Test both systems**: V1 and V2 can run simultaneously during migration
5. **Deprecate V1**: Remove old `FileSessionRecorder` once V2 is stable

## Graceful Shutdown

To ensure all pending events are flushed before shutdown:

```rust
// Consume the recorder and wait for flush
if let Some(recorder) = async_recorder {
    recorder.shutdown().await?;
}
```

If the recorder is dropped without calling `shutdown()`, a warning will be logged and pending events may not be fully flushed.

## Troubleshooting

### No events recorded
- Check `session_recording.enabled = true`
- Verify at least one writer is enabled
- Check directory/database permissions

### Events being dropped
```
WARN Session recording buffer full, dropping event
```
- Recording is falling behind
- Increase `worker.channel_buffer_size` (default: 10,000)
- Increase `worker.batch_size` for more efficient writes
- Check for slow I/O (disk performance, database locks)

### Schema version mismatch
```
Error: Unsupported schema version: 2
```
- Migration required, current version is 1
- Do not modify schema manually

### Database locked errors
- Check `sqlite.max_connections` (increase if needed)
- Verify WAL mode is enabled (automatic)
- Check for long-running queries

### Path traversal attempts logged
Session IDs with special characters are automatically sanitized. Check logs for patterns like:
- `../../../` - Path traversal attempt
- `/absolute/path` - Absolute path attempt
- Session ID will be sanitized to alphanumeric + `-` + `_` only
