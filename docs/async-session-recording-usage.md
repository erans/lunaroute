# Async Multi-Writer Session Recording - Usage Guide

## Overview

The async multi-writer session recording system provides:
- **Zero request blocking**: All recording happens in background workers
- **Dual storage**: JSONL files for complete data + SQLite for queryable metadata
- **Configurable writers**: Enable/disable JSONL and SQLite independently
- **Batched writes**: Efficient I/O with configurable batch sizes
- **Schema versioning**: SQLite schema version tracking (v1)
- **Disk space management**: Retention policies with automatic cleanup and compression
- **Advanced search**: Powerful filtering, pagination, and analytics via SQLite

## Session ID Handling

### Automatic Session ID Extraction

LunaRoute automatically extracts session IDs from Anthropic API metadata to group related requests:

**Anthropic API Format:**
```json
{
  "model": "claude-sonnet-4",
  "messages": [...],
  "metadata": {
    "user_id": "user_<hash>_account_<uuid>_session_<session_id>"
  }
}
```

The session ID is parsed from after the `_session_` marker. For example:
- `user_abc123_account_def456_session_550e8400` → session ID: `550e8400`
- All requests with the same session ID are grouped in one JSONL file

**Security Validation:**
Session IDs are validated at multiple layers (ingress, storage, database) to prevent path traversal attacks:
- Must be alphanumeric with hyphens (`-`) and underscores (`_`) only
- Maximum 255 characters
- Rejects `..`, `/`, `\`, and other unsafe characters

**Fallback Behavior:**
If no valid session ID is found in metadata, a new UUID is generated for each request.

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

    # Retention policy (disk space management)
    retention:
      max_age_days: 30              # Delete sessions older than 30 days
      max_total_size_gb: 10         # Enforce 10GB disk quota
      compress_after_days: 7        # Compress sessions older than 7 days (Zstd level 3)
      cleanup_interval_minutes: 60  # Run cleanup every hour

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

**Important:** To use SQLite recording, you must enable the `sqlite-writer` feature:

```toml
# In your Cargo.toml
lunaroute-session = { path = "../lunaroute-session", features = ["sqlite-writer"] }
```

Without this feature, SQLite configuration will be ignored even if enabled in config.

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
    is_streaming: bool,  // NEW: Indicates if this is a streaming request
    metadata: SessionMetadata,
}
```

### StreamStarted (NEW - Streaming Only)
Records the time-to-first-token for streaming requests:
```rust
SessionEvent::StreamStarted {
    session_id: String,
    request_id: String,
    timestamp: DateTime<Utc>,
    time_to_first_token_ms: u64,  // Time from request to first chunk
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
        // NEW: Streaming fields
        is_streaming: bool,
        chunk_count: Option<u32>,
        streaming_duration_ms: Option<u64>,
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
    final_stats: FinalSessionStats {
        total_duration_ms: u64,
        provider_time_ms: u64,
        proxy_overhead_ms: f64,
        total_tokens: TokenTotals { /* ... */ },
        tool_summary: ToolUsageSummary { /* ... */ },
        performance: PerformanceMetrics { /* ... */ },
        // NEW: Streaming statistics (only present for streaming requests)
        streaming_stats: Option<StreamingStats>,
        estimated_cost: Option<CostEstimate>,
    },
}
```

