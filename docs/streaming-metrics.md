# Streaming Metrics

## Overview

LunaRoute provides comprehensive Prometheus metrics for streaming requests, enabling detailed observability into streaming performance, user experience (TTFT), and resource usage.

## Available Metrics

### 1. Time-to-First-Token (TTFT)

**Metric Name:** `lunaroute_streaming_ttft_seconds`
**Type:** Histogram
**Labels:** `provider`, `model`
**Description:** Time from request to first SSE chunk (critical UX metric)

**Buckets (seconds):** 0.01, 0.05, 0.1, 0.15, 0.2, 0.3, 0.5, 1.0, 2.0, 5.0

**Example Query:**
```promql
# P95 TTFT by model
histogram_quantile(0.95,
  sum(rate(lunaroute_streaming_ttft_seconds_bucket[5m])) by (le, model)
)

# Average TTFT over last hour
rate(lunaroute_streaming_ttft_seconds_sum[1h])
  /
rate(lunaroute_streaming_ttft_seconds_count[1h])
```

### 2. Chunk Latency

**Metric Name:** `lunaroute_streaming_chunk_latency_seconds`
**Type:** Histogram
**Labels:** `provider`, `model`
**Description:** Individual chunk latency (time between consecutive chunks)

**Buckets (seconds):** 0.01, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0

**Sampling:** For streams with >100 chunks, every 10th chunk is sampled to avoid overwhelming Prometheus

**Example Query:**
```promql
# P99 chunk latency (detect slow chunks)
histogram_quantile(0.99,
  sum(rate(lunaroute_streaming_chunk_latency_seconds_bucket[5m])) by (le, model)
)

# Chunk latency variance (P99/P50 ratio)
histogram_quantile(0.99, sum(rate(lunaroute_streaming_chunk_latency_seconds_bucket[5m])) by (le))
  /
histogram_quantile(0.50, sum(rate(lunaroute_streaming_chunk_latency_seconds_bucket[5m])) by (le))
```

### 3. Streaming Requests

**Metric Name:** `lunaroute_streaming_requests_total`
**Type:** Counter
**Labels:** `provider`, `model`
**Description:** Total number of streaming requests completed

**Example Query:**
```promql
# Streaming requests per second by provider
rate(lunaroute_streaming_requests_total[5m])

# Percentage of requests that are streaming
rate(lunaroute_streaming_requests_total[5m])
  /
rate(lunaroute_requests_total[5m]) * 100
```

### 4. Chunk Count

**Metric Name:** `lunaroute_streaming_chunks_total`
**Type:** Histogram
**Labels:** `provider`, `model`
**Description:** Number of chunks per streaming request

**Buckets:** 1, 5, 10, 25, 50, 100, 250, 500, 1000, 5000, 10000

**Example Query:**
```promql
# Average chunks per streaming request
rate(lunaroute_streaming_chunks_total_sum[5m])
  /
rate(lunaroute_streaming_chunks_total_count[5m])

# P95 chunk count (detect very long streams)
histogram_quantile(0.95,
  sum(rate(lunaroute_streaming_chunks_total_bucket[5m])) by (le, model)
)
```

### 5. Streaming Duration

**Metric Name:** `lunaroute_streaming_duration_seconds`
**Type:** Histogram
**Labels:** `provider`, `model`
**Description:** Total streaming duration (first to last chunk)

**Buckets (seconds):** 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0

**Example Query:**
```promql
# P95 streaming duration
histogram_quantile(0.95,
  sum(rate(lunaroute_streaming_duration_seconds_bucket[5m])) by (le, model)
)

# Streams taking longer than 30 seconds
sum(rate(lunaroute_streaming_duration_seconds_bucket{le="30"}[5m])) by (model)
```

### 6. Memory Bounds Hit

**Metric Name:** `lunaroute_streaming_memory_bounds_hit_total`
**Type:** Counter
**Labels:** `provider`, `model`, `bound_type`
**Description:** Number of times streaming memory bounds were hit

