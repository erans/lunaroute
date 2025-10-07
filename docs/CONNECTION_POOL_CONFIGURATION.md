# Connection Pool Configuration Specification

**Version:** 1.0
**Status:** Draft
**Last Updated:** 2025-10-07

---

## Overview

This document specifies the connection pool configuration system for LunaRoute, supporting:
- **Default behavior**: Shared pools per dialect (efficient resource usage)
- **Override capability**: Provider-specific pools when needed (performance tuning)
- **Full configurability**: All settings exposed via YAML and environment variables

### Design Principles

1. **Simplicity by default** - Most providers share dialect-level pool settings
2. **Flexibility when needed** - Any provider can override with dedicated pool
3. **Clear mental model** - Presence of `http_client` on provider = dedicated pool
4. **Resource efficient** - Only create separate pools when explicitly configured

---

## Configuration Structure

### Layered Configuration Model

```
┌─────────────────────────────────────┐
│   Dialect-Level Pool Config         │
│   (Shared by all providers)         │
│                                     │
│   ┌─────────────────────────────┐  │
│   │  Provider 1 (inherits)      │  │
│   └─────────────────────────────┘  │
│                                     │
│   ┌─────────────────────────────┐  │
│   │  Provider 2 (OVERRIDE)      │  │
│   │  - Dedicated pool config    │  │
│   └─────────────────────────────┘  │
│                                     │
│   ┌─────────────────────────────┐  │
│   │  Provider 3 (inherits)      │  │
│   └─────────────────────────────┘  │
└─────────────────────────────────────┘
```

**Result:** Providers 1 and 3 share one pool, Provider 2 gets dedicated pool.

---

## YAML Configuration

### Full Configuration Example

```yaml
providers:
  # OpenAI-compatible providers
  openai_compatible:
    # Dialect-level pool configuration (shared by default)
    http_client:
      timeout_secs: 300                    # Total request timeout (10 min for long streams)
      connect_timeout_secs: 10             # Connection establishment timeout
      pool_max_idle_per_host: 32           # Max idle connections per host
      pool_idle_timeout_secs: 90           # Expire idle connections after 90s
      tcp_keepalive_secs: 60               # TCP keepalive interval
      max_retries: 3                       # Retry attempts for transient errors
      enable_pool_metrics: true            # Expose Prometheus metrics

    # Provider backends
    backends:
      # OpenAI - uses shared dialect pool
      - name: openai
        enabled: true
        base_url: "https://api.openai.com/v1"
        api_key: "${OPENAI_API_KEY}"
        # No http_client = inherits dialect settings

      # Groq - dedicated pool (very fast, needs different tuning)
      - name: groq
        enabled: true
        base_url: "https://api.groq.com/openai/v1"
        api_key: "${GROQ_API_KEY}"
        http_client:                       # OVERRIDE = separate pool
          timeout_secs: 30                 # Groq responds in <10s
          pool_max_idle_per_host: 64       # Higher concurrency
          pool_idle_timeout_secs: 60       # Faster rotation
          # Other settings inherited from dialect

      # Together AI - uses shared dialect pool
      - name: together
        enabled: true
        base_url: "https://api.together.xyz/v1"
        api_key: "${TOGETHER_API_KEY}"

      # Fireworks AI - partial override
      - name: fireworks
        enabled: true
        base_url: "https://api.fireworks.ai/inference/v1"
        api_key: "${FIREWORKS_API_KEY}"
        http_client:
          timeout_secs: 60                 # Only override timeout
          # Inherits other settings from dialect

  # Anthropic-compatible providers
  anthropic_compatible:
    http_client:
      timeout_secs: 600                    # Extended thinking needs longer timeout
      connect_timeout_secs: 10
      pool_max_idle_per_host: 32
      pool_idle_timeout_secs: 90
      tcp_keepalive_secs: 60
      max_retries: 3
      enable_pool_metrics: true

    backends:
      - name: anthropic
        enabled: true
        base_url: "https://api.anthropic.com"
        api_key: "${ANTHROPIC_API_KEY}"

      - name: anthropic_bedrock
        enabled: false
        base_url: "https://bedrock-runtime.us-east-1.amazonaws.com"
        # Uses AWS SigV4 auth instead of API key
        http_client:
          timeout_secs: 300                # Bedrock has different latency
```

