# Connection Pool Environment Variables

## Overview

All connection pool settings can be configured via environment variables. This is useful for containerized deployments and CI/CD pipelines.

## Environment Variable Naming

Format: `LUNAROUTE_<PROVIDER>_<SETTING>=value`

## OpenAI Provider Pool Settings

```bash
# Request timeout (seconds) - default: 600
export LUNAROUTE_OPENAI_TIMEOUT_SECS=600

# Connection timeout (seconds) - default: 10
export LUNAROUTE_OPENAI_CONNECT_TIMEOUT_SECS=10

# Max idle connections per host - default: 32
export LUNAROUTE_OPENAI_POOL_MAX_IDLE=32

# Idle connection timeout (seconds) - default: 90
export LUNAROUTE_OPENAI_POOL_IDLE_TIMEOUT_SECS=90

# TCP keepalive interval (seconds) - default: 60
export LUNAROUTE_OPENAI_TCP_KEEPALIVE_SECS=60

# Max retries for transient errors - default: 3
export LUNAROUTE_OPENAI_MAX_RETRIES=3

# Enable pool metrics - default: true
export LUNAROUTE_OPENAI_ENABLE_POOL_METRICS=true
```

## Anthropic Provider Pool Settings

```bash
# Request timeout (seconds) - default: 600 (10 min for extended thinking)
export LUNAROUTE_ANTHROPIC_TIMEOUT_SECS=600

# Connection timeout (seconds) - default: 10
export LUNAROUTE_ANTHROPIC_CONNECT_TIMEOUT_SECS=10

# Max idle connections per host - default: 32
export LUNAROUTE_ANTHROPIC_POOL_MAX_IDLE=32

# Idle connection timeout (seconds) - default: 90
export LUNAROUTE_ANTHROPIC_POOL_IDLE_TIMEOUT_SECS=90

# TCP keepalive interval (seconds) - default: 60
export LUNAROUTE_ANTHROPIC_TCP_KEEPALIVE_SECS=60

# Max retries for transient errors - default: 3
export LUNAROUTE_ANTHROPIC_MAX_RETRIES=3

# Enable pool metrics - default: true
export LUNAROUTE_ANTHROPIC_ENABLE_POOL_METRICS=true
```

## Configuration Precedence

Settings are applied in this order (later overrides earlier):

1. **Built-in defaults** (in code)
2. **YAML config file** (`--config config.yaml`)
3. **Environment variables** (highest priority)

## Complete Example

```bash
#!/bin/bash
# Production deployment with optimized pool settings

# OpenAI - standard timeout, higher concurrency
export OPENAI_API_KEY="sk-..."
export LUNAROUTE_OPENAI_TIMEOUT_SECS=300
export LUNAROUTE_OPENAI_POOL_MAX_IDLE=64
export LUNAROUTE_OPENAI_POOL_IDLE_TIMEOUT_SECS=60

# Anthropic - extended timeout for thinking, lower concurrency
export ANTHROPIC_API_KEY="sk-ant-..."
export LUNAROUTE_ANTHROPIC_TIMEOUT_SECS=600
export LUNAROUTE_ANTHROPIC_POOL_MAX_IDLE=16
export LUNAROUTE_ANTHROPIC_POOL_IDLE_TIMEOUT_SECS=90

# Session recording
export LUNAROUTE_ENABLE_SESSION_RECORDING=true
export LUNAROUTE_LOG_LEVEL=info

# Start server
./lunaroute-server
```

## Docker Compose Example

```yaml
version: '3.8'
services:
  lunaroute:
    image: lunaroute:latest
    ports:
      - "8081:8081"
    environment:
      # OpenAI pool config
      OPENAI_API_KEY: ${OPENAI_API_KEY}
      LUNAROUTE_OPENAI_TIMEOUT_SECS: 300
      LUNAROUTE_OPENAI_POOL_MAX_IDLE: 64

      # Anthropic pool config
      ANTHROPIC_API_KEY: ${ANTHROPIC_API_KEY}
      LUNAROUTE_ANTHROPIC_TIMEOUT_SECS: 600
      LUNAROUTE_ANTHROPIC_POOL_MAX_IDLE: 32

      # Logging
      LUNAROUTE_LOG_LEVEL: info
      RUST_LOG: lunaroute=debug
```

## Tuning Guidelines

