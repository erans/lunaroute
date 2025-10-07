# 🌙 LunaRoute

**Your AI Coding Assistant's Best Friend**

LunaRoute is a high-performance local proxy for AI coding assistants like Claude Code and OpenCode. Get complete visibility into every LLM interaction with zero-overhead passthrough, comprehensive session recording, and powerful debugging capabilities.

```bash
# Start in 5 seconds (no API key needed!)
lunaroute-server

# Point Claude Code to it
export ANTHROPIC_BASE_URL=http://localhost:8081

# Done! Start coding with full visibility
```

---

## 🎯 Why LunaRoute for Local Development?

### See Everything Your AI Does

Stop flying blind. LunaRoute records every conversation, token, and tool call:

- **🔍 Debug AI conversations** - See exactly what Claude Code sends and receives
- **💰 Track token usage** - Know where your money goes (input/output/thinking tokens)
- **🔧 Analyze tool performance** - Which tools are slow? Which get used most?
- **📊 Measure proxy overhead** - Is it the LLM or your code that's slow?
- **🔎 Search past sessions** - "How did Claude solve that bug last week?"

### Zero Configuration Required

Literally just run it:

```bash
lunaroute-server
```

**That's it.** No API keys, no config files, nothing. Claude Code provides authentication automatically through client headers. Your API key never touches the proxy server.

### Sub-Millisecond Performance

Built in Rust for speed:
- **0.1-0.2ms added latency** in passthrough mode
- **100% API fidelity** - preserves extended thinking, all response fields
- **Zero-copy routing** - no normalization overhead
- **Production-ready** - 73% code coverage, 544 tests passing

---

## 🚀 Quick Start

### Installation

```bash
# Clone the repo
git clone https://github.com/yourusername/lunaroute.git
cd lunaroute

# Build (one time only)
cargo build --release --package lunaroute-server

# Binary location: target/release/lunaroute-server
# Add to PATH or run directly
```

### Option 1: Zero-Config Passthrough (Recommended) 🔥

Maximum performance with complete visibility:

```bash
# 1. Start LunaRoute (no API key needed!)
lunaroute-server

# 2. Point Claude Code to the proxy
export ANTHROPIC_BASE_URL=http://localhost:8081

# 3. Use Claude Code normally - full visibility enabled!
```

Your API key stays in Claude Code. The proxy forwards it automatically.

### Option 2: With Session Recording

Record every conversation for later analysis:

```bash
# Enable recording with environment variables
LUNAROUTE_ENABLE_SESSION_RECORDING=true \
LUNAROUTE_LOG_LEVEL=debug \
lunaroute-server

# Or use a config file
lunaroute-server --config examples/configs/claude-code-proxy-with-recording.yaml
```

Sessions saved to `~/.lunaroute/sessions/` in both JSONL (human-readable) and SQLite (queryable) formats.

---

## 💡 Real-World Use Cases

### 1. Debug Expensive Conversations

**Problem:** Your Claude Code session cost $5 but you don't know why.

**Solution:** Check the session stats on shutdown:

```
Session: abc123
  Requests:        12
  Input tokens:    5,420
  Output tokens:   28,330  ← This is why!
  Thinking tokens: 2,100

  Tool usage:
    Read:  45 calls (avg 30ms)
    Write: 8 calls (avg 120ms)
```

**Result:** Discover Claude's responses are verbose. Adjust system prompt to be more concise.

### 2. Identify Performance Bottlenecks

**Problem:** Claude Code feels slow. Is it the LLM or your tools?

**Solution:** Check session statistics:

```
Tool usage:
  Read:  12 calls (avg 45ms)   ← Fast
  Write: 8 calls (avg 120ms)   ← Reasonable
  Bash:  3 calls (avg 850ms)   ← SLOW! Optimize these
```

**Result:** Bash commands are the bottleneck, not the LLM.

### 3. Search Past Conversations

**Problem:** "How did Claude help me fix that TypeError last week?"

**Solution:**

```bash
# Search all sessions for "TypeError"
curl "http://localhost:8081/sessions?text_search=TypeError&days=7"

# Get the full conversation
curl "http://localhost:8081/sessions/{session_id}" | jq
```

**Result:** Find the exact solution approach and reuse it.

### 4. Team Collaboration

**Problem:** Team shares one proxy but everyone has different API keys.

**Solution:** Run LunaRoute without configuring API keys:

```yaml
providers:
  anthropic:
    enabled: true
    # No api_key field = use client headers
```

**Result:** Each developer's Claude Code sends their own key. No shared secrets.

### 5. Privacy & Compliance

**Problem:** Need to log sessions but can't store PII.

