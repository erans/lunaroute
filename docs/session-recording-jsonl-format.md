# JSONL Session Recording Format

## Overview

Each session is stored as a series of newline-delimited JSON events in a single file. The filename format is `{session_id}.jsonl` and files are organized by date: `~/.lunaroute/sessions/2024-01-20/{session_id}.jsonl`

## Event Types and Examples

### 1. Session Started Event

```json
{
  "type": "started",
  "session_id": "550e8400-e29b-41d4-a716-446655440000",
  "request_id": "req-abc-123",
  "timestamp": "2024-01-20T10:30:45.123Z",
  "model_requested": "claude-3-5-sonnet-20241022",
  "provider": "anthropic",
  "listener": "anthropic",
  "is_streaming": false,
  "metadata": {
    "client_ip": "192.168.1.100",
    "user_agent": "Claude Code/1.0",
    "api_version": "2023-06-01",
    "request_headers": {
      "anthropic-version": "2023-06-01",
      "anthropic-beta": "extended-thinking-2024-01-01"
    },
    "session_tags": ["production", "passthrough", "claude-code"]
  }
}
```

### 2. Request Recorded Event

```json
{
  "type": "request_recorded",
  "session_id": "550e8400-e29b-41d4-a716-446655440000",
  "timestamp": "2024-01-20T10:30:45.125Z",
  "request_text": "Write a function that implements binary search in Python",
  "request_json": {
    "model": "claude-3-5-sonnet-20241022",
    "messages": [
      {
        "role": "user",
        "content": "Write a function that implements binary search in Python"
      }
    ],
    "max_tokens": 4096,
    "temperature": 0
  },
  "estimated_tokens": 12,
  "stats": {
    "pre_processing_ms": 0.08,
    "request_size_bytes": 234,
    "message_count": 1,
    "has_system_prompt": false,
    "has_tools": false,
    "tool_count": 0
  }
}
```

### 3. Response Recorded Event (with Tool Calls)

```json
{
  "type": "response_recorded",
  "session_id": "550e8400-e29b-41d4-a716-446655440000",
  "timestamp": "2024-01-20T10:30:48.652Z",
  "response_text": "I'll write a binary search function in Python with proper documentation and error handling.\n\nHere's the implementation:",
  "response_json": {
    "id": "msg_01XYZ",
    "model": "claude-3-5-sonnet-20241022",
    "content": [
      {
        "type": "text",
        "text": "I'll write a binary search function in Python with proper documentation and error handling.\n\nHere's the implementation:"
      },
      {
        "type": "tool_use",
        "id": "toolu_01ABC",
        "name": "Write",
        "input": {
          "file_path": "binary_search.py",
          "content": "def binary_search(arr, target):\n    ..."
        }
      }
    ],
    "usage": {
      "input_tokens": 12,
      "output_tokens": 245,
      "thinking_tokens": 15420,
      "cache_creation_input_tokens": 0,
      "cache_read_input_tokens": 0
    }
  },
  "model_used": "claude-3-5-sonnet-20241022",
  "stats": {
    "provider_latency_ms": 3527,
    "post_processing_ms": 0.12,
    "total_proxy_overhead_ms": 0.20,
    "tokens": {
      "input_tokens": 12,
      "output_tokens": 245,
      "total_tokens": 15677,
      "thinking_tokens": 15420,
      "reasoning_tokens": null,
      "cache_read_tokens": null,
      "cache_creation_tokens": null,
      "audio_input_tokens": null,
      "audio_output_tokens": null,
      "thinking_percentage": 98.4,
      "tokens_per_second": 69.5
    },
    "tool_calls": [
      {
        "tool_name": "Write",
        "tool_call_id": "toolu_01ABC",
        "execution_time_ms": null,
        "input_size_bytes": 1024,
        "output_size_bytes": null,
        "success": null
      }
    ],
    "response_size_bytes": 2048,
    "content_blocks": 2,
    "has_refusal": false
  }
}
```

