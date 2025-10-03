# Session Recording Query Templates

## SQLite Queries for Session Analysis

### 1. Basic Session Queries

```sql
-- Recent sessions with details
SELECT
    session_id,
    model_used,
    datetime(started_at) as started,
    ROUND((total_duration_ms / 1000.0), 2) as duration_secs,
    input_tokens,
    output_tokens,
    thinking_tokens,
    reasoning_tokens,
    cache_read_tokens,
    cache_creation_tokens,
    audio_input_tokens,
    audio_output_tokens,
    success,
    finish_reason
FROM sessions
WHERE started_at > datetime('now', '-1 hour')
ORDER BY started_at DESC
LIMIT 20;

-- Sessions with errors
SELECT
    session_id,
    model_requested,
    model_used,
    error_message,
    datetime(started_at) as started
FROM sessions
WHERE success = 0
    AND started_at > datetime('now', '-24 hours')
ORDER BY started_at DESC;

-- Find a specific session
SELECT *
FROM sessions
WHERE session_id = 'YOUR_SESSION_ID'
    OR request_text LIKE '%search term%'
ORDER BY started_at DESC
LIMIT 10;
```

### 2. Token Usage Analysis

```sql
-- Sessions with high thinking/reasoning tokens
SELECT
    session_id,
    model_used,
    thinking_tokens,
    reasoning_tokens,
    ROUND((COALESCE(thinking_tokens, 0) + COALESCE(reasoning_tokens, 0)) * 100.0 /
          (input_tokens + output_tokens + COALESCE(thinking_tokens, 0) + COALESCE(reasoning_tokens, 0)), 2) as extended_pct,
    cache_read_tokens,
    cache_creation_tokens,
    substr(request_text, 1, 100) as request_preview,
    substr(response_text, 1, 100) as response_preview,
    datetime(started_at) as started
FROM sessions
WHERE (thinking_tokens > 10000 OR reasoning_tokens > 10000)
ORDER BY (COALESCE(thinking_tokens, 0) + COALESCE(reasoning_tokens, 0)) DESC
LIMIT 50;

-- Token usage by model
SELECT
    model_used,
    COUNT(*) as requests,
    AVG(input_tokens) as avg_input,
    AVG(output_tokens) as avg_output,
    AVG(thinking_tokens) as avg_thinking,
    AVG(reasoning_tokens) as avg_reasoning,
    AVG(cache_read_tokens) as avg_cache_read,
    AVG(cache_creation_tokens) as avg_cache_creation,
    SUM(input_tokens) as total_input,
    SUM(output_tokens) as total_output,
    SUM(thinking_tokens) as total_thinking,
    SUM(reasoning_tokens) as total_reasoning,
    SUM(cache_read_tokens) as total_cache_read,
    SUM(cache_creation_tokens) as total_cache_creation,
    SUM(audio_input_tokens) as total_audio_input,
    SUM(audio_output_tokens) as total_audio_output,
    SUM(input_tokens + output_tokens + COALESCE(thinking_tokens, 0) + COALESCE(reasoning_tokens, 0)) as total_tokens
FROM sessions
WHERE started_at > datetime('now', '-7 days')
GROUP BY model_used
ORDER BY total_tokens DESC;

-- Daily token consumption
SELECT
    DATE(started_at) as date,
    COUNT(*) as requests,
    SUM(input_tokens) as input_tokens,
    SUM(output_tokens) as output_tokens,
    SUM(thinking_tokens) as thinking_tokens,
    SUM(reasoning_tokens) as reasoning_tokens,
    SUM(cache_read_tokens) as cache_read_tokens,
    SUM(cache_creation_tokens) as cache_creation_tokens,
    SUM(audio_input_tokens) as audio_input_tokens,
    SUM(audio_output_tokens) as audio_output_tokens,
    SUM(input_tokens + output_tokens + COALESCE(thinking_tokens, 0) + COALESCE(reasoning_tokens, 0)) as total_tokens
FROM sessions
WHERE started_at > datetime('now', '-30 days')
GROUP BY DATE(started_at)
ORDER BY date DESC;
```

### 3. Performance Analysis

