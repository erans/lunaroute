# 🌕 LunaRoute

```
         ___---___
      .--         --.
    ./   ()      .-. \.
   /   o    .   (   )  \
  / .            '-'    \    _                      ____             _
 | ()    .  O         .  |  | |   _   _ _ __   __ _|  _ \ ___  _   _| |_ ___
|                         | | |  | | | | '_ \ / _` | |_) / _ \| | | | __/ _ \
|    o           ()       | | |__| |_| | | | | (_| |  _ < (_) | |_| | ||  __/
|       .--.          O   | |_____\__,_|_| |_|\__,_|_| \_\___/ \__,_|\__\___|
 | .   |    |            |
  \    `.__.'    o   .  / 
   \                   /   
    `\  o    ()      /    
      `--___   ___--'
            ---

```
**Your AI Coding Assistant's Best Friend**

LunaRoute is a high-performance local proxy for AI coding assistants like Claude Code, OpenAI Codex CLI, and OpenCode. Get complete visibility into every LLM interaction with zero-overhead passthrough, comprehensive session recording, and powerful debugging capabilities.

```bash
# Start in 5 seconds (no API key needed!)
lunaroute-server

# Point Claude Code to it
export ANTHROPIC_BASE_URL=http://localhost:8081

# Or use with OpenAI Codex CLI
export OPENAI_BASE_URL=http://localhost:8081/v1

# Done! Start coding with full visibility
```

---

## 🎯 Why LunaRoute for Local Development?

### See Everything Your AI Does

Stop flying blind. LunaRoute records every conversation, token, and tool call:

- **🔍 Debug AI conversations** - See exactly what your AI assistant sends and receives
- **💰 Track token usage** - Know where your money goes (input/output/thinking tokens)
- **🔧 Analyze tool performance** - Which tools are slow? Which get used most?
- **📊 Measure proxy overhead** - Is it the LLM or your code that's slow?
- **🔎 Search past sessions** - "How did the AI solve that bug last week?"

### Privacy & Compliance Built-In

Keep your data safe while maintaining visibility:

- **🔒 Automatic PII redaction** - Detect and redact emails, SSN, credit cards, phone numbers
- **🎯 Multiple redaction modes** - Mask, remove, tokenize, or show partial data
- **🔐 Deterministic tokenization** - HMAC-based tokens for reversible redaction
- **📝 Custom patterns** - Add your own regex patterns for API keys, secrets, etc.
- **✅ Zero trust storage** - Redact before hitting disk, not after

Perfect for regulated industries and security-conscious teams.

### Zero Configuration Required

Literally just run it:

```bash
lunaroute-server
```

**That's it.** No API keys, no config files, nothing. Your AI assistant provides authentication automatically:
- **Claude Code** - Uses client headers for authentication
- **OpenAI Codex CLI** - Reads from `~/.codex/auth.json` automatically
- **Custom clients** - Pass your own API keys via headers

Your API key never needs to be configured in the proxy.

### Sub-Millisecond Performance

Built in Rust for speed and security:
- **0.1-0.2ms added latency** in passthrough mode
- **100% API fidelity** - preserves extended thinking, all response fields
- **Zero-copy routing** - no normalization overhead
- **PII redaction** - protect sensitive data automatically
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

### Configuration Examples

#### Example 1: Passthrough for Claude Code

Zero-overhead passthrough with 100% API fidelity:

```yaml
# Save as claude-passthrough.yaml
host: "127.0.0.1"
port: 8081
api_dialect: "anthropic"  # Accept Anthropic format

providers:
  anthropic:
    enabled: true
    # No api_key needed - Claude Code sends it via x-api-key header

session_recording:
  enabled: false  # Disable for maximum performance

logging:
  level: "info"
  log_requests: true
```

**Usage:**
```bash
# Start server
lunaroute-server --config claude-passthrough.yaml

# Point Claude Code to proxy
export ANTHROPIC_BASE_URL=http://localhost:8081

# Use Claude Code normally - zero config needed!
```

**Performance:** ~0.1-0.2ms overhead, 100% API fidelity

---

#### Example 2: Passthrough for OpenAI Codex CLI

Works with your ChatGPT account via Codex authentication:

```yaml
# Save as codex-passthrough.yaml
host: "127.0.0.1"
port: 8081
api_dialect: "openai"  # Accept OpenAI format

providers:
  openai:
    enabled: true
    # Special base URL for ChatGPT authentication
    base_url: "https://chatgpt.com/backend-api/codex"
    codex_auth:
      enabled: false  # Disabled for pure passthrough

session_recording:
  enabled: false

logging:
  level: "info"
  log_requests: true
```

**Usage:**
```bash
# Start server (reads auth from ~/.codex/auth.json)
lunaroute-server --config codex-passthrough.yaml

# Point Codex to proxy
export OPENAI_BASE_URL=http://localhost:8081/v1

# Use Codex normally with your ChatGPT account
codex exec "help me debug this function"
```

**Note:** Use `base_url: "https://api.openai.com/v1"` if you have an OpenAI API key instead.

---

#### Example 3: Dual-Dialect Passthrough (Both Claude Code & Codex)

Run a single proxy that accepts both OpenAI and Anthropic formats simultaneously:

```yaml
# Save as dual-passthrough.yaml
host: "127.0.0.1"
port: 8081
api_dialect: "both"  # Accept BOTH OpenAI and Anthropic formats

providers:
  openai:
    enabled: true
    # For Codex with ChatGPT account
    base_url: "https://chatgpt.com/backend-api/codex"
    codex_auth:
      enabled: true  # Reads auth from ~/.codex/auth.json automatically
    # For OpenAI API instead, use: base_url: "https://api.openai.com/v1"

  anthropic:
    enabled: true
    # No api_key needed - Claude Code sends it via x-api-key header

session_recording:
  enabled: true

  # SQLite analytics - lightweight session stats
  sqlite:
    enabled: true
    path: "~/.lunaroute/sessions.db"
    max_connections: 10

  # JSONL logs - full request/response recording
  jsonl:
    enabled: true
    directory: "~/.lunaroute/sessions"
    retention:
      max_age_days: 30
      max_size_mb: 1024

# Web UI for browsing sessions
ui:
  enabled: true
  host: "127.0.0.1"
  port: 8082
  refresh_interval: 5

logging:
  level: "info"
  log_requests: true
```

**Usage:**
```bash
# Start server - ONE proxy for BOTH tools!
lunaroute-server --config dual-passthrough.yaml

# Point Claude Code to proxy
export ANTHROPIC_BASE_URL=http://localhost:8081

# Point Codex to proxy (in another terminal or project)
export OPENAI_BASE_URL=http://localhost:8081

# Use both tools simultaneously - same proxy!
# Claude Code → /v1/messages → Anthropic
# Codex CLI   → /v1/chat/completions → OpenAI

# All sessions recorded to ~/.lunaroute/sessions/
# Analytics stored in ~/.lunaroute/sessions.db
# Browse sessions at http://localhost:8082
```

**What happens:**
1. LunaRoute accepts requests at both `/v1/messages` (Anthropic) and `/v1/chat/completions` (OpenAI)
2. Anthropic requests → passthrough to Anthropic provider
3. OpenAI requests → passthrough to OpenAI provider (with Codex auth)
4. Both formats work simultaneously with zero normalization overhead
5. Each tool gets native responses in its expected format
6. All sessions recorded with full request/response logs + SQLite analytics
7. Web UI available at http://localhost:8082 for session browsing and analysis

**Performance:** ~0.5-1ms overhead (async recording), 100% API fidelity for both formats

**Use Cases:**
- Use Claude Code and Codex in the same development environment
- Single proxy for team using mixed AI tools
- Consolidated logging and metrics for all AI interactions
- Unified session recording across both OpenAI and Anthropic APIs
- Visual session analysis through web UI
- Simplified infrastructure (one proxy instead of two)

---

#### Example 4: Map Claude Code to Gemini (OpenAI dialect)

Translate Anthropic format to Gemini's OpenAI-compatible endpoint:

```yaml
# Save as claude-to-gemini.yaml
host: "127.0.0.1"
port: 8081
api_dialect: "anthropic"  # Accept Anthropic format from Claude Code

providers:
  openai:
    enabled: true
    # Gemini's OpenAI-compatible endpoint
    base_url: "https://generativelanguage.googleapis.com/v1beta/openai"
    api_key: "${GOOGLE_API_KEY}"  # Set via environment variable

  anthropic:
    enabled: false  # Don't route to Anthropic

session_recording:
  enabled: true  # Optional: track translations

logging:
  level: "debug"
  log_requests: true
```

**Usage:**
```bash
# Set your Google API key
export GOOGLE_API_KEY="your-gemini-api-key"

# Start server
lunaroute-server --config claude-to-gemini.yaml

# Point Claude Code to proxy
export ANTHROPIC_BASE_URL=http://localhost:8081

# Claude Code sends Anthropic format, LunaRoute translates to Gemini!
```

**What happens:**
1. Claude Code sends request in Anthropic format
2. LunaRoute translates to OpenAI format
3. Request routes to Gemini's OpenAI endpoint
4. Response translates back to Anthropic format
5. Claude Code receives native Anthropic response

---

**More examples:** See `examples/configs/` for PII redaction, routing strategies, and advanced features.

---

### Controlling Session Recording

LunaRoute provides two independent recording modes that can be enabled/disabled separately:

#### SQLite Analytics (Session Stats)

Lightweight session statistics and metadata - perfect for tracking usage without storing full content:

```yaml
session_recording:
  enabled: true
  sqlite:
    enabled: true                        # Enable SQLite analytics
    path: "~/.lunaroute/sessions.db"     # Database location
    max_connections: 10                  # Connection pool size
  jsonl:
    enabled: false                       # Disable full request/response logs
```

**What gets recorded:** Session IDs, timestamps, token counts, model names, tool usage stats, durations
**Storage:** ~1-2KB per session
**Use case:** Track costs and performance without storing sensitive request/response data

#### JSONL Request/Response Logs

Complete request/response recording for debugging and analysis:

```yaml
session_recording:
  enabled: true
  jsonl:
    enabled: true                                # Enable full logs
    directory: "~/.lunaroute/sessions"           # Log directory
    retention:
      max_age_days: 30                           # Delete logs older than 30 days
      max_size_mb: 1024                          # Delete oldest when > 1GB
  sqlite:
    enabled: false                               # Disable analytics database
```

**What gets recorded:** Full request/response bodies, headers, streaming events, tool calls
**Storage:** ~10KB per request (varies with content size)
**Use case:** Debug conversations, replay sessions, analyze AI behavior

#### Both Enabled (Recommended)

Get the best of both worlds:

```yaml
session_recording:
  enabled: true

  # Quick queryable stats
  sqlite:
    enabled: true
    path: "~/.lunaroute/sessions.db"

  # Full conversation logs
  jsonl:
    enabled: true
    directory: "~/.lunaroute/sessions"
    retention:
      max_age_days: 7                            # Keep full logs for 1 week
      max_size_mb: 512                           # Limit to 512MB
```

**Use case:** Analytics stay forever, but full logs are cleaned up after 7 days

#### Disable All Recording (Maximum Performance)

```yaml
session_recording:
  enabled: false  # Master switch - disables everything
```

**Or via environment variable:**
```bash
LUNAROUTE_ENABLE_SESSION_RECORDING=false lunaroute-server
```

**Performance:** Sub-millisecond overhead (~0.1-0.2ms), perfect for production

#### Environment Variables

Control recording without a config file:

```bash
# Master switch
export LUNAROUTE_ENABLE_SESSION_RECORDING=true

# SQLite analytics
export LUNAROUTE_ENABLE_SQLITE_WRITER=true
export LUNAROUTE_SESSIONS_DB_PATH="~/.lunaroute/sessions.db"

# JSONL logs
export LUNAROUTE_ENABLE_JSONL_WRITER=true
export LUNAROUTE_SESSIONS_DIR="~/.lunaroute/sessions"

# Start server
lunaroute-server
```

---

### Zero-Config Quick Start

Prefer to skip config files? Just run:

```bash
# For Claude Code (Anthropic passthrough)
lunaroute-server
export ANTHROPIC_BASE_URL=http://localhost:8081

# For Codex CLI (reads ~/.codex/auth.json automatically)
lunaroute-server
export OPENAI_BASE_URL=http://localhost:8081/v1
```

Your API keys are provided by the client. No configuration needed!

---

## 💡 Real-World Use Cases

### 1. Debug Expensive Conversations

**Problem:** Your AI session cost $5 but you don't know why.

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

**Result:** Discover the AI's responses are verbose. Adjust system prompt to be more concise.

### 2. Identify Performance Bottlenecks

**Problem:** Your AI assistant feels slow. Is it the LLM or your tools?

**Solution:** Check session statistics:

```
Tool usage:
  Read:  12 calls (avg 45ms)   ← Fast
  Write: 8 calls (avg 120ms)   ← Reasonable
  Bash:  3 calls (avg 850ms)   ← SLOW! Optimize these
```

**Result:** Bash commands are the bottleneck, not the LLM.

### 3. Search Past Conversations

**Problem:** "How did my AI assistant help me fix that TypeError last week?"

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

  openai:
    enabled: true
    # Codex users get automatic auth from ~/.codex/auth.json
```

**Result:**
- Claude Code users send their own API keys via headers
- Codex CLI users get automatic authentication from their profile
- No shared secrets, everyone uses their own credentials

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
- **Custom storage backends** (S3, CloudWatch, GCS - implement your own)

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
│          Claude Code / OpenAI Codex CLI / OpenCode         │
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
- `lunaroute-egress` - Provider connectors with connection pooling and Codex auth
- `lunaroute-session` - Recording and search
- `lunaroute-pii` - PII detection/redaction
- `lunaroute-observability` - Metrics and health
- `lunaroute-server` - Production binary

### Connection Pooling

LunaRoute uses HTTP connection pooling for optimal performance:

**Default Settings:**
- Request timeout: 600s (for long streaming sessions)
- Connection timeout: 10s
- Pool size per host: 32 idle connections
- Idle timeout: 90s (expires before server timeout)
- TCP keepalive: 60s (keeps long requests alive)

**Tunable via YAML:**
```yaml
providers:
  openai:
    http_client:
      timeout_secs: 300
      pool_max_idle_per_host: 64
      pool_idle_timeout_secs: 120
```

**Or Environment Variables:**
```bash
export LUNAROUTE_OPENAI_TIMEOUT_SECS=300
export LUNAROUTE_OPENAI_POOL_MAX_IDLE=64
```

See [Connection Pool Configuration](docs/CONNECTION_POOL_ENV_VARS.md) for complete tuning guide.

---

## 📚 Documentation

- **[Claude Code Guide](CLAUDE_CODE_GUIDE.md)** - Complete guide for Claude Code integration
- **[Server README](crates/lunaroute-server/README.md)** - Configuration reference
- **[Config Examples](examples/configs/README.md)** - Pre-built configs for common scenarios
- **[Connection Pool Configuration](docs/CONNECTION_POOL_ENV_VARS.md)** - HTTP client pool tuning
- **[PII Detection](crates/lunaroute-pii/README.md)** - PII redaction details
- **[TODO.md](TODO.md)** - Roadmap and implementation status

### Supported AI Assistants

- ✅ **Claude Code** - Full passthrough support, zero config
- ✅ **OpenAI Codex CLI** - Automatic auth.json integration
- ✅ **OpenCode** - Standard OpenAI/Anthropic API compatibility
- ✅ **Custom Clients** - Any tool using OpenAI or Anthropic APIs

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

**Recently Implemented:**
- ✅ Custom storage backends (S3, CloudWatch, GCS)

**Popular feature requests:**
- UI for session browsing
- More routing strategies
- Additional provider connectors

---

## 📝 License

Licensed under the Apache License, Version 2.0 ([LICENSE](LICENSE)).

---

## 🌟 Why "LunaRoute"?

Like the moon 🌕 guides travelers at night, LunaRoute illuminates your AI interactions. Every request, every token, every decision - visible and trackable.

**Built with ❤️ for developers who want visibility, control, and performance.**

### OpenAI Codex CLI Support

LunaRoute provides first-class support for OpenAI's Codex CLI:
- ✅ **Automatic authentication** from `~/.codex/auth.json`
- ✅ **Account ID header injection** for proper request routing
- ✅ **Zero configuration** - just point Codex at the proxy
- ✅ **Full compatibility** with all Codex commands

Works seamlessly with `codex exec`, `codex chat`, `codex eval`, and more!

---

## 🎨 Web UI

LunaRoute includes a built-in web interface for browsing and analyzing sessions:

```bash
# The UI server starts automatically on port 8082
lunaroute-server

# Then open: http://localhost:8082
```

**Features:**
- 📊 **Dashboard** - View all sessions with filtering and search
- 🔍 **Session Details** - Inspect individual sessions with timeline view
- 📄 **Raw Request/Response** - View the complete JSON data
- 📈 **Analytics** - Token usage, tool statistics, and performance metrics
- ⌨️ **Keyboard Shortcuts** - Press `ESC` to close dialogs

**Configuration:**
```yaml
ui:
  enabled: true
  host: "127.0.0.1"
  port: 8082
  refresh_interval: 5
```

---

## 🚀 Coming Soon

We're focusing on local development first, but here's what's next:

- **Production Routing** - Intelligent load balancing for production traffic
- **Cost Optimization** - Automatic routing to cheapest provider
- **CI/CD Integration** - Test suite recording and playback
- **Multi-Region** - Geo-routing and disaster recovery

Want to help shape the roadmap? [Open an issue](https://github.com/yourusername/lunaroute/issues)!

---

<p align="center">
  <strong>Give your AI coding assistant the visibility it deserves.</strong><br>
  <code>lunaroute-server</code>
</p>