### 4. Stats Snapshot Event (for long-running sessions)

```json
{
  "type": "stats_snapshot",
  "session_id": "550e8400-e29b-41d4-a716-446655440000",
  "timestamp": "2024-01-20T10:31:00.000Z",
  "stats": {
    "request_count": 5,
    "total_input_tokens": 1250,
    "total_output_tokens": 8420,
    "total_thinking_tokens": 45230,
    "total_tool_calls": 12,
    "unique_tools": ["Read", "Write", "Edit", "Bash"],
    "cumulative_latency_ms": 18523,
    "cumulative_proxy_overhead_ms": 2.4
  }
}
```

### 5. Session Completed Event

```json
{
  "type": "completed",
  "session_id": "550e8400-e29b-41d4-a716-446655440000",
  "timestamp": "2024-01-20T10:31:15.789Z",
  "success": true,
  "error": null,
  "finish_reason": "end_turn",
  "final_stats": {
    "total_duration_ms": 30664,
    "provider_time_ms": 30500,
    "proxy_overhead_ms": 164,
    "total_tokens": {
      "input": 1250,
      "output": 8420,
      "thinking": 45230,
      "cached": 0,
      "total": 54900,
      "by_model": {
        "claude-3-5-sonnet-20241022": {
          "input_tokens": 1250,
          "output_tokens": 8420,
          "total_tokens": 54900,
          "thinking_tokens": 45230,
          "reasoning_tokens": 0,
          "cache_read_tokens": 0,
          "cache_creation_tokens": 0,
          "audio_input_tokens": 0,
          "audio_output_tokens": 0,
          "thinking_percentage": 82.4,
          "tokens_per_second": 276.2
        }
      }
    },
    "tool_summary": {
      "total_calls": 12,
      "unique_tools": 4,
      "by_tool": {
        "Read": {
          "call_count": 5,
          "total_execution_time_ms": 45,
          "avg_execution_time_ms": 9,
          "error_count": 0
        },
        "Write": {
          "call_count": 3,
          "total_execution_time_ms": 28,
          "avg_execution_time_ms": 9,
          "error_count": 0
        },
        "Edit": {
          "call_count": 3,
          "total_execution_time_ms": 32,
          "avg_execution_time_ms": 11,
          "error_count": 0
        },
        "Bash": {
          "call_count": 1,
          "total_execution_time_ms": 523,
          "avg_execution_time_ms": 523,
          "error_count": 0
        }
      },
      "total_tool_time_ms": 628,
      "tool_error_count": 0
    },
    "performance": {
      "avg_provider_latency_ms": 6100,
      "p50_latency_ms": 3200,
      "p95_latency_ms": 12500,
      "p99_latency_ms": 14200,
      "max_latency_ms": 14500,
      "min_latency_ms": 1200,
      "avg_pre_processing_ms": 0.08,
      "avg_post_processing_ms": 0.12,
      "proxy_overhead_percentage": 0.54
    },
    "estimated_cost": {
      "provider": "anthropic",
      "model": "claude-3-5-sonnet-20241022",
      "input_cost_usd": 0.00375,
      "output_cost_usd": 0.0126,
      "thinking_cost_usd": 0.00452,
      "total_cost_usd": 0.02087,
      "cost_per_1k_tokens": 0.38
    }
  }
}
```

### 6. Error Event

```json
{
  "type": "error",
  "session_id": "550e8400-e29b-41d4-a716-446655440000",
  "timestamp": "2024-01-20T10:30:47.456Z",
  "error_type": "rate_limit",
  "error_message": "Rate limit exceeded: 429 Too Many Requests",
  "error_details": {
    "retry_after": 30,
    "limit": "50000 tokens/min",
    "reset_at": "2024-01-20T10:31:00Z"
  }
}
```

## Querying JSONL Files

### Using jq