```sql
-- Latency percentiles by model
WITH latency_data AS (
    SELECT
        model_used,
        provider_latency_ms,
        ROW_NUMBER() OVER (PARTITION BY model_used ORDER BY provider_latency_ms) as row_num,
        COUNT(*) OVER (PARTITION BY model_used) as total_count
    FROM sessions
    WHERE started_at > datetime('now', '-24 hours')
        AND provider_latency_ms IS NOT NULL
)
SELECT
    model_used,
    MAX(CASE WHEN row_num = CAST(total_count * 0.50 AS INTEGER) THEN provider_latency_ms END) as p50_ms,
    MAX(CASE WHEN row_num = CAST(total_count * 0.90 AS INTEGER) THEN provider_latency_ms END) as p90_ms,
    MAX(CASE WHEN row_num = CAST(total_count * 0.95 AS INTEGER) THEN provider_latency_ms END) as p95_ms,
    MAX(CASE WHEN row_num = CAST(total_count * 0.99 AS INTEGER) THEN provider_latency_ms END) as p99_ms,
    MAX(provider_latency_ms) as max_ms,
    total_count as sample_size
FROM latency_data
GROUP BY model_used, total_count
ORDER BY p95_ms DESC;

-- Hourly performance trends
SELECT
    strftime('%Y-%m-%d %H:00', started_at) as hour,
    COUNT(*) as requests,
    ROUND(AVG(provider_latency_ms), 2) as avg_latency_ms,
    ROUND(MIN(provider_latency_ms), 2) as min_latency_ms,
    ROUND(MAX(provider_latency_ms), 2) as max_latency_ms,
    SUM(CASE WHEN success = 0 THEN 1 ELSE 0 END) as errors
FROM sessions
WHERE started_at > datetime('now', '-24 hours')
GROUP BY hour
ORDER BY hour DESC;

-- Slow requests
SELECT
    session_id,
    model_used,
    provider_latency_ms,
    total_duration_ms,
    input_tokens + output_tokens + COALESCE(thinking_tokens, 0) + COALESCE(reasoning_tokens, 0) as total_tokens,
    substr(request_text, 1, 200) as request_preview,
    datetime(started_at) as started
FROM sessions
WHERE provider_latency_ms > 10000  -- Over 10 seconds
    AND started_at > datetime('now', '-24 hours')
ORDER BY provider_latency_ms DESC
LIMIT 20;
```

### 4. Tool Usage Analysis

```sql
-- Tool usage frequency
SELECT
    t.tool_name,
    COUNT(DISTINCT t.session_id) as unique_sessions,
    SUM(t.call_count) as total_calls,
    ROUND(AVG(t.call_count), 2) as avg_calls_per_session
FROM tool_calls t
JOIN sessions s ON t.session_id = s.session_id
WHERE s.started_at > datetime('now', '-7 days')
GROUP BY t.tool_name
ORDER BY total_calls DESC;

-- Sessions with multiple tool calls
SELECT
    s.session_id,
    s.model_used,
    COUNT(DISTINCT t.tool_name) as unique_tools,
    SUM(t.call_count) as total_tool_calls,
    GROUP_CONCAT(t.tool_name || '(' || t.call_count || ')', ', ') as tools_used,
    s.provider_latency_ms,
    datetime(s.started_at) as started
FROM sessions s
JOIN tool_calls t ON s.session_id = t.session_id
WHERE s.started_at > datetime('now', '-24 hours')
GROUP BY s.session_id
HAVING total_tool_calls > 5
ORDER BY total_tool_calls DESC;

-- Tool combinations
WITH tool_sessions AS (
    SELECT
        s.session_id,
        GROUP_CONCAT(t.tool_name, '+') as tool_combo
    FROM sessions s
    JOIN tool_calls t ON s.session_id = t.session_id
    WHERE s.started_at > datetime('now', '-7 days')
    GROUP BY s.session_id
)
SELECT
    tool_combo,
    COUNT(*) as occurrences
FROM tool_sessions
GROUP BY tool_combo
HAVING COUNT(*) > 1
ORDER BY occurrences DESC
LIMIT 20;
```

### 5. Client Analysis

```sql
-- Requests by client IP
SELECT
    client_ip,
    COUNT(*) as requests,
    COUNT(DISTINCT model_used) as unique_models,
    SUM(input_tokens + output_tokens + COALESCE(thinking_tokens, 0) + COALESCE(reasoning_tokens, 0)) as total_tokens,
    MIN(started_at) as first_seen,
    MAX(started_at) as last_seen
FROM sessions
WHERE client_ip IS NOT NULL
    AND started_at > datetime('now', '-24 hours')
GROUP BY client_ip
ORDER BY requests DESC
LIMIT 50;

-- User agent analysis
SELECT
    CASE
        WHEN user_agent LIKE '%Claude%' THEN 'Claude Code'
        WHEN user_agent LIKE '%python%' THEN 'Python SDK'
        WHEN user_agent LIKE '%node%' THEN 'Node SDK'
        WHEN user_agent LIKE '%curl%' THEN 'curl'
        ELSE 'Other'
    END as client_type,
    COUNT(*) as requests,
    COUNT(DISTINCT client_ip) as unique_ips
FROM sessions
WHERE user_agent IS NOT NULL
    AND started_at > datetime('now', '-24 hours')
GROUP BY client_type
ORDER BY requests DESC;
```

