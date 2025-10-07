# Using LunaRoute with Claude Code / OpenCode

**The ultimate local proxy for AI coding assistants with zero-overhead passthrough, comprehensive session recording, and powerful debugging capabilities.**

---

## 🎯 Why Use LunaRoute Locally?

LunaRoute transforms your AI coding experience by giving you complete visibility and control over every interaction:

- **🔍 Debug AI conversations** - See exactly what Claude Code sends/receives
- **💰 Track token usage** - Know where your money goes (input/output/thinking tokens)
- **🔧 Analyze tool usage** - Which tools does Claude use most? How long do they take?
- **🔒 Privacy protection** - Auto-redact PII before logging sensitive data
- **📊 Performance insights** - Measure proxy overhead vs provider latency
- **🔎 Search past sessions** - Full-text search across all recorded conversations
- **⚡ Zero overhead** - Sub-millisecond passthrough mode (0.1-0.2ms)
- **🔑 No API keys needed** - Claude Code provides authentication (zero-config setup)

---

## ⚡ Quick Start (60 seconds)

> **First time?** Build the binary: `cargo build --release --package lunaroute-server`
> Binary location: `target/release/lunaroute-server`

### Option 1: Zero-Config Passthrough (Recommended) 🔥

**No API key needed!** Claude Code provides authentication automatically:

```bash
# 1. Start LunaRoute (no API key required!)
lunaroute-server

# 2. Point Claude Code to the proxy
export ANTHROPIC_BASE_URL=http://localhost:8081

# 3. Use Claude Code normally - it will send its API key automatically!
```

**That's it!** All requests flow through LunaRoute with <0.2ms overhead. Your API key stays in Claude Code only.

> **Why no API key?** In passthrough mode, LunaRoute forwards Claude Code's `x-api-key` header directly to Anthropic. This means:
> - ✅ Zero configuration needed
> - ✅ API key never stored on proxy server
> - ✅ Multiple developers can use same proxy with different keys
> - ✅ More secure (no shared secrets)

### Option 1b: With Server-Side API Key

If you prefer to configure the API key on the server (e.g., for centralized management):

```bash
# 1. Start with API key configured
ANTHROPIC_API_KEY=sk-ant-... lunaroute-server

# 2. Point Claude Code to the proxy
export ANTHROPIC_BASE_URL=http://localhost:8081
```

### Option 2: With Session Recording

Capture every conversation for analysis (still no API key needed!):

```bash
# 1. Start with recording enabled (Claude Code will provide API key)
LUNAROUTE_ENABLE_SESSION_RECORDING=true \
LUNAROUTE_LOG_LEVEL=debug \
lunaroute-server

# 2. Point Claude Code to the proxy
export ANTHROPIC_BASE_URL=http://localhost:8081

# 3. Watch sessions get recorded to ~/.lunaroute/sessions/
```

### Option 3: Using Config Files

For repeatable setups:

```bash
# Use the pre-configured Claude Code setup (no API key in config!)
lunaroute-server --config examples/configs/claude-code-proxy.yaml

# With recording
lunaroute-server --config examples/configs/claude-code-proxy-with-recording.yaml
```

**📝 Note:** All example configs use client authentication by default (no `api_key` field). If you want server-side keys, add `api_key: "${ANTHROPIC_API_KEY}"` to the config.

---

## 🚀 Key Features for Local Development

### 1. **Zero-Overhead Passthrough Mode**

When your API dialect matches the provider (Anthropic→Anthropic), LunaRoute skips normalization entirely:

- **Sub-millisecond overhead**: ~0.1-0.2ms added latency
- **100% API fidelity**: Preserves extended thinking, all response fields
- **No translation layer**: Direct proxy with optional recording
- **Session recording compatible**: Record without normalization overhead

**How it works:**
```yaml
# claude-code-proxy.yaml
api_dialect: "anthropic"  # Match your provider
providers:
  anthropic:
    enabled: true
    # No api_key = forwards client's header
```