```bash
# Get all stats for a session
jq 'select(.session_id == "550e8400-e29b-41d4-a716-446655440000")' session.jsonl

# Extract final stats only
jq 'select(.type == "completed") | .final_stats' session.jsonl

# Calculate total thinking tokens across all sessions in a day
jq -s 'map(select(.type == "completed")) |
       map(.final_stats.total_tokens.thinking) |
       add' ~/.lunaroute/sessions/2024-01-20/*.jsonl

# Find sessions with high proxy overhead
jq 'select(.type == "completed" and
           .final_stats.proxy_overhead_ms > 100)' \
    ~/.lunaroute/sessions/2024-01-20/*.jsonl

# Tool usage analysis
jq -s 'map(select(.type == "completed")) |
       map(.final_stats.tool_summary) |
       map(.by_tool | to_entries) |
       flatten |
       group_by(.key) |
       map({
         tool: .[0].key,
         total_calls: (map(.value.call_count) | add),
         avg_time: (map(.value.avg_execution_time_ms) | add / length)
       })' ~/.lunaroute/sessions/2024-01-20/*.jsonl

# Find sessions with high thinking token usage
jq 'select(.type == "response_recorded" and
           .stats.tokens.thinking_tokens > 10000) |
    {
      session_id,
      thinking: .stats.tokens.thinking_tokens,
      percentage: .stats.tokens.thinking_percentage,
      model: .model_used
    }' ~/.lunaroute/sessions/2024-01-20/*.jsonl

# Extract request/response pairs with stats
jq 'select(.type == "request_recorded" or .type == "response_recorded") |
    {
      session_id,
      type,
      text: (.request_text // .response_text),
      tokens: .stats.tokens,
      latency: .stats.provider_latency_ms
    }' session.jsonl
```

### Python Analysis

```python
import json
from pathlib import Path
from datetime import datetime
import pandas as pd

def load_session_stats(session_dir: Path):
    """Load all session completion stats into a DataFrame"""
    stats = []

    for jsonl_file in session_dir.glob("*.jsonl"):
        with open(jsonl_file) as f:
            for line in f:
                event = json.loads(line)
                if event["type"] == "completed":
                    # Flatten stats for DataFrame
                    flat_stats = {
                        "session_id": event["session_id"],
                        "timestamp": event["timestamp"],
                        "success": event["success"],
                        "total_duration_ms": event["final_stats"]["total_duration_ms"],
                        "provider_time_ms": event["final_stats"]["provider_time_ms"],
                        "proxy_overhead_ms": event["final_stats"]["proxy_overhead_ms"],
                        "total_tokens": event["final_stats"]["total_tokens"]["total"],
                        "thinking_tokens": event["final_stats"]["total_tokens"]["thinking"],
                        "thinking_pct": event["final_stats"]["total_tokens"]["thinking"] * 100 / event["final_stats"]["total_tokens"]["total"],
                        "tool_calls": event["final_stats"]["tool_summary"]["total_calls"],
                        "unique_tools": event["final_stats"]["tool_summary"]["unique_tools"],
                        "avg_latency_ms": event["final_stats"]["performance"]["avg_provider_latency_ms"],
                        "p95_latency_ms": event["final_stats"]["performance"].get("p95_latency_ms"),
                        "cost_usd": event["final_stats"].get("estimated_cost", {}).get("total_cost_usd"),
                    }
                    stats.append(flat_stats)

    return pd.DataFrame(stats)

# Load and analyze
df = load_session_stats(Path("~/.lunaroute/sessions/2024-01-20").expanduser())

# High thinking token sessions
high_thinking = df[df["thinking_tokens"] > 10000].sort_values("thinking_tokens", ascending=False)
print(f"Sessions with >10k thinking tokens: {len(high_thinking)}")
print(f"Average thinking percentage: {high_thinking['thinking_pct'].mean():.1f}%")

# Tool usage patterns
tool_users = df[df["tool_calls"] > 0]
print(f"Sessions using tools: {len(tool_users)} ({len(tool_users)/len(df)*100:.1f}%)")
print(f"Average tools per session: {tool_users['tool_calls'].mean():.1f}")

# Cost analysis
total_cost = df["cost_usd"].sum()
print(f"Total cost for {len(df)} sessions: ${total_cost:.2f}")
print(f"Average cost per session: ${df['cost_usd'].mean():.4f}")

# Proxy overhead analysis
print(f"Average proxy overhead: {df['proxy_overhead_ms'].mean():.2f}ms")
print(f"95th percentile overhead: {df['proxy_overhead_ms'].quantile(0.95):.2f}ms")
overhead_pct = (df["proxy_overhead_ms"] / df["total_duration_ms"] * 100).mean()
print(f"Overhead as % of total: {overhead_pct:.2f}%")
```

