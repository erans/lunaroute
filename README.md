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

A blazing-fast local proxy for AI coding assistants that gives you complete visibility into every LLM interaction. Zero configuration, sub-millisecond overhead, and powerful debugging capabilities.

## ⚡ Quick Start - Literally One Command

```bash
eval $(lunaroute-server env)
```

**That's it!** This single command:
- ✅ Starts LunaRoute server in the background
- ✅ Configures Claude Code (sets `ANTHROPIC_BASE_URL`)
- ✅ Configures Codex CLI (sets `OPENAI_BASE_URL`)
- ✅ Accepts **both** OpenAI and Anthropic formats simultaneously
- ✅ Tracks every token, tool call, and conversation

Start coding with your AI assistant immediately - both APIs are ready to use!

---

### Alternative: Manual Setup

If you prefer to run the server manually:

```bash
# Terminal 1: Start the server
lunaroute-server

# Terminal 2: Point your AI tools to it
export ANTHROPIC_BASE_URL=http://localhost:8081  # For Claude Code
export OPENAI_BASE_URL=http://localhost:8081/v1  # For Codex CLI
```

**That's it.** No API keys to configure, no YAML files to write, nothing. LunaRoute automatically:
- ✅ Accepts **both** OpenAI and Anthropic formats simultaneously
- ⚡ Runs in **dual passthrough mode** (zero normalization overhead)
- 🔑 Uses your existing API keys (from env vars or client headers)
- 📊 Tracks every token, tool call, and conversation
- 🎯 Routes requests based on model prefix (`gpt-*` → OpenAI, `claude-*` → Anthropic)

---

## 🎯 Why LunaRoute?

### See Everything Your AI Does

Stop flying blind. LunaRoute records every interaction with zero configuration:

- **🔍 Debug AI conversations** - See exactly what your assistant sends and receives
- **💰 Track token usage** - Input, output, and thinking tokens broken down by session
- **🔧 Analyze tool performance** - Which tools are slow? Which get used most?
- **📊 Measure overhead** - Is it the LLM or your code that's slow?
- **🔎 Search past sessions** - "How did the AI solve that bug last week?"

### Zero Configuration, Maximum Power

```bash
# Literally just one command
eval $(lunaroute-server env)
```

**What you get instantly:**
- ⚡ **Dual API support** - OpenAI `/v1/chat/completions` + Anthropic `/v1/messages`
- 🚀 **Passthrough mode** - Sub-millisecond overhead, 100% API fidelity
- 🔑 **Automatic auth** - Uses environment variables or client-provided keys
- 📊 **Session tracking** - SQLite database + JSONL logs for deep analysis
- 🎨 **Web UI** - Browse sessions at `http://localhost:8082`
- 🔄 **Background server** - Detached process, continues running even after terminal closes

**How it works:**
```bash
$ lunaroute-server env
export ANTHROPIC_BASE_URL=http://127.0.0.1:8081
export OPENAI_BASE_URL=http://127.0.0.1:8081/v1
# LunaRoute server started on http://127.0.0.1:8081
# Web UI available at http://127.0.0.1:8082

$ eval $(lunaroute-server env)  # Sets env vars and starts server

$ # Now use your AI tools normally - they're automatically configured!
```

### Privacy & Performance

- **🔒 PII redaction** - Auto-detect and redact emails, SSN, credit cards, phone numbers
- ⚡ **0.1-0.2ms overhead** - Sub-millisecond proxy latency in passthrough mode
- 🛡️ **Zero trust storage** - Redact before hitting disk, not after
- 🔐 **Local first** - All data stays on your machine

---

## 📦 Installation

### Option 1: Download Binary (Recommended)