### StreamingStats (NEW)
Detailed streaming performance metrics:
```rust
pub struct StreamingStats {
    pub time_to_first_token_ms: u64,     // TTFT - critical UX metric
    pub total_chunks: u32,                // Total number of SSE chunks
    pub streaming_duration_ms: u64,       // Total time from first to last chunk
    pub avg_chunk_latency_ms: f64,        // Average time between chunks
    pub p50_chunk_latency_ms: Option<u64>, // Median chunk latency
    pub p95_chunk_latency_ms: Option<u64>, // 95th percentile
    pub p99_chunk_latency_ms: Option<u64>, // 99th percentile
    pub max_chunk_latency_ms: u64,        // Maximum chunk latency
    pub min_chunk_latency_ms: u64,        // Minimum chunk latency
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
- NEW Streaming fields: `is_streaming`, `time_to_first_token_ms`, `chunk_count`, `streaming_duration_ms`
- Indexes:
  - `idx_sessions_created`: `created_at DESC`
  - `idx_sessions_provider`: `provider, created_at DESC`
  - `idx_sessions_model`: `model_used, created_at DESC`
  - `idx_sessions_request_id`: `request_id`
  - `idx_sessions_provider_model`: `provider, model_used, created_at DESC`
  - `idx_sessions_streaming`: `is_streaming, created_at DESC` (NEW)

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

**stream_metrics** (NEW): Detailed streaming performance analytics
- Includes: `session_id`, `request_id`, `time_to_first_token_ms`, `total_chunks`, `streaming_duration_ms`
- Latency metrics: `avg_chunk_latency_ms`, `p50/p95/p99_chunk_latency_ms`, `max/min_chunk_latency_ms`
- Indexes:
  - `idx_stream_metrics_session`: `session_id`
  - `idx_stream_metrics_ttft`: `time_to_first_token_ms`
  - `idx_stream_metrics_chunks`: `total_chunks DESC`

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

-- Streaming performance analysis (NEW)
SELECT
    s.session_id,
    s.model_used,
    sm.time_to_first_token_ms,
    sm.total_chunks,
    sm.avg_chunk_latency_ms,
    sm.p95_chunk_latency_ms
FROM sessions s
JOIN stream_metrics sm ON s.session_id = sm.session_id
WHERE s.is_streaming = 1
ORDER BY sm.time_to_first_token_ms ASC
LIMIT 10;

-- Slow TTFT detection
SELECT
    session_id,
    model_used,
    time_to_first_token_ms,
    total_chunks
FROM sessions
WHERE is_streaming = 1
  AND time_to_first_token_ms > 1000  -- TTFT > 1 second
ORDER BY time_to_first_token_ms DESC;

-- Streaming vs non-streaming comparison
SELECT
    is_streaming,
    COUNT(*) as request_count,
    AVG(provider_latency_ms) as avg_latency,
    AVG(input_tokens) as avg_input_tokens,
    AVG(output_tokens) as avg_output_tokens
FROM sessions
WHERE started_at > datetime('now', '-24 hours')
GROUP BY is_streaming;
```

## JSONL Format

Each session creates a file: `~/.lunaroute/sessions/YYYY-MM-DD/session-id.jsonl`

Each line is a JSON event:

**Non-streaming session:**
```json
{"type":"started","session_id":"abc-123","request_id":"req-456","is_streaming":false,...}
{"type":"response_recorded","session_id":"abc-123","request_id":"req-456","tokens":{...},...}
{"type":"completed","session_id":"abc-123","request_id":"req-456","success":true,...}
```