### 2. **Client Authorization Forwarding**

No need to store API keys on the server - let Claude Code provide them:

```yaml
providers:
  anthropic:
    enabled: true
    # api_key: ""  # Empty or omit = use client's x-api-key header
```

**Why this matters:**
- Keep your API key in Claude Code only
- Run LunaRoute without secrets
- Multiple developers can use same proxy with their own keys
- Secure by default

### 3. **Comprehensive Session Recording**

Capture everything for later analysis:

#### Dual Storage Format
- **JSONL**: Human-readable, one event per line
  ```jsonl
  {"session_id":"abc123","request_id":"req1","model":"claude-sonnet-4"}
  {"session_id":"abc123","request":"Please explain...","input_tokens":150}
  {"session_id":"abc123","response":"Here's the explanation...","output_tokens":420}
  ```
- **SQLite**: Queryable database with full-text search
  ```sql
  SELECT * FROM sessions WHERE model_used = 'claude-sonnet-4'
    AND input_tokens > 1000
    ORDER BY started_at DESC;
  ```

#### Session Grouping
Multi-turn conversations automatically grouped by session ID:
```
sessions/2025-01-06/session_550e8400-e29b-41d4-a716-446655440000.jsonl
  ├─ Turn 1: "Help me debug this function"
  ├─ Turn 2: "Now optimize it"
  └─ Turn 3: "Add error handling"
```

#### Async Recording
- **Non-blocking**: Uses async channels with batching
- **Batched writes**: 100 events or 100ms timeout
- **Compression**: Zstd level 3 for old sessions (7+ days)
- **Retention policies**: Auto-cleanup by age or total size

### 4. **Session Statistics & Analytics**

Know exactly what's happening in each conversation:

```
📊 Session Statistics Summary
═══════════════════════════════════════════════════════════════

Session: 550e8400-e29b-41d4-a716-446655440000
  Requests:        5
  Input tokens:    2,450
  Output tokens:   5,830
  Thinking tokens: 1,200
  Total tokens:    9,480

  Tool usage:
    Read:  12 calls (avg 45ms)
    Write: 8 calls (avg 120ms)
    Bash:  3 calls (avg 850ms)

  Performance:
    Avg response time: 2.3s
    Proxy overhead:    12ms total (0.5%)
    Provider latency:  2.288s (99.5%)

💰 Estimated cost: $0.14 USD
```

**Track per session:**
- Token counts (input/output/thinking separated)
- Request count in multi-turn conversations
- Tool usage breakdown with execution times
- Proxy overhead vs provider latency
- Cost estimates

### 5. **Request/Response Logging**

See everything in real-time:

```bash
LUNAROUTE_LOG_REQUESTS=true \
LUNAROUTE_LOG_LEVEL=debug \
lunaroute-server
```

**Output:**
```
┌─────────────────────────────────────────────────────────
│ REQUEST to Anthropic (streaming)
├─────────────────────────────────────────────────────────
│ Model: claude-sonnet-4-5
│ Messages: 3 messages
└─────────────────────────────────────────────────────────

┌─────────────────────────────────────────────────────────
│ STREAMING from Anthropic
│ 📝 I'll help you debug
│ 📝  that function. Let me
│ 📝  start by reading the code.
│ 🔧 Tool call: Read
│ 📊 Usage: input=150, output=420, total=570
│ 🏁 Stream ended: EndTurn
└─────────────────────────────────────────────────────────
```

### 6. **Advanced Session Search**

Find past conversations instantly:

```bash
# Search for sessions about "debugging"
curl "http://localhost:8081/sessions?text_search=debugging"

# Find expensive sessions (>10K tokens)
curl "http://localhost:8081/sessions?min_total_tokens=10000"

# Claude-specific sessions in last 7 days
curl "http://localhost:8081/sessions?model=claude-sonnet-4&days=7"

# Failed requests only
curl "http://localhost:8081/sessions?success=false"
```