## Benefits of This Format

### 1. **Complete Audit Trail**
- Every event is timestamped and ordered
- Full request/response JSON preserved
- Stats captured at multiple points

### 2. **Flexible Analysis**
- Can reconstruct entire session timeline
- Stats available at event level and session level
- Easy to extract specific metrics with jq

### 3. **Space Efficient**
- Stats add minimal overhead (~1KB per event)
- Compression (zstd) reduces files by 80-90%
- Can archive old files to S3/GCS

### 4. **Performance Insights**
- Pre/post processing times separated
- Token breakdown including thinking
- Tool execution metrics
- Latency percentiles

### 5. **Cost Tracking**
- Estimated costs per session
- Token usage by model
- Cost per 1K tokens

## Storage Estimates

Typical event sizes:
- Started event: ~500 bytes
- Request event: 1-5 KB (depends on prompt size)
- Response event: 2-10 KB (depends on response size)
- Stats snapshot: ~300 bytes
- Completed event: ~2 KB

Average session (5 requests):
- Uncompressed: ~30-50 KB
- Compressed (zstd): ~5-8 KB

Storage for 100K sessions/day:
- Uncompressed: ~3-5 GB/day
- Compressed: ~500-800 MB/day
- Monthly: ~15-25 GB compressed

## Migration Strategy

### From Current Format to Enhanced Format

```python
def migrate_session(old_jsonl_path: Path, new_jsonl_path: Path):
    """Migrate old format to new format with stats"""

    with open(old_jsonl_path) as f_in, open(new_jsonl_path, 'w') as f_out:
        session_stats = {}

        for line in f_in:
            event = json.loads(line)

            # Enhance with stats
            if event["type"] == "request_recorded":
                event["stats"] = {
                    "pre_processing_ms": 0.1,  # Estimate
                    "request_size_bytes": len(json.dumps(event.get("request_json", {}))),
                    "message_count": len(event.get("request_json", {}).get("messages", [])),
                    "has_system_prompt": False,
                    "has_tools": False,
                    "tool_count": 0
                }

            elif event["type"] == "response_recorded":
                # Add token stats
                usage = event.get("response_json", {}).get("usage", {})
                event["stats"] = {
                    "provider_latency_ms": event.get("latency_ms", 0),
                    "post_processing_ms": 0.1,  # Estimate
                    "total_proxy_overhead_ms": 0.2,
                    "tokens": {
                        "input_tokens": usage.get("input_tokens", 0),
                        "output_tokens": usage.get("output_tokens", 0),
                        "thinking_tokens": usage.get("thinking_tokens"),
                        "total_tokens": sum(filter(None, [
                            usage.get("input_tokens", 0),
                            usage.get("output_tokens", 0),
                            usage.get("thinking_tokens", 0)
                        ])),
                        "thinking_percentage": None,
                        "tokens_per_second": None
                    },
                    "tool_calls": [],
                    "response_size_bytes": len(json.dumps(event.get("response_json", {}))),
                    "content_blocks": 1,
                    "has_refusal": False
                }

            elif event["type"] == "completed":
                # Add comprehensive final stats
                event["final_stats"] = {
                    "total_duration_ms": event.get("total_duration_ms", 0),
                    "provider_time_ms": event.get("total_duration_ms", 0) * 0.99,
                    "proxy_overhead_ms": event.get("total_duration_ms", 0) * 0.01,
                    "total_tokens": session_stats.get("total_tokens", {}),
                    "tool_summary": session_stats.get("tool_summary", {}),
                    "performance": session_stats.get("performance", {}),
                    "estimated_cost": None
                }

            # Write enhanced event
            f_out.write(json.dumps(event) + '\n')
```

