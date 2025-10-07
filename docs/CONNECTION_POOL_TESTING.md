# Connection Pool Testing & Monitoring

## Problem Statement

**Issue 1: Idle Connection Reuse**
Requests getting stuck when reusing idle connections that have been closed by upstream servers.

**Root Cause:** HTTP connection pooling without idle timeout causes the client to keep connections forever. When upstream servers (OpenAI, Anthropic) close idle connections after 60-120 seconds, the client tries to reuse dead connections, causing requests to hang.

**Fix:** Configure `pool_idle_timeout(90s)` and `tcp_keepalive(60s)` in the HTTP client.

**Issue 2: Streaming Timeout**
Requests timing out mid-stream during long-running operations (extended thinking, compaction).

**Root Cause:** The request timeout applies to the entire request duration, including streaming responses. For extended thinking sessions or Claude Code compaction, streaming responses can run for 3-5+ minutes. A 60-second timeout kills the connection mid-stream, leaving it in an inconsistent state in the pool.

**Fix:** Increase `timeout` from 60s to 600s (10 minutes) to accommodate long-running streaming operations while still providing a safety timeout for truly stuck requests.

## Testing Strategy

### 1. Unit Tests (Fast)

Run the regression test that ensures pool configuration is correct:

```bash
cargo test --package lunaroute-egress test_client_has_pool_idle_timeout_configured
```

This test verifies the HTTP client is built with the necessary configuration.

### 2. Integration Tests (Medium)

Run connection pool behavior tests:

```bash
# Basic connection reuse test
cargo test --package lunaroute-egress --test connection_pool_test test_connection_pool_reuse

# Concurrent requests test
cargo test --package lunaroute-egress --test connection_pool_test test_concurrent_requests

# Server closes connection test
cargo test --package lunaroute-egress --test connection_pool_test test_server_closes_idle_connection
```

### 3. Long-Running Tests (Slow - 95+ seconds)

Run the idle timeout test to verify connections expire correctly:

```bash
cargo test --package lunaroute-egress --test connection_pool_test -- --ignored

# Or run specific test:
cargo test --package lunaroute-egress --test connection_pool_test test_connection_pool_idle_timeout -- --ignored
```

**Note:** This test takes 95+ seconds because it waits for the pool idle timeout (90s).

## Manual Testing

### Reproduce the Bug (Pre-Fix Behavior)

To verify the fix works, you can simulate the stuck request scenario:

1. **Temporarily remove** the pool_idle_timeout from `crates/lunaroute-egress/src/client.rs`:
   ```rust
   // Comment out this line:
   // .pool_idle_timeout(Duration::from_secs(90))
   ```

2. **Start LunaRoute:**
   ```bash
   cargo run --package lunaroute-server --release
   ```

3. **Make a request:**
   ```bash
   curl http://localhost:8081/v1/chat/completions \
     -H "Content-Type: application/json" \
     -d '{"model": "gpt-4o-mini", "messages": [{"role": "user", "content": "Hello"}]}'
   ```

4. **Wait 90 seconds** (for upstream to close the connection)

5. **Make another request** - it should hang/timeout

6. **Restore the fix** and verify it works:
   ```rust
   // Uncomment:
   .pool_idle_timeout(Duration::from_secs(90))
   ```

7. **Repeat steps 2-5** - requests should work correctly

## Production Monitoring

### Metrics to Track

Add these metrics to detect connection pool issues in production:

1. **Connection Reuse Rate**
   ```
   http_connections_reused / http_connections_created
   ```
   - Healthy: 80-95% (connections are being reused)
   - Unhealthy: <50% (connections dying or not being pooled)

2. **Request Timeout Rate**
   ```
   http_requests_timeout / http_requests_total
   ```
   - Healthy: <0.1%
   - Unhealthy: >1%

3. **Request Duration P99**
   ```
   histogram_quantile(0.99, http_request_duration_seconds)
   ```
   - Healthy: <5 seconds for normal requests
   - Unhealthy: >30 seconds (possible stuck requests)

4. **Connection Pool Size**
   ```
   http_connection_pool_idle + http_connection_pool_active
   ```
   - Healthy: Stable, within configured limits
   - Unhealthy: Constantly growing or at max

### Log Monitoring

Watch for these patterns in logs:

```bash
# Stuck request indicators:
grep -i "timeout" logs/lunaroute.log
grep -i "connection reset" logs/lunaroute.log
grep -i "broken pipe" logs/lunaroute.log
grep -i "pool.*full" logs/lunaroute.log
```

### Health Check Endpoint

Monitor the `/healthz` endpoint for degraded performance:

```bash
# Check response time
time curl http://localhost:8081/healthz

# Should respond in <100ms
# If >1s, investigate connection pool
```

## Debugging Stuck Requests

If you suspect stuck requests in production:

### 1. Check Active Connections

```bash
# Count active connections to upstream
netstat -an | grep -E ":(443|80)" | grep ESTABLISHED | wc -l

# Should be < pool_max_idle_per_host (32) per upstream
```

### 2. Enable Debug Logging

Set `RUST_LOG=debug` to see connection pool behavior:

```bash
RUST_LOG=lunaroute_egress=debug cargo run --package lunaroute-server
```

Look for:
- `Connection pool: reusing connection`
- `Connection pool: creating new connection`
- `Connection reset by peer`

### 3. Check Upstream Server Timeout

Test how long upstream keeps connections alive:

```bash
# OpenAI
curl -v https://api.openai.com/v1/models \
  -H "Authorization: Bearer $OPENAI_API_KEY" \
  --keepalive-time 120

# Should close after ~60-120 seconds of idle
```

### 4. Force Connection Refresh

Restart LunaRoute to clear the connection pool:

```bash
# Graceful restart
kill -TERM $(pgrep lunaroute-server)
```

## Configuration Tuning

If you need to adjust pool settings:

### Current Configuration
```rust
// crates/lunaroute-egress/src/client.rs
.pool_max_idle_per_host(32)        // Max pooled connections per host
.pool_idle_timeout(90)              // Expire after 90s idle
.tcp_keepalive(60)                  // Send TCP keepalive every 60s
.timeout(600)                       // Request timeout after 600s (10 min for streaming)
.connect_timeout(10)                // Connection timeout after 10s
```

### Tuning Recommendations

**For high-traffic deployments:**
```rust
.pool_max_idle_per_host(64)        // More pooled connections
.pool_idle_timeout(60)              // More aggressive expiry
```

**For low-latency requirements:**
```rust
.pool_max_idle_per_host(16)        // Fewer connections
.pool_idle_timeout(30)              // Expire faster
.tcp_keepalive(30)                  // More frequent keepalive
```

**For very long-running operations (e.g., multi-file code generation):**
```rust
.timeout(1200)                      // 20 minute timeout for extreme cases
.tcp_keepalive(30)                  // More frequent keepalive
.pool_idle_timeout(60)              // Faster expiry to prevent stale connections
```

**IMPORTANT:** The `.timeout()` setting applies to the ENTIRE request, including:
- Time to establish connection
- Time to receive first byte
- Time to read complete streaming response

For streaming requests that can run for minutes (extended thinking, compaction),
the timeout must be set higher than the longest expected operation.

## Preventing Regression

### Pre-Commit Checklist

Before merging changes to `crates/lunaroute-egress/src/client.rs`:

- [ ] Run: `cargo test --package lunaroute-egress test_client_has_pool_idle_timeout_configured`
- [ ] Verify `pool_idle_timeout` is present in `create_client()`
- [ ] Verify `tcp_keepalive` is present in `create_client()`
- [ ] Run integration tests: `cargo test --package lunaroute-egress --test connection_pool_test`

### CI/CD Pipeline

Add these checks to your CI pipeline:

```yaml
# .github/workflows/ci.yml
- name: Test connection pool configuration
  run: |
    cargo test --package lunaroute-egress test_client_has_pool_idle_timeout_configured
    cargo test --package lunaroute-egress --test connection_pool_test

- name: Verify pool_idle_timeout exists
  run: |
    grep -q "pool_idle_timeout" crates/lunaroute-egress/src/client.rs || exit 1
```

## References

- **reqwest documentation:** https://docs.rs/reqwest/latest/reqwest/
- **Connection pooling best practices:** https://www.nginx.com/blog/http-keepalives-and-web-performance/
- **Issue history:** See git log for `crates/lunaroute-egress/src/client.rs`

## Contact

If you encounter stuck requests or connection pool issues:
1. Check this document first
2. Run the diagnostic steps above
3. Open an issue with logs and reproduction steps