**Search capabilities:**
- Full-text search in requests/responses
- Filter by model, provider, date range
- Token count ranges (input/output/total)
- Duration ranges, success/failure
- Pagination with multiple sort orders

### 7. **PII Detection & Redaction**

Protect sensitive data automatically:

```yaml
session_recording:
  pii:
    enabled: true
    detect_email: true
    detect_phone: true
    detect_ssn: true
    detect_credit_card: true
    redaction_mode: "tokenize"  # mask, remove, tokenize, or partial
    hmac_secret: "${PII_SECRET}"

    custom_patterns:
      - name: "api_key"
        pattern: "sk-[a-zA-Z0-9]{32}"
        confidence: 0.95
        redaction_mode: "mask"
        placeholder: "[API_KEY]"
```

**Before:** `My email is john.doe@example.com and SSN is 123-45-6789`
**After:** `My email is [EMAIL:a3f8e9d2] and SSN is [SSN:7b2c4f1a]`

**Features:**
- Built-in detectors (email, phone, SSN, credit cards, IPs)
- Custom regex patterns with JSON config
- HKDF-based secure tokenization
- JSON structure preservation in tool arguments
- Overlapping detection handling

### 8. **Prometheus Metrics**

Monitor everything at `/metrics`:

```
# Request counters
lunaroute_requests_total{listener="anthropic",model="claude-sonnet-4",provider="anthropic"} 142

# Latency histograms (P50, P95, P99)
lunaroute_request_duration_seconds_bucket{le="0.5"} 89
lunaroute_request_duration_seconds_bucket{le="1.0"} 120

# Token usage
lunaroute_tokens_total{type="input",model="claude-sonnet-4"} 45230
lunaroute_tokens_total{type="output",model="claude-sonnet-4"} 127450
lunaroute_tokens_total{type="thinking",model="claude-sonnet-4"} 8920

# Tool calls
lunaroute_tool_calls_total{tool="Read"} 234
lunaroute_tool_calls_total{tool="Write"} 156
lunaroute_tool_calls_total{tool="Bash"} 89

# Streaming metrics
lunaroute_streaming_ttft_seconds_bucket{le="0.5"} 134  # Time-to-first-token
lunaroute_streaming_chunk_latency_seconds_bucket{le="0.1"} 2456
```

**24 metric types covering:**
- Request rates and success/failure
- Latency percentiles (P50, P95, P99)
- Token usage breakdown
- Tool usage statistics
- Circuit breaker states
- Provider health
- Streaming performance (TTFT, chunk latency)

### 9. **Intelligent Routing & Fallbacks**

Multiple providers with automatic failover:

```yaml
providers:
  anthropic:
    enabled: true
    api_key: "${ANTHROPIC_API_KEY}"

  openai:
    enabled: true
    api_key: "${OPENAI_API_KEY}"

routing:
  rules:
    - name: "claude-with-fallback"
      model_pattern: "^claude-.*"
      primary: "anthropic"
      fallbacks: ["openai"]  # Try OpenAI if Anthropic fails

    - name: "weighted-distribution"
      model_pattern: "^gpt-.*"
      strategy: "weighted_round_robin"
      weights:
        openai_primary: 80
        openai_backup: 20
```

**Features:**
- Round-robin and weighted round-robin
- Circuit breakers (3 failures → open)
- Health monitoring (success rates)
- Lock-free concurrent access
- Automatic failover chains

### 10. **Custom Headers & Request Modifications**

Add context to every request:

```yaml
providers:
  anthropic:
    request_headers:
      headers:
        X-Developer-ID: "${env.USER}"
        X-Session-Context: "claude-code-${session_id}"
        X-Request-Source: "lunaroute-local"

    request_body:
      defaults:
        temperature: 0.7  # Set if not specified
        max_tokens: 4096

      overrides:
        # Force certain values
        top_p: 0.9

      prepend_messages:
        - role: "user"
          content: "You are helping with local development. Be concise."
```