**Solution:** Enable automatic PII redaction:

```yaml
session_recording:
  pii:
    enabled: true
    detect_email: true
    detect_phone: true
    detect_ssn: true
    redaction_mode: "tokenize"
```

**Result:** All PII auto-redacted before hitting disk. Compliance achieved.

---

## ✨ Key Features

### 🎯 **Zero-Overhead Passthrough Mode**

When your API dialect matches the provider (Anthropic→Anthropic), LunaRoute becomes a transparent proxy:

- Sub-millisecond overhead (~0.1-0.2ms)
- 100% API fidelity (preserves all fields)
- Optional session recording
- No normalization layer

### 📊 **Comprehensive Session Recording**

Dual-format storage for maximum flexibility:

**JSONL Format (Human-Readable):**
```jsonl
{"session_id":"abc123","model":"claude-sonnet-4","started_at":"2025-01-06T10:30:00Z"}
{"session_id":"abc123","request":"Help me debug this...","input_tokens":150}
{"session_id":"abc123","response":"I'll help you...","output_tokens":420}
```

**SQLite Format (Queryable):**
```sql
SELECT * FROM sessions
WHERE model_used = 'claude-sonnet-4'
  AND input_tokens > 1000
ORDER BY started_at DESC;
```

**Features:**
- Session grouping (multi-turn conversations in one file)
- Async recording (non-blocking, batched writes)
- Compression (Zstd after 7 days, ~10x smaller)
- Retention policies (age-based, size-based cleanup)

### 📈 **Session Statistics**

Track everything that matters:

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

### 🔍 **Advanced Session Search**

Find anything, instantly:

```bash
# Full-text search
curl "http://localhost:8081/sessions?text_search=TypeError"

# Filter by tokens
curl "http://localhost:8081/sessions?min_total_tokens=10000"

# By model and date
curl "http://localhost:8081/sessions?model=claude-sonnet-4&days=7"

# Failed requests
curl "http://localhost:8081/sessions?success=false"
```

### 🔒 **PII Detection & Redaction**

Protect sensitive data automatically:

**Supported PII Types:**
- Email addresses, phone numbers, SSN
- Credit card numbers (Luhn validation)
- IP addresses (IPv4/IPv6)
- Custom patterns (API keys, AWS secrets, etc.)

**Redaction Modes:**
- **Mask**: `[EMAIL]`, `[PHONE]`, etc.
- **Remove**: Delete completely
- **Tokenize**: HMAC-based deterministic tokens (reversible with key)
- **Partial**: Show last N characters

**Before:** `My email is john.doe@example.com and SSN is 123-45-6789`
**After:** `My email is [EMAIL:a3f8e9d2] and SSN is [SSN:7b2c4f1a]`

### 📊 **Prometheus Metrics**

24 metric types at `/metrics`:

- Request rates (total, success, failure)
- Latency histograms (P50, P95, P99)
- Token usage (input/output/thinking)
- Tool call statistics (per-tool breakdown)
- Circuit breaker states
- Streaming performance (TTFT, chunk latency)

Perfect for Grafana dashboards.

### 🔧 **Request Logging**

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
│ 📝 I'll help you debug that function...
│ 🔧 Tool call: Read
│ 📊 Usage: input=150, output=420, total=570
│ 🏁 Stream ended: EndTurn
└─────────────────────────────────────────────────────────
```

---

## 🏗️ Architecture

LunaRoute is built as a modular Rust workspace:

```
┌─────────────────────────────────────────────────────────────┐
│                        Claude Code                          │
└─────────────────────────┬───────────────────────────────────┘
                          │ HTTP/SSE
                          ↓
┌─────────────────────────────────────────────────────────────┐
│                      LunaRoute Proxy                        │
│  ┌──────────────────────────────────────────────────────┐  │
│  │  Ingress (Anthropic/OpenAI endpoints)                │  │
│  └─────────────────────┬────────────────────────────────┘  │
│                        │                                    │
│  ┌─────────────────────▼────────────────────────────────┐  │
│  │  Passthrough Mode (Zero-copy) OR Normalization       │  │
│  └─────────────────────┬────────────────────────────────┘  │
│                        │                                    │
│  ┌─────────────────────▼────────────────────────────────┐  │
│  │  Session Recording (JSONL + SQLite, PII redaction)   │  │
│  └─────────────────────┬────────────────────────────────┘  │
│                        │                                    │
│  ┌─────────────────────▼────────────────────────────────┐  │
│  │  Metrics & Statistics (Prometheus, session stats)    │  │
│  └─────────────────────┬────────────────────────────────┘  │
│                        │                                    │
│  ┌─────────────────────▼────────────────────────────────┐  │
│  │  Egress (Provider connectors)                         │  │
│  └──────────────────────────────────────────────────────┘  │
└─────────────────────────┬───────────────────────────────────┘
                          │ HTTP/SSE
                          ↓
              ┌───────────────────────┐
              │  Anthropic API        │
              │  (api.anthropic.com)  │
              └───────────────────────┘