### Minimal Configuration (All Defaults)

```yaml
providers:
  openai_compatible:
    backends:
      - name: openai
        base_url: "https://api.openai.com/v1"
        api_key: "${OPENAI_API_KEY}"

      - name: groq
        base_url: "https://api.groq.com/openai/v1"
        api_key: "${GROQ_API_KEY}"
```

**Result:** All providers use built-in defaults, share single dialect pool.

---

## Environment Variable Configuration

### Naming Convention

```
LUNAROUTE_<DIALECT>_<SETTING>=value              # Dialect-level
LUNAROUTE_<DIALECT>_<PROVIDER>_<SETTING>=value   # Provider-level (override)
```

### Environment Variable Precedence

1. **Provider-specific env var** (highest priority)
2. **Provider-specific YAML config**
3. **Dialect-level env var**
4. **Dialect-level YAML config**
5. **Built-in defaults** (lowest priority)

### Examples

```bash
# Dialect-level pool config (shared by all OpenAI-compatible providers)
export LUNAROUTE_OPENAI_TIMEOUT_SECS=300
export LUNAROUTE_OPENAI_POOL_MAX_IDLE=32
export LUNAROUTE_OPENAI_POOL_IDLE_TIMEOUT_SECS=90
export LUNAROUTE_OPENAI_TCP_KEEPALIVE_SECS=60
export LUNAROUTE_OPENAI_MAX_RETRIES=3
export LUNAROUTE_OPENAI_ENABLE_POOL_METRICS=true

# Provider-specific overrides (Groq gets dedicated pool)
export LUNAROUTE_OPENAI_GROQ_TIMEOUT_SECS=30
export LUNAROUTE_OPENAI_GROQ_POOL_MAX_IDLE=64
export LUNAROUTE_OPENAI_GROQ_POOL_IDLE_TIMEOUT_SECS=60

# Anthropic dialect
export LUNAROUTE_ANTHROPIC_TIMEOUT_SECS=600
export LUNAROUTE_ANTHROPIC_POOL_MAX_IDLE=32

# Provider base URLs and API keys
export LUNAROUTE_OPENAI_OPENAI_BASE_URL="https://api.openai.com/v1"
export LUNAROUTE_OPENAI_OPENAI_API_KEY="sk-..."
export LUNAROUTE_OPENAI_GROQ_BASE_URL="https://api.groq.com/openai/v1"
export LUNAROUTE_OPENAI_GROQ_API_KEY="gsk-..."
```

### Complete Environment Variable Reference

| Environment Variable | Type | Default | Description |
|---------------------|------|---------|-------------|
| `LUNAROUTE_<DIALECT>_TIMEOUT_SECS` | u64 | 300/600* | Total request timeout including streaming |
| `LUNAROUTE_<DIALECT>_CONNECT_TIMEOUT_SECS` | u64 | 10 | Connection establishment timeout |
| `LUNAROUTE_<DIALECT>_POOL_MAX_IDLE` | usize | 32 | Max idle connections per host |
| `LUNAROUTE_<DIALECT>_POOL_IDLE_TIMEOUT_SECS` | u64 | 90 | Expire idle connections after N seconds |
| `LUNAROUTE_<DIALECT>_TCP_KEEPALIVE_SECS` | u64 | 60 | TCP keepalive interval |
| `LUNAROUTE_<DIALECT>_MAX_RETRIES` | u32 | 3 | Retry attempts for transient errors |
| `LUNAROUTE_<DIALECT>_ENABLE_POOL_METRICS` | bool | true | Expose Prometheus metrics |

*300s for OpenAI, 600s for Anthropic (extended thinking)

---

## Configuration Resolution Logic

### Pool Assignment Algorithm

```rust
fn resolve_pool_for_provider(
    dialect_config: &HttpClientConfig,
    provider_config: &ProviderBackend,
) -> Client {
    if let Some(override_config) = &provider_config.http_client {
        // Provider has explicit override -> create dedicated pool
        info!(
            "Provider '{}' using dedicated pool (override detected)",
            provider_config.name
        );

        // Merge: override settings take precedence, others from dialect
        let merged_config = merge_configs(dialect_config, override_config);
        create_client(&merged_config)
    } else {
        // No override -> use shared dialect pool
        debug!(
            "Provider '{}' using shared dialect pool",
            provider_config.name
        );

        get_or_create_dialect_pool(dialect_config)
    }
}
```