### High-Traffic Scenario
```bash
# More connections, faster expiry
export LUNAROUTE_OPENAI_POOL_MAX_IDLE=128
export LUNAROUTE_OPENAI_POOL_IDLE_TIMEOUT_SECS=60
export LUNAROUTE_OPENAI_TCP_KEEPALIVE_SECS=30
```

### Long-Running Requests (Extended Thinking)
```bash
# Higher timeout, longer keepalive
export LUNAROUTE_ANTHROPIC_TIMEOUT_SECS=900  # 15 minutes
export LUNAROUTE_ANTHROPIC_TCP_KEEPALIVE_SECS=120
export LUNAROUTE_ANTHROPIC_POOL_IDLE_TIMEOUT_SECS=120
```

### Resource-Constrained Environment
```bash
# Fewer connections, quicker cleanup
export LUNAROUTE_OPENAI_POOL_MAX_IDLE=8
export LUNAROUTE_OPENAI_POOL_IDLE_TIMEOUT_SECS=30
export LUNAROUTE_OPENAI_MAX_RETRIES=1
```

## Monitoring

### Pool Metrics (Currently Not Populated)

⚠️ **IMPORTANT LIMITATION**: Pool metrics are defined but not currently populated in production.

The underlying HTTP client (`reqwest`) doesn't expose connection pool lifecycle events needed to track:
- Connection creation/reuse
- Idle connection counts
- Connection lifetimes

**Metrics Infrastructure Ready:**
- ✅ Metric definitions: `http_pool_connections_created_total`, `http_pool_connections_reused_total`, etc.
- ✅ Recording methods: `record_pool_connection_created()`, etc.
- ✅ Comprehensive test coverage (6 tests)
- ❌ Production instrumentation: Not possible without reqwest changes

**Example metrics (when/if implemented):**

```promql
# Connection reuse ratio (higher is better)
rate(http_pool_connections_reused_total[5m]) /
(rate(http_pool_connections_reused_total[5m]) +
 rate(http_pool_connections_created_total[5m]))

# Connection creation rate (lower is better when traffic is steady)
rate(http_pool_connections_created_total[5m])

# Idle connections gauge
http_pool_connections_idle

# Connection lifetime distribution
histogram_quantile(0.95, rate(http_pool_connection_lifetime_seconds_bucket[5m]))
```

**Options for Future Implementation:**
1. Wait for reqwest to add pool metrics API (upstream feature request needed)
2. Migrate to hyper with custom Connector implementation (major refactoring)
3. Switch to different HTTP client that exposes pool metrics (disruptive)

For now, rely on debug logging and application-level metrics (request latency, success rate) to monitor pool health.

## Debug Logging

Set `RUST_LOG=lunaroute_egress=debug` to see connection pool behavior:

```bash
export RUST_LOG=lunaroute_egress=debug
./lunaroute-server
```

Output:
```
[DEBUG] Creating HTTP client: timeout=300s, pool_max_idle=64, pool_idle_timeout=60s...
[DEBUG] 🔌 Provider 'openai' initiating connection to https://api.openai.com
[DEBUG] ♻️ Provider 'openai' reused connection (2ms) - from pool
```

## Troubleshooting

### Symptom: Requests hang or timeout
**Cause:** Pool idle timeout too long, server closed connections
**Fix:** Reduce `POOL_IDLE_TIMEOUT_SECS` to 60-90s

### Symptom: High connection creation rate
**Cause:** Pool too small or idle timeout too aggressive
**Fix:** Increase `POOL_MAX_IDLE` or `POOL_IDLE_TIMEOUT_SECS`

### Symptom: Memory usage growing
**Cause:** Too many idle connections
**Fix:** Reduce `POOL_MAX_IDLE` or `POOL_IDLE_TIMEOUT_SECS`

### Symptom: "Connection reset by peer" errors
**Cause:** TCP keepalive not working
**Fix:** Reduce `TCP_KEEPALIVE_SECS` to 30-60s

## Implementation Status

✅ **Phase 1 Complete:**
- All settings configurable via YAML
- All settings configurable via environment variables
- Debug logging for connection behavior
- Prometheus metrics infrastructure ready (definitions, tests)

⚠️ **Limitations:**
- Pool metrics not populated (reqwest doesn't expose pool events)
- Static pool config metrics could be added (TODO)

🚧 **Phase 2 (Future):**
- Pool metrics instrumentation (requires upstream changes or HTTP client migration)
- Multi-provider per dialect support
- Per-provider pool override
- Connection rotation strategies
- Advanced health checks