**Bound Types:**
- `latency_array`: Chunk latency array reached 10,000 entries
- `text_buffer`: Accumulated text reached 1MB

**Example Query:**
```promql
# Memory bound hits per hour by type
rate(lunaroute_streaming_memory_bounds_hit_total[1h]) * 3600

# Percentage of streams hitting memory bounds
(rate(lunaroute_streaming_memory_bounds_hit_total[5m])
  /
rate(lunaroute_streaming_requests_total[5m])) * 100
```

## Dashboard Examples

### TTFT Dashboard Panel

```yaml
panel:
  title: "Time-to-First-Token (P50, P95, P99)"
  type: graph
  targets:
    - expr: |
        histogram_quantile(0.50,
          sum(rate(lunaroute_streaming_ttft_seconds_bucket[5m])) by (le, model)
        )
      legendFormat: "{{model}} P50"
    - expr: |
        histogram_quantile(0.95,
          sum(rate(lunaroute_streaming_ttft_seconds_bucket[5m])) by (le, model)
        )
      legendFormat: "{{model}} P95"
    - expr: |
        histogram_quantile(0.99,
          sum(rate(lunaroute_streaming_ttft_seconds_bucket[5m])) by (le, model)
        )
      legendFormat: "{{model}} P99"
```

### Streaming Throughput Panel

```yaml
panel:
  title: "Streaming Requests/sec"
  type: graph
  targets:
    - expr: |
        sum(rate(lunaroute_streaming_requests_total[5m])) by (provider, model)
      legendFormat: "{{provider}}/{{model}}"
```

### Memory Bounds Alert

```yaml
alert:
  name: StreamingMemoryBoundsExceeded
  expr: |
    rate(lunaroute_streaming_memory_bounds_hit_total[5m]) > 0.1
  for: 5m
  labels:
    severity: warning
  annotations:
    summary: "Streaming memory bounds frequently exceeded"
    description: "{{ $labels.provider }}/{{ $labels.model }} hitting {{ $labels.bound_type }} limit"
```

## Interpretation Guide

### TTFT (Time-to-First-Token)

**Good:** < 200ms
**Acceptable:** 200-500ms
**Poor:** > 500ms

High TTFT indicates:
- Provider API latency
- Network latency
- Cold start issues

### Chunk Latency

**Good:** < 100ms
**Acceptable:** 100-200ms
**Poor:** > 200ms

High variance (P99/P50 ratio > 3) indicates:
- Inconsistent provider performance
- Network congestion
- Token generation issues

### Chunk Count

**Typical:** 10-100 chunks per request
**Long:** 100-1000 chunks
**Very Long:** > 1000 chunks (may hit memory bounds)

### Memory Bounds Hits

**Expected:** 0 for typical requests
**Investigate if:** > 0.1% of streams hit bounds

Frequent memory bound hits suggest:
- Extremely long streaming responses
- Possible infinite loops in generation
- Need to review client timeout configurations

## Operational Queries

### Detect Degraded Streaming Performance

```promql
# TTFT P95 exceeding 1 second
histogram_quantile(0.95,
  sum(rate(lunaroute_streaming_ttft_seconds_bucket[5m])) by (le)
) > 1.0

# Chunk latency P99 exceeding 500ms
histogram_quantile(0.99,
  sum(rate(lunaroute_streaming_chunk_latency_seconds_bucket[5m])) by (le)
) > 0.5
```

### Compare Streaming vs Non-Streaming Performance

```promql
# Average latency: streaming vs non-streaming
avg(rate(lunaroute_streaming_duration_seconds_sum[5m]) / rate(lunaroute_streaming_duration_seconds_count[5m]))
  by (provider)
  /
avg(rate(lunaroute_request_duration_seconds_sum[5m]) / rate(lunaroute_request_duration_seconds_count[5m]))
  by (provider)
```

