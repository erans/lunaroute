# Session Recording System Summary

## Enhanced JSONL Format

The session recording system now captures comprehensive statistics alongside request/response data in JSONL files. Each session produces a single file with multiple event types.

## Key Data Points Captured

### Per-Request Stats
- **Pre-processing time**: Time before sending to provider (ms)
- **Post-processing time**: Time after receiving response (ms)
- **Total proxy overhead**: Pre + post processing combined
- **Request size**: Bytes of request payload
- **Message count**: Number of messages in conversation

### Per-Response Stats
- **Provider latency**: Actual API response time
- **Token breakdown**:
  - Input tokens
  - Output tokens
  - Thinking tokens (Anthropic extended thinking)
  - Cache tokens (read/write)
  - Total tokens
  - Thinking percentage of total
  - Tokens per second
- **Tool calls**: Name, ID, execution time, input/output size
- **Response characteristics**: Size, content blocks, refusal detection

### Session-Level Stats (in completed event)
- **Duration metrics**:
  - Total session duration
  - Provider time (waiting for API)
  - Proxy overhead (processing time)
  - Overhead percentage

- **Token totals**:
  - Aggregated by type (input/output/thinking/cached)
  - Breakdown by model (if multiple models used)
  - Cost estimation based on current pricing

- **Tool usage summary**:
  - Total calls and unique tools
  - Per-tool statistics (count, avg time, errors)
  - Total tool execution time

- **Performance metrics**:
  - Latency percentiles (p50, p95, p99)
  - Min/max/average latencies
  - Average pre/post processing times

## File Structure

```
~/.lunaroute/sessions/
├── 2024-01-20/
│   ├── 550e8400-e29b-41d4-a716-446655440000.jsonl
│   ├── 660f9500-f39c-52e5-b827-557766550111.jsonl
│   └── ...
├── 2024-01-21/
│   └── ...
└── sessions.db  (SQLite for metadata/queries, schema v1)
```

## SQLite Schema

- **schema_version** table: Single column tracking schema version (currently 1)
- **sessions** table: Core session metadata with session_id, request_id, model info
- **session_stats** table: Detailed stats per session with session_id, request_id, model_name
- **tool_calls** table: Tool usage tracking with session_id, request_id, model_name
- **stream_events** table: Streaming events with session_id, request_id, model_name
- **daily_stats** table: Aggregated daily statistics

All stats tables include **session_id**, **request_id**, and **model_name** for comprehensive querying.

## Event Flow

1. **Started**: Session initialized with metadata
2. **RequestRecorded**: User request with pre-processing stats
3. **ResponseRecorded**: Assistant response with detailed stats
4. **StatsSnapshot**: (Optional) Periodic stats for long sessions
5. **Completed**: Final comprehensive statistics

## Querying Capabilities

### With jq (command-line)
```bash
# High thinking token usage
jq 'select(.type == "completed" and .final_stats.total_tokens.thinking > 10000)' *.jsonl

# Tool usage analysis
jq '.final_stats.tool_summary.by_tool' *.jsonl | jq -s 'add'

# Cost analysis
jq '.final_stats.estimated_cost.total_cost_usd' *.jsonl | jq -s 'add'
```

### With SQLite
```sql
-- Performance analysis
SELECT session_id, provider_latency_ms, proxy_overhead_ms,
       (proxy_overhead_ms * 100.0 / total_duration_ms) as overhead_pct
FROM sessions
WHERE started_at > datetime('now', '-1 day')
ORDER BY overhead_pct DESC;

-- Tool usage patterns
SELECT tool_name, SUM(call_count) as total_calls
FROM tool_calls
GROUP BY tool_name
ORDER BY total_calls DESC;
```

### With Python
```python
# Load completion stats into DataFrame
df = pd.read_json('session.jsonl', lines=True)
completed = df[df['type'] == 'completed']
stats = pd.json_normalize(completed['final_stats'])

# Analyze
print(f"Average thinking tokens: {stats['total_tokens.thinking'].mean():.0f}")
print(f"Total cost: ${stats['estimated_cost.total_cost_usd'].sum():.2f}")
```

## Storage Requirements

- **Per event**: 0.5-10 KB (varies by type)
- **Per session** (5 requests avg): 30-50 KB uncompressed
- **With zstd compression**: 80-90% reduction
- **100K sessions/day**: ~500-800 MB compressed

## Benefits

1. **Complete observability**: Every metric captured
2. **No request blocking**: Async background recording
3. **Flexible analysis**: JSON, SQL, and DataFrame queries
4. **Cost tracking**: Built-in usage and cost estimation
5. **Performance insights**: Detailed latency breakdowns
6. **Tool analytics**: Usage patterns and performance
7. **Audit compliance**: Full request/response preservation

## Implementation Priority

1. **Phase 1**: Core JSONL writer with stats
2. **Phase 2**: SQLite metadata database
3. **Phase 3**: Query tools and dashboards
4. **Phase 4**: Compression and archival
5. **Phase 5**: Real-time streaming support