### Config Merging Rules

When a provider overrides specific settings:

```yaml
# Dialect defaults
http_client:
  timeout_secs: 300
  pool_max_idle_per_host: 32
  pool_idle_timeout_secs: 90

# Provider override
backends:
  - name: groq
    http_client:
      timeout_secs: 30  # OVERRIDE
      # pool_max_idle_per_host not specified
```

**Result:**
- `timeout_secs: 30` (from provider)
- `pool_max_idle_per_host: 32` (inherited from dialect)
- `pool_idle_timeout_secs: 90` (inherited from dialect)

---

## Observability

### Prometheus Metrics

Metrics are emitted **per-provider** when `enable_pool_metrics: true`:

```promql
# Connection creation rate (should be low if pooling works)
http_pool_connections_created_total{provider="openai", dialect="openai_compatible"}
http_pool_connections_created_total{provider="groq", dialect="openai_compatible"}

# Connection reuse rate (higher is better)
http_pool_connections_reused_total{provider="openai", dialect="openai_compatible"}

# Current idle connections
http_pool_connections_idle{provider="openai", dialect="openai_compatible"}

# Connection lifetime distribution
http_pool_connection_lifetime_seconds{provider="openai", dialect="openai_compatible"}

# Pool configuration (gauge)
http_pool_config{provider="openai", setting="max_idle_per_host"} 32
http_pool_config{provider="groq", setting="max_idle_per_host"} 64
```

### Key Performance Indicators

**Connection Reuse Ratio** (target: >90%):
```promql
rate(http_pool_connections_reused_total[5m]) /
(rate(http_pool_connections_reused_total[5m]) +
 rate(http_pool_connections_created_total[5m]))
```

**Pool Churn Rate** (target: <1/sec for steady traffic):
```promql
rate(http_pool_connections_created_total[5m])
```

**Average Connection Lifetime** (target: >60s):
```promql
rate(http_pool_connection_lifetime_seconds_sum[5m]) /
rate(http_pool_connection_lifetime_seconds_count[5m])
```

### Debug Logging

With `RUST_LOG=debug`:

```
[INFO] Creating HTTP client pool for dialect 'openai_compatible':
       pool_max_idle=32, idle_timeout=90s, keepalive=60s
[DEBUG] Provider 'openai' using shared dialect pool
[DEBUG] Provider 'groq' using dedicated pool (override detected)
[INFO] Creating HTTP client pool for provider 'groq':
       pool_max_idle=64, idle_timeout=60s, keepalive=60s
[DEBUG] 🔌 Provider 'openai' initiating connection to https://api.openai.com
[DEBUG] ♻️ Provider 'openai' reused connection (2ms) - from pool
[DEBUG] 🔌 Provider 'groq' initiating connection to https://api.groq.com
[DEBUG] 🆕 Provider 'groq' new connection (45ms) - likely not from pool
```

### Health Endpoint

`GET /health` includes pool information:

```json
{
  "status": "healthy",
  "version": "0.1.0",
  "connection_pools": {
    "openai_compatible": {
      "shared": {
        "providers": ["openai", "together", "fireworks"],
        "config": {
          "max_idle_per_host": 32,
          "idle_timeout_secs": 90,
          "keepalive_secs": 60
        },
        "client_age_secs": 3600
      },
      "dedicated": {
        "groq": {
          "config": {
            "max_idle_per_host": 64,
            "idle_timeout_secs": 60,
            "keepalive_secs": 60
          },
          "client_age_secs": 3600
        }
      }
    },
    "anthropic_compatible": {
      "shared": {
        "providers": ["anthropic"],
        "config": {
          "max_idle_per_host": 32,
          "idle_timeout_secs": 90,
          "keepalive_secs": 60
        },
        "client_age_secs": 3600
      }
    }
  }
}
```

---

## Routing Strategies

### How Requests are Routed to Providers

Multiple approaches can be configured:

#### 1. Header-Based Routing
```bash
curl -H "X-LunaRoute-Provider: groq" \
     -H "Authorization: Bearer gsk-..." \
     http://localhost:8081/v1/chat/completions
```