**Streaming session (NEW):**
```json
{"type":"started","session_id":"stream-789","request_id":"req-012","is_streaming":true,...}
{"type":"stream_started","session_id":"stream-789","request_id":"req-012","time_to_first_token_ms":150,...}
{"type":"completed","session_id":"stream-789","request_id":"req-012","streaming_stats":{"time_to_first_token_ms":150,"total_chunks":42,"p95_chunk_latency_ms":200,...},...}
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

# Find streaming sessions with slow TTFT (NEW)
jq 'select(.type == "stream_started" and .time_to_first_token_ms > 500)' \
  ~/.lunaroute/sessions/2025-10-01/*.jsonl

# Analyze streaming performance (NEW)
jq 'select(.type == "completed" and .streaming_stats != null) |
    {session_id, ttft: .streaming_stats.time_to_first_token_ms,
     chunks: .streaming_stats.total_chunks,
     p95: .streaming_stats.p95_chunk_latency_ms}' \
  ~/.lunaroute/sessions/2025-10-01/*.jsonl

# Compare streaming vs non-streaming latencies (NEW)
jq 'select(.type == "started") | {is_streaming, model_requested}' \
  ~/.lunaroute/sessions/2025-10-01/*.jsonl | \
  jq -s 'group_by(.is_streaming) | map({streaming: .[0].is_streaming, count: length})'
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

**Production-Ready Hardening** ✅
- **Path traversal protection**:
  - Session IDs sanitized (alphanumeric, `-`, `_` only)
  - Date directory validation (YYYY-MM-DD format only)
  - Rejects `.`, `..`, and invalid directory names
- **SQL injection prevention**:
  - Parameterized queries via SQLx
  - LIKE pattern metacharacter escaping (`%`, `_`, `\`)
- **Input validation**:
  - Text search: 1000 character limit
  - Filter arrays: 100 items maximum
  - Individual strings: 256 characters maximum
  - Progressive page size limits: 1000/500/100 based on query complexity
- **Timezone validation**:
  - UTC enforcement via `DateTime<Utc>` type
  - 10-year boundary checks (past/future)
- **Integer overflow protection**: Clamped age calculations to u32::MAX
- **Database index**: Started_at column indexed for time range queries
- **Bounded resources**: Channel size limit prevents memory exhaustion
- **Graceful shutdown**: Proper cleanup with `shutdown()` method

**Test Coverage**
- 74 tests passing with sqlite-writer feature
- 60 tests passing without sqlite-writer
- 22 new security validation test cases
- 100% coverage of critical security paths

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

## Streaming Support

The async session recording system fully supports streaming requests with comprehensive metrics:

### What's Recorded

**Time-to-First-Token (TTFT)**
- Critical UX metric: time from request to first SSE chunk
- Recorded in `StreamStarted` event and `streaming_stats.time_to_first_token_ms`
- Indexed for fast queries on slow TTFT detection

**Chunk Metrics**
- Total chunk count
- Individual chunk latencies
- Percentile analysis (P50, P95, P99)
- Min/max chunk latencies
- Average chunk latency

**Streaming Duration**
- Total time from first to last chunk
- Automatically calculated: `total_duration_ms - time_to_first_token_ms`

### Event Flow for Streaming

1. **Request starts** → `Started` event with `is_streaming: true`
2. **First chunk received** → `StreamStarted` event with TTFT
3. **Stream completes** → `Completed` event with full `streaming_stats`

### Performance Considerations

**Zero-Copy Passthrough**
- Anthropic→Anthropic and OpenAI→OpenAI streaming use passthrough mode
- SSE events forwarded directly to client
- Metrics extracted without buffering full response
- Minimal latency overhead (< 1ms per chunk)

**Chunk Tracking**
- Each SSE chunk latency measured in real-time
- Percentiles calculated on stream completion
- Memory-bounded: 10,000 chunk latency cap, 1MB text accumulation limit
- Warning logs when memory bounds reached (once per session)
- Graceful degradation if limits exceeded

### Example: Analyzing Streaming Performance

```sql
-- Find sessions with slow TTFT (> 500ms)
SELECT session_id, model_used, time_to_first_token_ms, total_chunks
FROM sessions s
JOIN stream_metrics sm ON s.session_id = sm.session_id
WHERE time_to_first_token_ms > 500
ORDER BY time_to_first_token_ms DESC;

-- Identify inconsistent chunk latencies (high P99/P50 ratio)
SELECT
    session_id,
    model_used,
    p50_chunk_latency_ms,
    p99_chunk_latency_ms,
    (p99_chunk_latency_ms * 1.0 / NULLIF(p50_chunk_latency_ms, 0)) as p99_p50_ratio
FROM stream_metrics
WHERE p50_chunk_latency_ms > 0
ORDER BY p99_p50_ratio DESC
LIMIT 20;
```

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

### Streaming memory bounds warnings
```
WARN Chunk latency array reached maximum size (10000 entries), dropping further measurements
WARN Accumulated text reached maximum size (1000000 bytes), dropping further content
```
- Indicates extremely long streaming session (10K+ chunks or 1MB+ text)
- **Impact**: Metrics collection continues but additional chunks not tracked
- **Normal behavior**: Protection against OOM, stream continues unaffected
- **Action**: If frequent, investigate why sessions are so long
  - Check for infinite loops in streaming responses
  - Review client streaming timeout configurations
  - Consider if this is expected behavior for your use case
- **Note**: Final statistics still recorded with data up to the limit

## Disk Space Management

### Retention Policies

Automatic disk space management prevents storage exhaustion with configurable retention policies:

**Age-Based Deletion**
```yaml
retention:
  max_age_days: 30  # Delete sessions older than 30 days
```

**Size-Based Quota**
```yaml
retention:
  max_total_size_gb: 10  # Enforce 10GB total storage limit