**Template variables:**
- `${provider}` - Provider name
- `${model}` - Model name
- `${session_id}` - Session ID
- `${request_id}` - Request ID
- `${env.VAR_NAME}` - Environment variables (filtered for security)

---

## 📋 Practical Use Cases

### Use Case 1: Debug Token Usage

**Problem:** Claude Code conversations get expensive, but you don't know why.

**Solution:**
```bash
# Start with session recording + debug logging
LUNAROUTE_LOG_LEVEL=debug lunaroute-server

# After session, check stats on shutdown
# Output shows per-turn token breakdown
```

**Result:** Discover that tool outputs are huge, optimize Read tool usage.

---

### Use Case 2: Analyze Tool Performance

**Problem:** Claude seems slow, but is it the tools or the LLM?

**Solution:** Check session statistics
```
Tool usage:
  Read:  12 calls (avg 45ms)   ← Fast
  Write: 8 calls (avg 120ms)   ← Reasonable
  Bash:  3 calls (avg 850ms)   ← SLOW! Optimize these
```

**Result:** Realize Bash commands need optimization, not the LLM.

---

### Use Case 3: Search Past Conversations

**Problem:** "I solved this bug last week, how did Claude help me?"

**Solution:**
```bash
# Search for "TypeError" in past sessions
curl "http://localhost:8081/sessions?text_search=TypeError&days=7"

# Get the session details
curl "http://localhost:8081/sessions/{session_id}"
```

**Result:** Find the exact conversation and solution approach.

---

### Use Case 4: Compliance & Privacy

**Problem:** Need to log sessions but can't store PII.

**Solution:**
```yaml
session_recording:
  pii:
    enabled: true
    detect_email: true
    detect_phone: true
    detect_ssn: true
    detect_credit_card: true
    redaction_mode: "tokenize"
```

**Result:** All PII auto-redacted before hitting disk. Compliance achieved.

---

### Use Case 5: Multi-Developer Team

**Problem:** Team uses same proxy, but everyone has different API keys.

**Solution:**
```yaml
providers:
  anthropic:
    enabled: true
    # No api_key field = use client headers
```

**Result:** Each developer's Claude Code sends their own key. No shared secrets.

---

### Use Case 6: Cost Attribution

**Problem:** Multiple projects, need to track costs per project.

**Solution:**
```yaml
providers:
  anthropic:
    request_headers:
      headers:
        X-Project-ID: "${env.PROJECT_NAME}"
```

Then query sessions:
```bash
# Get all sessions for project "myapp"
curl "http://localhost:8081/sessions" | jq '.[] | select(.metadata.project == "myapp")'
```

**Result:** Per-project token and cost breakdown.

---

## 🔧 Configuration Examples

### Minimal Setup (True Zero Config!) 🎉

**Literally just run it - no API key needed:**

```bash
# 1. Start LunaRoute (that's it!)
lunaroute-server

# 2. Point Claude Code to it
export ANTHROPIC_BASE_URL=http://localhost:8081

# 3. Done! Claude Code will provide its own API key
```

**Or if you prefer server-side API key:**

```bash
# Set API key and run
export ANTHROPIC_API_KEY=sk-ant-...
lunaroute-server

# Point Claude Code
export ANTHROPIC_BASE_URL=http://localhost:8081
```

### Debug Everything

```yaml
# development.yaml
host: "127.0.0.1"
port: 8081
api_dialect: "anthropic"

providers:
  anthropic:
    enabled: true

session_recording:
  enabled: true
  sessions_dir: "./sessions"

logging:
  level: "debug"  # HTTP headers, timing, full requests
  log_requests: true

session_stats_max_sessions: 100
```