Download the latest release for your platform from [GitHub Releases](https://github.com/erans/lunaroute/releases):

```bash
# Linux/macOS: Extract and run
tar -xzf lunaroute-server-*.tar.gz
chmod +x lunaroute-server

# Optional: Add to PATH for global access
sudo mv lunaroute-server /usr/local/bin/

# Start using it immediately!
eval $(lunaroute-server env)
```

### Option 2: Build from Source

```bash
git clone https://github.com/erans/lunaroute.git
cd lunaroute
cargo build --release --package lunaroute-server

# Binary location: target/release/lunaroute-server

# Start using it!
eval $(./target/release/lunaroute-server env)
```

---

## 🚀 Usage

### Recommended: One-Command Setup

The fastest way to get started - automatically starts server and configures your shell:

```bash
# One command does everything!
eval $(lunaroute-server env)

# Now use your AI tools - they're automatically configured
# Both Claude Code and Codex CLI work immediately
```

**What happens:**
- Server starts in background on port 8081
- `ANTHROPIC_BASE_URL` set to `http://127.0.0.1:8081`
- `OPENAI_BASE_URL` set to `http://127.0.0.1:8081/v1`
- Web UI available at `http://127.0.0.1:8082`

**Custom port:**
```bash
eval $(lunaroute-server env --port 8090)
```

**Stop the server:**
```bash
pkill -f "lunaroute-server serve"
```

---

### Alternative: Manual Mode

If you prefer to manage the server yourself:

```bash
# Terminal 1: Start LunaRoute
lunaroute-server

# Terminal 2: Configure your shell
export ANTHROPIC_BASE_URL=http://localhost:8081
export OPENAI_BASE_URL=http://localhost:8081/v1

# Now use Claude Code or Codex CLI
# View sessions at http://localhost:8082
```

**What you get out of the box:**
```
✓ OpenAI provider enabled (no API key - will use client auth)
✓ Anthropic provider enabled (no API key - will use client auth)
📡 API dialect: Both (OpenAI + Anthropic)
⚡ Dual passthrough mode: OpenAI→OpenAI + Anthropic→Anthropic (no normalization)
   - gpt-* models    → OpenAI provider (passthrough)
   - claude-* models → Anthropic provider (passthrough)
🚀 Bypass enabled for unknown API paths
📊 Session recording enabled (SQLite + JSONL)
🎨 Web UI available at http://localhost:8082
```

### With Custom Configuration

Need more control? Use a config file:

```yaml
# Save as config.yaml
host: "127.0.0.1"
port: 8081
api_dialect: "both"  # Already the default!

providers:
  openai:
    enabled: true
    base_url: "https://api.openai.com/v1"  # Or use ChatGPT backend
    # api_key: "sk-..."  # Optional: defaults to OPENAI_API_KEY env var

  anthropic:
    enabled: true
    # api_key: "sk-ant-..."  # Optional: defaults to ANTHROPIC_API_KEY env var

session_recording:
  enabled: true
  sqlite:
    enabled: true
    path: "~/.lunaroute/sessions.db"
  jsonl:
    enabled: true
    directory: "~/.lunaroute/sessions"
    retention:
      max_age_days: 30
      max_size_mb: 1024

ui:
  enabled: true
  host: "127.0.0.1"
  port: 8082
```

```bash
lunaroute-server --config config.yaml
```

---

## ✨ Key Features

### 🎯 Dual-Dialect Passthrough (Default)

LunaRoute accepts **both** OpenAI and Anthropic formats simultaneously with zero normalization:

- **OpenAI format** at `/v1/chat/completions` → routes to OpenAI
- **Anthropic format** at `/v1/messages` → routes to Anthropic
- **Zero overhead** - ~0.1-0.2ms added latency
- **100% API fidelity** - preserves extended thinking, all response fields
- **No normalization** - direct passthrough to native API

### 📊 Comprehensive Session Recording

Track everything that matters with dual storage:

**SQLite Database** - Fast queries and analytics:
```sql
SELECT model_used, COUNT(*), SUM(input_tokens), SUM(output_tokens)
FROM sessions
WHERE started_at > datetime('now', '-7 days')
GROUP BY model_used;
```

**JSONL Logs** - Human-readable, full request/response data:
```bash
# Watch live sessions
tail -f ~/.lunaroute/sessions/$(date +%Y-%m-%d)/session_*.jsonl | jq

# Search for specific content
grep -r "TypeError" ~/.lunaroute/sessions/
```

### 📈 Session Statistics

Get detailed breakdowns on shutdown or via API:

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

### 🔒 PII Detection & Redaction

Protect sensitive data automatically before it hits disk:

```yaml
session_recording:
  pii:
    enabled: true
    detect_email: true
    detect_phone: true
    detect_ssn: true
    detect_credit_card: true
    redaction_mode: "tokenize"  # mask, remove, tokenize, or partial
```

**Before:** `My email is john.doe@example.com and SSN is 123-45-6789`
**After:** `My email is [EMAIL:a3f8e9d2] and SSN is [SSN:7b2c4f1a]`

### 📊 Prometheus Metrics

24 metric types at `/metrics`:
- Request rates (total, success, failure)
- Latency histograms (P50, P95, P99)
- Token usage (input/output/thinking)
- Tool call statistics
- Streaming performance

Perfect for Grafana dashboards.

### 🎨 Web UI

Built-in web interface for browsing sessions:

```bash
# Automatically available at http://localhost:8082
lunaroute-server
```

**Features:**
- 📊 Dashboard with filtering and search
- 🔍 Session details with timeline view
- 📄 Raw JSON inspection
- 📈 Token usage and performance analytics

---

## 🔧 Advanced Configuration

### Quick Setup with `env` Command

The `env` command starts the server in the background and outputs shell commands to configure your environment:

```bash
# Basic usage - starts server on default port 8081
eval $(lunaroute-server env)

# Custom port
eval $(lunaroute-server env --port 8090)

# Custom host and port
eval $(lunaroute-server env --host 0.0.0.0 --port 8090)

# Check what it does without executing
lunaroute-server env
# Output:
#   export ANTHROPIC_BASE_URL=http://127.0.0.1:8081
#   export OPENAI_BASE_URL=http://127.0.0.1:8081/v1
#   # LunaRoute server started on http://127.0.0.1:8081
#   # Web UI available at http://127.0.0.1:8082
```

**What happens:**
- Server starts in background (detached from terminal)
- Environment variables are set in your current shell
- Server continues running even if you close the terminal
- Both Claude Code and Codex CLI are instantly configured

**Stop the server:**
```bash
pkill -f "lunaroute-server serve"
```

### Environment Variables

Control behavior without config files:

```bash
# API dialect (default: both)
export LUNAROUTE_DIALECT=both  # openai, anthropic, or both

# Provider API keys (optional - can use client headers)
export OPENAI_API_KEY="sk-..."
export ANTHROPIC_API_KEY="sk-ant-..."

# Session recording
export LUNAROUTE_ENABLE_SESSION_RECORDING=true
export LUNAROUTE_ENABLE_SQLITE_WRITER=true
export LUNAROUTE_SESSIONS_DB_PATH="~/.lunaroute/sessions.db"
export LUNAROUTE_ENABLE_JSONL_WRITER=true
export LUNAROUTE_SESSIONS_DIR="~/.lunaroute/sessions"

# Logging
export LUNAROUTE_LOG_LEVEL=info  # trace, debug, info, warn, error
export LUNAROUTE_LOG_REQUESTS=false

# Server settings
export LUNAROUTE_HOST=127.0.0.1
export LUNAROUTE_PORT=8081

# UI
export LUNAROUTE_UI_ENABLED=true
export LUNAROUTE_UI_PORT=8082
```

### Connection Pooling

Tune HTTP client performance:

```yaml
providers:
  openai:
    http_client:
      timeout_secs: 600              # Request timeout (default: 600)
      connect_timeout_secs: 10       # Connection timeout (default: 10)
      pool_max_idle_per_host: 32     # Pool size (default: 32)
      pool_idle_timeout_secs: 600    # Idle timeout (default: 600)
      tcp_keepalive_secs: 60         # Keepalive (default: 60)
      max_retries: 3                 # Retries (default: 3)
```

Or use environment variables:
```bash
export LUNAROUTE_OPENAI_TIMEOUT_SECS=300
export LUNAROUTE_OPENAI_POOL_MAX_IDLE=64
```

See [Connection Pool Configuration](docs/CONNECTION_POOL_ENV_VARS.md) for details.

### Provider Switch Notifications

LunaRoute can automatically notify users when requests are routed to alternative providers due to rate limits, errors, or circuit breaker events.

**Features:**
- 🔔 Automatic user notifications via LLM response
- 🎛️ Global on/off with per-provider customization
- 🔄 Works with cross-dialect failover (OpenAI ↔ Claude)
- 📝 Template variables for customization
- 🛡️ Idempotent (no duplicate notifications)

**Configuration:**

```yaml
routing:
  provider_switch_notification:
    enabled: true
    default_message: |
      IMPORTANT: Please inform the user that due to temporary service constraints,
      their request is being handled by an alternative AI service provider.
      Continue with their original request.

providers:
  anthropic-backup:
    type: "anthropic"
    # Custom message when THIS provider is used as alternative
    switch_notification_message: |
      Using Claude due to ${reason}. Quality remains the same.
```

**Template Variables:**
- `${original_provider}` - Provider that failed
- `${new_provider}` - Provider being used
- `${reason}` - Generic reason (high demand, service issue, maintenance)
- `${model}` - Model name

See `examples/configs/provider-switch-notification.yaml` for complete example.

---

## 💡 Real-World Use Cases

### Debug Expensive Conversations

**Problem:** Your AI session cost $5 but you don't know why.

**Solution:** Check session stats to see the AI's output was extremely verbose.

### Identify Performance Bottlenecks

**Problem:** Your AI assistant feels slow.

**Solution:** Session statistics reveal Bash commands take 850ms on average - optimize those!

### Team Collaboration

**Problem:** Team shares one proxy but everyone has different API keys.

**Solution:** LunaRoute uses client-provided auth - no shared secrets needed.

### Privacy & Compliance

**Problem:** Need to log sessions but can't store PII.

**Solution:** Enable automatic PII redaction - all sensitive data removed before hitting disk.

---

## 📚 Documentation

- **[Config Examples](examples/configs/README.md)** - Pre-built configs for common scenarios
- **[Server README](crates/lunaroute-server/README.md)** - Complete configuration reference
- **[Claude Code Guide](CLAUDE_CODE_GUIDE.md)** - Claude Code integration
- **[Connection Pool Configuration](docs/CONNECTION_POOL_ENV_VARS.md)** - HTTP client tuning
- **[PII Detection](crates/lunaroute-pii/README.md)** - PII redaction details

### Supported AI Assistants

- ✅ **Claude Code** - Full passthrough support, zero config
- ✅ **OpenAI Codex CLI** - Automatic auth.json integration. Supports both HTTP and WebSocket transports — set `supports_websockets = true` in `~/.codex/config.toml` to use the WS path (lunaroute terminates the WS and drives the HTTP pipeline; session recording, markers, and metrics all work the same).
- ✅ **OpenCode** - Standard OpenAI/Anthropic API compatibility
- ✅ **Custom Clients** - Any tool using OpenAI or Anthropic APIs

---

## 📊 Performance

### Passthrough Mode (Default)
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
│  │  Dual Passthrough Mode (Zero-copy, 100% fidelity)   │  │
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
              ┌───────────────────────┐   ┌─────────────────┐
              │  OpenAI API           │   │  Anthropic API  │
              │  (api.openai.com)     │   │  (api.anth...)  │
              └───────────────────────┘   └─────────────────┘
```

**Key Crates:**
- `lunaroute-core` - Types and traits
- `lunaroute-ingress` - HTTP endpoints (OpenAI, Anthropic)
- `lunaroute-egress` - Provider connectors with connection pooling
- `lunaroute-session` - Recording and search
- `lunaroute-pii` - PII detection/redaction
- `lunaroute-observability` - Metrics and health
- `lunaroute-server` - Production binary

---

## 🤝 Contributing

We welcome contributions! Whether it's:
- Bug reports and fixes
- New PII detectors
- Additional metrics
- Documentation improvements
- Performance optimizations

Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

---

## 📝 License

Licensed under the Apache License, Version 2.0 ([LICENSE](LICENSE)).

---

## 🌟 Why "LunaRoute"?

Like the moon 🌕 guides travelers at night, LunaRoute illuminates your AI interactions. Every request, every token, every decision - visible and trackable.

**Built with ❤️ for developers who want visibility, control, and performance.**

---

<p align="center">
  <strong>Give your AI coding assistant the visibility it deserves.</strong><br>
  <code>lunaroute-server</code>
</p>