### 6. Content Analysis

```sql
-- Common request patterns
SELECT
    substr(request_text, 1, 50) as request_start,
    COUNT(*) as frequency,
    AVG(output_tokens) as avg_response_tokens,
    AVG(provider_latency_ms) as avg_latency_ms
FROM sessions
WHERE request_text IS NOT NULL
    AND LENGTH(request_text) > 10
    AND started_at > datetime('now', '-24 hours')
GROUP BY request_start
HAVING COUNT(*) > 2
ORDER BY frequency DESC
LIMIT 20;

-- Sessions with long responses
SELECT
    session_id,
    model_used,
    output_tokens,
    LENGTH(response_text) as response_length,
    substr(request_text, 1, 100) as request_preview,
    substr(response_text, 1, 200) as response_preview,
    datetime(started_at) as started
FROM sessions
WHERE output_tokens > 2000
    AND started_at > datetime('now', '-24 hours')
ORDER BY output_tokens DESC
LIMIT 20;
```

### 7. Operational Queries

```sql
-- Database size and table statistics
SELECT
    name as table_name,
    (SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND tbl_name = name) as index_count,
    (SELECT COUNT(*) FROM pragma_table_info(name)) as column_count
FROM sqlite_master
WHERE type = 'table'
ORDER BY name;

-- Storage usage by table
SELECT
    'sessions' as table_name,
    COUNT(*) as row_count,
    ROUND(SUM(LENGTH(request_text) + LENGTH(response_text)) / 1048576.0, 2) as text_mb
FROM sessions
UNION ALL
SELECT
    'tool_calls' as table_name,
    COUNT(*) as row_count,
    0 as text_mb
FROM tool_calls
UNION ALL
SELECT
    'stream_events' as table_name,
    COUNT(*) as row_count,
    ROUND(SUM(LENGTH(content)) / 1048576.0, 2) as text_mb
FROM stream_events;

-- Cleanup old sessions (archive/delete)
-- First, export to JSONL for archiving
SELECT json_object(
    'session_id', session_id,
    'started_at', started_at,
    'model_used', model_used,
    'tokens', json_object(
        'input', input_tokens,
        'output', output_tokens,
        'thinking', thinking_tokens,
        'reasoning', reasoning_tokens,
        'cache_read', cache_read_tokens,
        'cache_creation', cache_creation_tokens,
        'audio_input', audio_input_tokens,
        'audio_output', audio_output_tokens
    ),
    'request_text', request_text,
    'response_text', response_text
) as json_data
FROM sessions
WHERE started_at < datetime('now', '-30 days')
ORDER BY started_at;

-- Then delete old records
DELETE FROM sessions
WHERE started_at < datetime('now', '-30 days');

-- Vacuum to reclaim space
VACUUM;
```

## Query Optimization Tips

### 1. Create Custom Indexes

```sql
-- For frequent text searches
CREATE INDEX idx_sessions_request_text ON sessions(request_text) WHERE request_text IS NOT NULL;

-- For IP-based queries
CREATE INDEX idx_sessions_client_ip_date ON sessions(client_ip, started_at DESC);

-- For model-specific analysis
CREATE INDEX idx_sessions_model_tokens ON sessions(model_used, input_tokens + output_tokens + COALESCE(thinking_tokens, 0) + COALESCE(reasoning_tokens, 0));
```

### 2. Materialized Views for Dashboards