```bash
# With server-side API key
ANTHROPIC_API_KEY=sk-ant-... \
lunaroute-server --config development.yaml

# Or without API key (Claude Code will provide it)
lunaroute-server --config development.yaml
export ANTHROPIC_BASE_URL=http://localhost:8081
```

### Privacy-First Recording

```yaml
# privacy-first.yaml
api_dialect: "anthropic"

providers:
  anthropic:
    enabled: true

session_recording:
  enabled: true
  sessions_dir: "~/.lunaroute/sessions"

  pii:
    enabled: true
    detect_email: true
    detect_phone: true
    detect_ssn: true
    detect_credit_card: true
    detect_ip_address: true
    redaction_mode: "tokenize"
    hmac_secret: "${PII_SECRET}"

    custom_patterns:
      - name: "api_key"
        pattern: "sk-[a-zA-Z0-9]{32,}"
        confidence: 0.95
        redaction_mode: "mask"
        placeholder: "[API_KEY]"

      - name: "aws_secret"
        pattern: "AKIA[0-9A-Z]{16}"
        confidence: 0.95
        redaction_mode: "mask"
        placeholder: "[AWS_KEY]"

logging:
  level: "info"
  log_requests: false  # Privacy: don't log to stdout
```

### Multi-Provider with Fallback

```yaml
# multi-provider.yaml
api_dialect: "openai"  # Accept OpenAI format

providers:
  openai:
    enabled: true
    api_key: "${OPENAI_API_KEY}"

  anthropic:
    enabled: true
    api_key: "${ANTHROPIC_API_KEY}"

routing:
  rules:
    - name: "gpt-primary"
      model_pattern: "^gpt-.*"
      primary: "openai"
      fallbacks: ["anthropic"]  # Use Claude if OpenAI down

    - name: "claude-primary"
      model_pattern: "^claude-.*"
      primary: "anthropic"
      fallbacks: ["openai"]  # Use GPT if Anthropic down

session_recording:
  enabled: true

logging:
  level: "info"
  log_requests: true
```

---

## 🎓 Pro Tips

### Tip 1: Session Grouping for Claude Code

Claude Code uses Anthropic's metadata to track sessions. Extract the session ID:

```python
# In Claude Code's requests, look for:
{
  "metadata": {
    "user_id": "user_abc123_account_def456_session_550e8400-e29b-41d4-a716-446655440000"
  }
}
```

LunaRoute extracts `550e8400-e29b-41d4-a716-446655440000` after `_session_` and groups all turns together.

### Tip 2: Watch Sessions in Real-Time

```bash
# Terminal 1: Start LunaRoute
lunaroute-server

# Terminal 2: Watch sessions being created
watch -n 1 'ls -lh ~/.lunaroute/sessions/$(date +%Y-%m-%d)/'

# Terminal 3: Tail the latest session
tail -f ~/.lunaroute/sessions/$(date +%Y-%m-%d)/session_*.jsonl | jq
```

### Tip 3: Query Sessions with jq

```bash
# Get total tokens from today's sessions
cat ~/.lunaroute/sessions/$(date +%Y-%m-%d)/*.jsonl | \
  jq -s '[.[] | select(.final_stats) | .final_stats.tokens.total] | add'

# Find most expensive session
cat ~/.lunaroute/sessions/$(date +%Y-%m-%d)/*.jsonl | \
  jq -s 'group_by(.session_id) |
         map({session: .[0].session_id, tokens: ([.[] | .final_stats.tokens.total // 0] | add)}) |
         sort_by(.tokens) |
         reverse |
         first'
```

### Tip 4: Prometheus + Grafana

Export metrics to Grafana for beautiful dashboards:

```bash
# prometheus.yml
scrape_configs:
  - job_name: 'lunaroute'
    static_configs:
      - targets: ['localhost:8081']
    metrics_path: '/metrics'
    scrape_interval: 5s
```