### Identify Outliers

```promql
# Streams with > 5000 chunks
sum(increase(lunaroute_streaming_chunks_total_bucket{le="5000"}[1h])) by (model)

# Streams lasting > 60 seconds
sum(increase(lunaroute_streaming_duration_seconds_bucket{le="60"}[1h])) by (model)
```

## Integration with Session Recording

Streaming metrics provide real-time aggregated statistics, while session recording provides per-session details:

- **Prometheus Metrics**: Real-time percentiles, rates, trends
- **Session Recording**: Individual session drill-down, full request/response

Use metrics for alerting and dashboards, session recording for debugging specific issues.

## Implementation Architecture

Streaming metrics are implemented via a shared `StreamingMetricsTracker` module in `lunaroute-ingress/src/streaming_metrics.rs`:

### StreamingMetricsTracker

Centralized tracker for all streaming metrics, eliminating code duplication between OpenAI and Anthropic handlers:

```rust
pub struct StreamingMetricsTracker {
    ttft_time: Arc<Mutex<Option<Instant>>>,
    chunk_count: Arc<Mutex<u32>>,
    chunk_latencies: Arc<Mutex<Vec<u64>>>,
    last_chunk_time: Arc<Mutex<Instant>>,
    accumulated_text: Arc<Mutex<String>>,
    stream_model: Arc<Mutex<Option<String>>>,
    stream_finish_reason: Arc<Mutex<Option<String>>>,
}
```

**Key methods:**
- `new(start_time)` - Create tracker
- `record_ttft(now)` - Record first token time
- `record_chunk_latency(now, provider, model, metrics)` - Track latencies with bounds
- `increment_chunk_count()` - Increment chunk counter
- `accumulate_text(text, provider, model, metrics)` - Accumulate text with bounds
- `set_model(model)` / `set_finish_reason(reason)` - Set metadata
- `finalize(start, before_provider)` - Compute final statistics

### FinalizedStreamingMetrics

Computed statistics after stream completion:

```rust
pub struct FinalizedStreamingMetrics {
    ttft_ms: u64,
    total_chunks: u32,
    streaming_duration_ms: u64,
    total_duration_ms: u64,
    latencies: Vec<u64>,
    p50/p95/p99/max/min/avg: Statistics,
    finish_reason: Option<String>,
}
```

**Key methods:**
- `record_to_prometheus(metrics, provider, model)` - Record all metrics to Prometheus
- `to_streaming_stats()` - Convert to session recording format

### Benefits

- **Code reuse**: ~256 lines eliminated, single source of truth
- **Consistency**: Same metrics computation for all providers
- **Extensibility**: Easy to add new providers (just use the tracker)
- **Testability**: Comprehensive unit tests in shared module (9 tests)
- **Safety**: Memory bounds protection built-in (10K chunks, 1MB text)

## Performance Impact

- **TTFT/Duration/Chunk Count**: Recorded once per streaming request (~3 histogram observations)
- **Chunk Latency**: Sampled at 10% for streams > 100 chunks
- **Memory Bounds**: Only incremented when limits hit (rare)
- **Total Overhead**: < 0.5ms per streaming request

## Memory Bounds

The streaming implementation includes memory protection:

- **Chunk Latency Array**: Capped at 10,000 entries
- **Text Accumulation**: Capped at 1MB

When these limits are hit:
1. Warning is logged once per session
2. `lunaroute_streaming_memory_bounds_hit_total` is incremented
3. Stream continues unaffected
4. Final statistics include data up to the limit

## Future Enhancements

Potential additions under consideration:

1. **Token Generation Rate**: Tokens/second during streaming
2. **Backpressure Metrics**: Client read speed vs generation speed
3. **Content Type Distribution**: Text vs tool call chunks
4. **Streaming Error Rates**: Mid-stream failures vs clean completions