#### 2. Model-Based Routing (via prefix)
```json
{
  "model": "groq/llama-3-70b-8192",
  "messages": [...]
}
```

Config:
```yaml
routing:
  strategy: model_prefix
  # Extract provider from "provider/model" format
```

#### 3. Config-Based Routing (pattern matching)
```yaml
routing:
  strategy: config
  default_provider: openai

  model_routing:
    "llama-*": groq
    "mixtral-*": together
    "gpt-*": openai
    "claude-*": anthropic
```

#### 4. Load Balancing
```yaml
routing:
  strategy: round_robin  # or: least_latency, random
  providers: [openai, groq, together]
```

---

## Use Cases & Examples

### Use Case 1: Cost Optimization

**Scenario:** Route cheap queries to Groq, expensive to OpenAI.

```yaml
providers:
  openai_compatible:
    http_client:
      timeout_secs: 300
      pool_max_idle_per_host: 16  # Lower for expensive provider

    backends:
      - name: openai
        base_url: "https://api.openai.com/v1"

      - name: groq
        base_url: "https://api.groq.com/openai/v1"
        http_client:
          pool_max_idle_per_host: 64  # Higher for high-volume cheap provider

routing:
  strategy: config
  model_routing:
    "gpt-4*": openai          # Expensive models
    "llama-3-8b*": groq       # Cheap, fast models
    "llama-3-70b*": groq
```

### Use Case 2: Multi-Region Deployment

**Scenario:** Route based on geography.

```yaml
providers:
  openai_compatible:
    backends:
      - name: openai_us
        base_url: "https://api.openai.com/v1"

      - name: openai_eu
        base_url: "https://api.openai.com/v1"  # Could be EU endpoint
        http_client:
          connect_timeout_secs: 5  # Stricter for same-region

      - name: kimi_cn
        base_url: "https://api.kimi.ai/v1"
        http_client:
          timeout_secs: 60
          pool_max_idle_per_host: 8  # Lower volume from CN

routing:
  strategy: geo
  rules:
    - region: us
      provider: openai_us
    - region: eu
      provider: openai_eu
    - region: cn
      provider: kimi_cn
```

### Use Case 3: Failover & Redundancy

**Scenario:** Try Groq first, fallback to OpenAI.

```yaml
providers:
  openai_compatible:
    backends:
      - name: groq
        base_url: "https://api.groq.com/openai/v1"
        http_client:
          timeout_secs: 30
          max_retries: 1  # Fail fast

      - name: openai
        base_url: "https://api.openai.com/v1"
        http_client:
          max_retries: 3  # More retries for fallback

routing:
  strategy: failover
  primary: groq
  fallback: openai
```

### Use Case 4: Development vs Production

**Environment-based config:**

```bash
# Development - single provider, simple
export LUNAROUTE_OPENAI_OPENAI_BASE_URL="https://api.openai.com/v1"
export LUNAROUTE_OPENAI_POOL_MAX_IDLE=8

# Production - multi-provider, optimized
export LUNAROUTE_OPENAI_POOL_MAX_IDLE=32
export LUNAROUTE_OPENAI_GROQ_POOL_MAX_IDLE=128
export LUNAROUTE_OPENAI_GROQ_TIMEOUT_SECS=30
```

---

## Implementation Checklist

### Phase 1: Core Configuration (Priority: High)
- [ ] Extend `HttpClientConfig` with new fields
- [ ] Add dialect-level config to YAML structure
- [ ] Add provider-level config override to YAML
- [ ] Implement config merging logic
- [ ] Add environment variable parsing
- [ ] Add env var precedence handling
- [ ] Update config validation

### Phase 2: Pool Management (Priority: High)
- [ ] Implement shared pool registry (per dialect)
- [ ] Implement dedicated pool creation (per provider override)
- [ ] Add pool assignment logic
- [ ] Add connection reuse detection (timing-based)
- [ ] Add debug logging for pool creation/reuse

### Phase 3: Observability (Priority: Medium)
- [ ] Add Prometheus metrics (connection created/reused/idle)
- [ ] Add per-provider metric labels
- [ ] Extend `/health` endpoint with pool info
- [ ] Add connection lifetime tracking
- [ ] Document Grafana dashboard queries