Create dashboards showing:
- Requests per minute
- P95 latency over time
- Token usage trends
- Tool call frequency
- Cost per day

### Tip 5: Compression for Long-Term Storage

Old sessions auto-compress after 7 days:

```yaml
session_recording:
  cleanup:
    retention:
      max_age_days: 30        # Delete after 30 days
      max_total_size_gb: 10   # Keep max 10GB
      compress_after_days: 7  # Compress after 7 days
```

Compressed sessions are ~10x smaller but still readable:

```bash
# Read compressed session
zstdcat sessions/2024-12-01/session_abc123.jsonl.zst | jq
```

---

## 📊 Performance Characteristics

### Passthrough Mode
- **Added latency**: 0.1-0.2ms (P95 < 0.5ms)
- **Memory overhead**: ~2MB baseline + ~1KB per request
- **CPU usage**: <1% idle, <5% at 100 RPS
- **API fidelity**: 100% (zero-copy proxy)

### With Session Recording
- **Added latency**: 0.5-1ms (async recording, non-blocking)
- **Disk I/O**: Batched writes every 100ms
- **Storage**: ~10KB per request (uncompressed), ~1KB (compressed)

### With Normalization (cross-provider)
- **Added latency**: 2-5ms (parsing + conversion)
- **Memory**: ~50KB per request (temporary buffers)
- **Fidelity**: 99%+ (some provider-specific fields may be lost)

---

## 🚨 Common Issues

### Issue: "No providers configured"

**Cause:** No providers enabled in config

**Fix:**

This warning is normal in passthrough mode! If you see requests working, ignore it. The warning appears when no `api_key` is configured, but passthrough mode forwards the client's API key automatically.

**If you actually want to configure a provider:**

```bash
# Option 1: Server-side API key via environment variable
export ANTHROPIC_API_KEY=sk-ant-...
lunaroute-server

# Option 2: Server-side API key in config file
providers:
  anthropic:
    enabled: true
    api_key: "sk-ant-..."  # Or use ${ANTHROPIC_API_KEY}

# Option 3: Client-provided API key (no server config needed - recommended!)
providers:
  anthropic:
    enabled: true
    # No api_key field = use client's x-api-key header
```

### Issue: "Connection refused" from Claude Code

**Cause:** Wrong base URL or server not running

**Fix:**
```bash
# 1. Check server is running
curl http://localhost:8081/healthz
# Should return: {"status":"ok"}

# 2. Check base URL
echo $ANTHROPIC_BASE_URL
# Should be: http://localhost:8081 (no /v1)

# 3. Set correctly
export ANTHROPIC_BASE_URL=http://localhost:8081
```

### Issue: Sessions not grouping correctly

**Cause:** Claude Code not sending session ID in metadata

**Fix:** LunaRoute will auto-generate UUIDs for each request. To group sessions, ensure Claude Code includes:
```json
{
  "metadata": {
    "user_id": "session_<uuid>"
  }
}
```

### Issue: High memory usage

**Cause:** Too many sessions tracked in memory

**Fix:**
```yaml
# Reduce max sessions tracked
session_stats_max_sessions: 50  # Default: 100

# Enable cleanup
session_recording:
  cleanup:
    retention:
      max_total_size_gb: 5
```

---

## 📚 Additional Resources

- **Configuration Reference**: `config.example.yaml`
- **Example Configs**: `examples/configs/`
- **API Documentation**: `crates/lunaroute-server/README.md`
- **Session Recording**: `crates/lunaroute-session/`
- **PII Detection**: `crates/lunaroute-pii/README.md`

---

## 🤝 Contributing

Found a bug or have a feature request? Open an issue!

**Common feature requests:**
- New PII detectors
- Additional metrics
- More routing strategies
- Custom storage backends
- UI for session browsing

---

## ⚖️ License

Licensed under either Apache License 2.0 or MIT License at your option.

---

**Built with ❤️ for Claude Code users who want visibility, control, and performance.**