## Streaming Sessions

For streaming requests, additional events and metrics are recorded:

### Stream Started Event

Records time-to-first-token (TTFT), a critical UX metric:

```json
{
  "type": "stream_started",
  "session_id": "stream-789",
  "request_id": "req-stream-012",
  "timestamp": "2024-01-20T10:30:45.275Z",
  "time_to_first_token_ms": 150
}
```

### Streaming Statistics in Completed Event

The `Completed` event includes comprehensive streaming statistics:

```json
{
  "type": "completed",
  "session_id": "stream-789",
  "request_id": "req-stream-012",
  "timestamp": "2024-01-20T10:30:48.650Z",
  "success": true,
  "error": null,
  "finish_reason": "end_turn",
  "final_stats": {
    "total_duration_ms": 3375,
    "provider_time_ms": 3275,
    "proxy_overhead_ms": 100.0,
    "total_tokens": {
      "total_input": 50,
      "total_output": 300,
      "total_thinking": 25,
      "total_reasoning": 0,
      "total_cached": 10,
      "total_cache_read": 10,
      "total_cache_creation": 0,
      "total_audio_input": 0,
      "total_audio_output": 0,
      "grand_total": 375,
      "by_model": {}
    },
    "tool_summary": {
      "total_tool_calls": 0,
      "unique_tool_count": 0,
      "by_tool": {},
      "total_tool_time_ms": 0,
      "tool_error_count": 0
    },
    "performance": {
      "avg_provider_latency_ms": 3275.0,
      "p50_latency_ms": 3275,
      "p95_latency_ms": 3275,
      "p99_latency_ms": 3275,
      "max_latency_ms": 3275,
      "min_latency_ms": 3275,
      "avg_pre_processing_ms": 50.0,
      "avg_post_processing_ms": 50.0,
      "proxy_overhead_percentage": 2.96
    },
    "streaming_stats": {
      "time_to_first_token_ms": 150,
      "total_chunks": 28,
      "streaming_duration_ms": 3225,
      "avg_chunk_latency_ms": 115.2,
      "p50_chunk_latency_ms": 110,
      "p95_chunk_latency_ms": 180,
      "p99_chunk_latency_ms": 200,
      "max_chunk_latency_ms": 250,
      "min_chunk_latency_ms": 80
    },
    "estimated_cost": null
  }
}
```

### Streaming Session Flow

A complete streaming session includes these events:

1. **Started** with `is_streaming: true`
2. **StreamStarted** with TTFT (time to first chunk)
3. **Completed** with `streaming_stats` and `final_stats` containing:
   - Time-to-first-token (TTFT)
   - Total chunk count
   - Chunk latency percentiles (P50, P95, P99)
   - Min/max/avg chunk latencies
   - Total streaming duration
   - Token totals (input, output, thinking, cached)
   - Tool usage summary

**Critical Difference from Non-Streaming:**
- Streaming sessions do **NOT** emit `ResponseRecorded` events
- All token counts come from `Completed.final_stats.total_tokens`
- Tool calls come from `Completed.final_stats.tool_summary.by_tool`
- The SQLite writer uses `MAX(COALESCE(existing, 0), new_value)` to handle both session types without double-counting

This approach captures comprehensive streaming performance metrics without recording every individual chunk, keeping file sizes manageable while providing detailed analytics.