### Phase 4: Routing (Priority: Low)
- [ ] Implement header-based routing
- [ ] Implement model-prefix routing
- [ ] Implement config-based routing
- [ ] Implement load balancing strategies
- [ ] Add routing metrics

### Phase 5: Documentation (Priority: High)
- [ ] Update README with multi-provider examples
- [ ] Create migration guide from single-provider
- [ ] Add troubleshooting guide
- [ ] Create example configs for common use cases

---

## Migration Guide

### From Single Provider to Multi-Provider

**Before (current):**
```yaml
providers:
  openai:
    enabled: true
    api_key: "sk-..."
    base_url: "https://api.openai.com"
```

**After (backward compatible):**
```yaml
providers:
  openai_compatible:
    backends:
      - name: openai
        enabled: true
        api_key: "sk-..."
        base_url: "https://api.openai.com/v1"
```

**No breaking changes** - maintain backward compatibility with config auto-migration.

---

## Performance Considerations

### Memory Usage

**Per-pool overhead:** ~2MB baseline + ~1KB per connection

**Example calculation:**
- 1 dialect pool (32 connections): ~2MB + 32KB
- 1 dedicated provider pool (64 connections): ~2MB + 64KB
- **Total:** ~4MB for 2 pools

**Recommendation:** Default to shared pools unless specific tuning is needed.

### Latency Impact

**Connection reuse:** 1-5ms (from pool)
**New connection:** 50-200ms (TLS handshake)

**Target reuse ratio:** >90% for optimal performance

### Resource Limits

**Operating system limits:**
- Most systems: 1024 file descriptors by default
- Each connection = 1 file descriptor
- Monitor with: `ulimit -n` and `lsof | grep ESTABLISHED | wc -l`

**Recommended limits:**
- Development: 256 total connections (8 providers × 32 max)
- Production: 1024+ total connections
- Adjust with: `ulimit -n 4096`

---

## Security Considerations

### API Key Management

**Never log API keys:**
```rust
debug!("Using provider '{}' with key: {}",
       provider_name,
       mask_api_key(&api_key));  // Show only last 4 chars
```

**Environment variable precedence** allows:
- YAML: Development (checked into git without keys)
- Env vars: Production (injected at runtime)

### TLS Configuration

All connections use `rustls` (no OpenSSL dependency):
```rust
ClientBuilder::new()
    .use_rustls_tls()
    .min_tls_version(tls::Version::TLS_1_2)
```

### Connection Isolation

Pools are isolated per host by reqwest:
- `api.openai.com` connections never mix with `api.groq.com`
- Credentials cannot leak between providers
- Network issues at one provider don't affect others

---

## Testing Strategy

### Unit Tests
- [ ] Config parsing (YAML + env vars)
- [ ] Config merging (dialect + provider override)
- [ ] Pool assignment logic
- [ ] Metric emission

### Integration Tests
- [ ] Multiple providers sharing dialect pool
- [ ] Provider with dedicated pool
- [ ] Env var override precedence
- [ ] Connection reuse verification
- [ ] Failover between providers

### Performance Tests
- [ ] Connection reuse ratio measurement
- [ ] Pool exhaustion handling
- [ ] Concurrent request load test
- [ ] Memory usage profiling

---

## Open Questions & Future Work

1. **Dynamic pool resizing** - Adjust pool size based on traffic?
2. **Connection health checks** - Proactive validation before reuse?
3. **Provider-specific retry policies** - Different backoff strategies?
4. **Circuit breaker integration** - Stop using failed providers?
5. **WebSocket support** - Extend pooling to WS connections?
6. **HTTP/2 multiplexing** - Does reqwest support it? (Yes, but needs testing)
7. **Connection draining** - Graceful shutdown of pools during config reload?

---

## References

- [reqwest Connection Pooling](https://docs.rs/reqwest/latest/reqwest/#connection-pooling)
- [hyper Connection Pooling](https://hyper.rs/guides/1/client/pool/)
- [Prometheus Best Practices](https://prometheus.io/docs/practices/naming/)
- [12-Factor App Config](https://12factor.net/config)

---

## Changelog

### v1.0 (2025-10-07)
- Initial specification
- Layered configuration model (dialect + provider)
- Environment variable support
- Observability design
- Routing strategies

---

**Status:** Ready for implementation
**Next Steps:** Phase 1 implementation (Core Configuration)