```sql
-- Create a summary table updated periodically
CREATE TABLE session_summary_hourly AS
SELECT
    strftime('%Y-%m-%d %H:00:00', started_at) as hour,
    provider,
    model_used,
    COUNT(*) as request_count,
    SUM(CASE WHEN success = 1 THEN 1 ELSE 0 END) as success_count,
    AVG(provider_latency_ms) as avg_latency,
    SUM(input_tokens) as total_input_tokens,
    SUM(output_tokens) as total_output_tokens,
    SUM(thinking_tokens) as total_thinking_tokens,
    SUM(reasoning_tokens) as total_reasoning_tokens,
    SUM(cache_read_tokens) as total_cache_read_tokens,
    SUM(cache_creation_tokens) as total_cache_creation_tokens,
    SUM(audio_input_tokens) as total_audio_input_tokens,
    SUM(audio_output_tokens) as total_audio_output_tokens
FROM sessions
GROUP BY hour, provider, model_used;

-- Refresh periodically
DELETE FROM session_summary_hourly WHERE hour < datetime('now', '-7 days');
INSERT INTO session_summary_hourly
SELECT ... -- same query as above
WHERE started_at >= datetime('now', '-1 hour')
    AND started_at < datetime('now');
```

### 3. Export Queries for Analysis

```sql
-- Export to CSV for external analysis
.headers on
.mode csv
.output sessions_export.csv

SELECT
    session_id,
    datetime(started_at) as started_at,
    provider,
    model_used,
    success,
    finish_reason,
    total_duration_ms,
    provider_latency_ms,
    input_tokens,
    output_tokens,
    thinking_tokens,
    reasoning_tokens,
    cache_read_tokens,
    cache_creation_tokens,
    audio_input_tokens,
    audio_output_tokens,
    client_ip,
    LENGTH(request_text) as request_length,
    LENGTH(response_text) as response_length
FROM sessions
WHERE started_at > datetime('now', '-7 days')
ORDER BY started_at DESC;

.output stdout
.mode list
```

## Monitoring Queries

### Real-time Health Check

```sql
-- Last 5 minutes activity
SELECT
    COUNT(*) as total_requests,
    SUM(CASE WHEN success = 1 THEN 1 ELSE 0 END) as successful,
    SUM(CASE WHEN success = 0 THEN 1 ELSE 0 END) as failed,
    ROUND(AVG(provider_latency_ms), 2) as avg_latency_ms,
    MAX(provider_latency_ms) as max_latency_ms,
    COUNT(DISTINCT model_used) as unique_models,
    COUNT(DISTINCT client_ip) as unique_clients
FROM sessions
WHERE started_at > datetime('now', '-5 minutes');

-- Error rate by minute
SELECT
    strftime('%Y-%m-%d %H:%M', started_at) as minute,
    COUNT(*) as total,
    SUM(CASE WHEN success = 0 THEN 1 ELSE 0 END) as errors,
    ROUND(SUM(CASE WHEN success = 0 THEN 1 ELSE 0 END) * 100.0 / COUNT(*), 2) as error_rate_pct
FROM sessions
WHERE started_at > datetime('now', '-1 hour')
GROUP BY minute
HAVING error_rate_pct > 5
ORDER BY minute DESC;
```

## JSONL Query Examples

### Using jq for JSONL Analysis

```bash
# Count events by type
jq -r '.type' ~/.lunaroute/sessions/2024-01-20/*.jsonl | sort | uniq -c

# Extract high thinking token sessions
jq 'select(.type == "completed" and .total_thinking_tokens > 10000)' \
    ~/.lunaroute/sessions/2024-01-20/*.jsonl

# Calculate total tokens for a session
jq -s 'map(select(.session_id == "SESSION_ID")) |
    map(select(.type == "completed")) |
    .[0] | .total_input_tokens + .total_output_tokens + .total_thinking_tokens' \
    ~/.lunaroute/sessions/2024-01-20/*.jsonl

# Find sessions with specific tool usage
jq 'select(.type == "tool_call_recorded" and .tool_name == "Read")' \
    ~/.lunaroute/sessions/2024-01-20/*.jsonl | \
    jq -s 'group_by(.session_id) | map({session_id: .[0].session_id, count: length})'

# Export request/response pairs
jq 'select(.type == "request_recorded" or .type == "response_recorded") |
    {session_id, type, timestamp, text: (.request_text // .response_text)}' \
    ~/.lunaroute/sessions/2024-01-20/*.jsonl > pairs.jsonl
```

## Performance Benchmarks

Expected query performance on moderate hardware (SSD, 4 cores):

| Query Type | Row Count | Expected Time |
|------------|-----------|---------------|
| Single session lookup | 1 | < 1ms |
| Recent sessions (indexed) | 100 | < 5ms |
| Daily aggregation | 10,000 | < 50ms |
| Token analysis | 100,000 | < 200ms |
| Full table scan | 1,000,000 | < 2s |

With proper indexing and periodic cleanup, the SQLite database can handle millions of sessions while maintaining sub-second query times for most operations.