```
When quota exceeded, oldest sessions are deleted first.

**Automatic Compression**
```yaml
retention:
  compress_after_days: 7  # Compress sessions older than 7 days
```
- Uses Zstd compression (level 3)
- Original `.jsonl` replaced with `.jsonl.zst`
- Typically 70-90% size reduction
- Transparent decompression for queries

**Cleanup Schedule**
```yaml
retention:
  cleanup_interval_minutes: 60  # Run cleanup every hour
```

### Cleanup Algorithm

1. **Delete old sessions**: Remove sessions older than `max_age_days`
2. **Compress aging sessions**: Compress sessions older than `compress_after_days` (skip already compressed)
3. **Enforce quota**: If total size exceeds `max_total_size_gb`, delete oldest sessions until under limit

### Manual Cleanup

```rust
use lunaroute_session::{execute_cleanup, RetentionPolicy};

let policy = RetentionPolicy {
    max_age_days: Some(30),
    max_total_size_gb: Some(10),
    compress_after_days: Some(7),
    cleanup_interval_minutes: 60,
};

let stats = execute_cleanup(Path::new("~/.lunaroute/sessions"), &policy).await?;

println!("Deleted: {} sessions, {} bytes freed",
    stats.sessions_deleted, stats.bytes_freed);
println!("Compressed: {} sessions, {} bytes saved",
    stats.sessions_compressed, stats.bytes_saved);
```

### Background Cleanup Task

Spawn automatic cleanup task:

```rust
use lunaroute_session::spawn_cleanup_task;
use std::sync::Arc;

let task = spawn_cleanup_task(
    PathBuf::from("~/.lunaroute/sessions"),
    Arc::new(policy),
);

// ... application runs ...

// Graceful shutdown
task.shutdown().await;
```

### Disk Usage Monitoring

```rust
use lunaroute_session::calculate_disk_usage;

let usage = calculate_disk_usage(Path::new("~/.lunaroute/sessions")).await?;

println!("Total sessions: {}", usage.session_count);
println!("Compressed: {}", usage.compressed_count);
println!("Total size: {} bytes ({} GB)",
    usage.total_bytes,
    usage.total_bytes / 1_073_741_824
);
```

## Advanced Search & Filtering

### SQLite Search API

The SQLite writer provides powerful search and analytics capabilities:

```rust
use lunaroute_session::{SqliteWriter, SessionFilter, SortOrder};

let writer = SqliteWriter::new("~/.lunaroute/sessions.db").await?;

// Build complex filter
let filter = SessionFilter::builder()
    .time_range(hour_ago, now)
    .providers(vec!["openai".to_string()])
    .models(vec!["gpt-4".to_string()])
    .success(true)
    .min_tokens(1000)
    .streaming(true)
    .page_size(50)
    .page(0)
    .sort(SortOrder::HighestTokens)
    .build()?;

// Search sessions
let results = writer.search_sessions(&filter).await?;

println!("Found {} sessions (page 1 of {})",
    results.items.len(), results.total_pages);

for session in results.items {
    println!("{}: {} tokens, {}ms",
        session.session_id,
        session.total_tokens,
        session.total_duration_ms.unwrap_or(0)
    );
}
```

### Filter Options

**Time Range**
```rust
.time_range(start: DateTime<Utc>, end: DateTime<Utc>)
```

**Provider & Model**
```rust
.providers(vec!["openai".to_string(), "anthropic".to_string()])
.models(vec!["gpt-4".to_string(), "claude-sonnet-4".to_string()])
```

**Token & Duration Ranges**
```rust
.min_tokens(100)
.max_tokens(10000)
.min_duration_ms(100)
.max_duration_ms(30000)
```

**Status Filters**
```rust
.success(true)         // Only successful sessions
.streaming(true)       // Only streaming sessions
```

**Client & Finish Reason**
```rust
.client_ips(vec!["192.168.1.1".to_string()])
.finish_reasons(vec!["end_turn".to_string(), "max_tokens".to_string()])
```

**Full-Text Search**
```rust
.text_search("error handling".to_string())  // Search request/response text (max 1000 chars)
```

**Specific IDs**
```rust
.request_ids(vec!["req-123".to_string()])
.session_ids(vec!["session-456".to_string()])
```

### Input Validation

All filter parameters are validated when calling `.build()`:

**Length Limits** (Security: prevent memory exhaustion)
- Text search: 1000 characters maximum
- Filter arrays: 100 items maximum each
- Individual strings: 256 characters maximum

**Range Validation**
- `min_tokens ≤ max_tokens`
- `min_duration_ms ≤ max_duration_ms`
- `start time < end time`

**Timezone Validation**
- Time ranges must be in UTC
- Bounded to ±10 years from current time

**Page Size Limits**
- Must be > 0
- Maximum varies by query complexity (see Pagination section)

**Example Validation Error:**
```rust
let result = SessionFilter::builder()
    .text_search("a".repeat(1001))  // Too long!
    .build();