```

**Key Crates:**
- `lunaroute-core` - Types and traits
- `lunaroute-ingress` - HTTP endpoints (OpenAI, Anthropic)
- `lunaroute-egress` - Provider connectors
- `lunaroute-session` - Recording and search
- `lunaroute-pii` - PII detection/redaction
- `lunaroute-observability` - Metrics and health
- `lunaroute-server` - Production binary

---

## 📚 Documentation

- **[Claude Code Guide](CLAUDE_CODE_GUIDE.md)** - Complete guide for local development
- **[Server README](crates/lunaroute-server/README.md)** - Configuration reference
- **[Config Examples](examples/configs/README.md)** - Pre-built configs for common scenarios
- **[PII Detection](crates/lunaroute-pii/README.md)** - PII redaction details
- **[TODO.md](TODO.md)** - Roadmap and implementation status

---

## 🎓 Pro Tips

### Watch Sessions in Real-Time

```bash
# Terminal 1: Start LunaRoute
lunaroute-server

# Terminal 2: Watch sessions being created
watch -n 1 'ls -lh ~/.lunaroute/sessions/$(date +%Y-%m-%d)/'

# Terminal 3: Tail the latest session
tail -f ~/.lunaroute/sessions/$(date +%Y-%m-%d)/session_*.jsonl | jq
```

### Query Sessions with jq

```bash
# Get total tokens from today's sessions
cat ~/.lunaroute/sessions/$(date +%Y-%m-%d)/*.jsonl | \
  jq -s '[.[] | select(.final_stats) | .final_stats.tokens.total] | add'

# Find most expensive session
cat ~/.lunaroute/sessions/$(date +%Y-%m-%d)/*.jsonl | \
  jq -s 'group_by(.session_id) |
         map({session: .[0].session_id, tokens: ([.[] | .final_stats.tokens.total // 0] | add)}) |
         sort_by(.tokens) | reverse | first'
```

### Prometheus + Grafana

Create beautiful dashboards:

```bash
# prometheus.yml
scrape_configs:
  - job_name: 'lunaroute'
    static_configs:
      - targets: ['localhost:8081']
    metrics_path: '/metrics'
    scrape_interval: 5s
```

Track:
- Requests per minute
- P95 latency trends
- Token usage over time
- Tool call frequency
- Daily costs

---

## 📊 Performance

### Passthrough Mode
- **Added latency**: 0.1-0.2ms (P95 < 0.5ms)
- **Memory overhead**: ~2MB baseline + ~1KB per request
- **CPU usage**: <1% idle, <5% at 100 RPS
- **API fidelity**: 100% (zero-copy proxy)

### With Session Recording
- **Added latency**: 0.5-1ms (async, non-blocking)
- **Disk I/O**: Batched writes every 100ms
- **Storage**: ~10KB per request (uncompressed), ~1KB (compressed)

### Quality Metrics
- **Test coverage**: 73.35% (2042/2784 lines)
- **Unit tests**: 544 passing
- **Integration tests**: 11 test files
- **Clippy warnings**: 0

---

## 🤝 Contributing

We welcome contributions! Whether it's:
- Bug reports and fixes
- New PII detectors
- Additional metrics
- Documentation improvements
- Performance optimizations

Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

**Popular feature requests:**
- UI for session browsing
- More routing strategies
- Additional provider connectors
- Custom storage backends

---

## 📝 License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.

---

## 🌟 Why "LunaRoute"?

Like the moon 🌙 guides travelers at night, LunaRoute illuminates your AI interactions. Every request, every token, every decision - visible and trackable.

**Built with ❤️ for developers who want visibility, control, and performance.**

---

## 🚀 Coming Soon

We're focusing on local development first, but here's what's next:

- **Production Routing** - Intelligent load balancing for production traffic
- **Cost Optimization** - Automatic routing to cheapest provider
- **CI/CD Integration** - Test suite recording and playback
- **Multi-Region** - Geo-routing and disaster recovery
- **Web UI** - Browse and analyze sessions visually

Want to help shape the roadmap? [Open an issue](https://github.com/yourusername/lunaroute/issues)!

---

<p align="center">
  <strong>Give your AI coding assistant the visibility it deserves.</strong><br>
  <code>lunaroute-server</code>
</p>