// Returns: Err("text_search exceeds maximum length of 1000")
```

### Sort Orders

```rust
pub enum SortOrder {
    NewestFirst,       // Default: most recent first
    OldestFirst,       // Oldest first
    HighestTokens,     // Highest token count first
    LongestDuration,   // Longest duration first
    ShortestDuration,  // Shortest duration first
}
```

### Pagination

**Progressive Page Size Limits**

The maximum page size is automatically adjusted based on query complexity to prevent resource exhaustion:

- **Simple queries** (≤1 filter): max 1000 results
- **Moderate complexity** (text search or multiple filters): max 500 results
- **High complexity** (text search + large arrays): max 100 results

```rust
let filter = SessionFilter::builder()
    .page_size(50)   // 50 results per page
    .page(0)         // First page (0-indexed)
    .build()?;       // Validates page_size against query complexity

let results = writer.search_sessions(&filter).await?;

println!("Page {} of {}", results.page + 1, results.total_pages);
println!("Total matches: {}", results.total_count);

if results.has_next_page() {
    // Fetch next page
    let next_filter = SessionFilter::builder()
        .page_size(50)
        .page(results.page + 1)
        .build()?;
    let next_results = writer.search_sessions(&next_filter).await?;
}
```

### Session Analytics

Get aggregate statistics for filtered sessions:

```rust
let filter = SessionFilter::builder()
    .time_range(day_ago, now)
    .providers(vec!["openai".to_string()])
    .build()?;

let agg = writer.get_aggregates(&filter).await?;

println!("Total sessions: {}", agg.total_sessions);
println!("Success rate: {:.1}%",
    (agg.successful_sessions as f64 / agg.total_sessions as f64) * 100.0
);
println!("Total tokens: {}", agg.total_tokens);
println!("Avg duration: {:.0}ms", agg.avg_duration_ms);
println!("P95 duration: {}ms", agg.p95_duration_ms.unwrap_or(0));

// Breakdown by provider
for (provider, count) in &agg.sessions_by_provider {
    println!("  {}: {} sessions", provider, count);
}

// Breakdown by model
for (model, count) in &agg.sessions_by_model {
    println!("  {}: {} sessions", model, count);
}
```

### Example: Dashboard Queries

**High Token Sessions**
```rust
let filter = SessionFilter::builder()
    .time_range(day_ago, now)
    .min_tokens(10000)
    .sort(SortOrder::HighestTokens)
    .page_size(20)
    .build()?;
```

**Failed Sessions**
```rust
let filter = SessionFilter::builder()
    .success(false)
    .sort(SortOrder::NewestFirst)
    .build()?;
```

**Slow Streaming Sessions**
```rust
let filter = SessionFilter::builder()
    .streaming(true)
    .min_duration_ms(5000)  // > 5 seconds
    .sort(SortOrder::LongestDuration)
    .build()?;
```

**Provider Comparison**
```rust
// OpenAI sessions
let openai_agg = writer.get_aggregates(
    &SessionFilter::builder()
        .providers(vec!["openai".to_string()])
        .time_range(day_ago, now)
        .build()?
).await?;

// Anthropic sessions
let anthropic_agg = writer.get_aggregates(
    &SessionFilter::builder()
        .providers(vec!["anthropic".to_string()])
        .time_range(day_ago, now)
        .build()?
).await?;

println!("OpenAI avg latency: {:.0}ms", openai_agg.avg_duration_ms);
println!("Anthropic avg latency: {:.0}ms", anthropic_agg.avg_duration_ms);
```
