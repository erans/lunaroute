# LunaRoute Server

Production-ready LLM API gateway server with intelligent routing, circuit breakers, and session recording.

## Quick Start

### 1. Using Environment Variables Only

```bash
# Run with Anthropic dialect (for Claude Code)
ANTHROPIC_API_KEY=your-key \
LUNAROUTE_DIALECT=anthropic \
cargo run --package lunaroute-server

# Run with OpenAI dialect
OPENAI_API_KEY=your-key \
LUNAROUTE_DIALECT=openai \
cargo run --package lunaroute-server
```

### 2. Using Configuration File

Create a `config.yaml`:

```yaml
host: "127.0.0.1"
port: 3000

# API dialect - which format to accept
api_dialect: "anthropic"  # or "openai"

providers:
  anthropic:
    enabled: true
    api_key: "sk-ant-..."  # Or use ${ANTHROPIC_API_KEY} to read from env

session_recording:
  enabled: false  # No file recording

logging:
  level: "info"
  log_requests: true  # Print requests to stdout
```

Run the server:

```bash
lunaroute-server --config config.yaml

# Or with environment variable
LUNAROUTE_CONFIG=config.yaml lunaroute-server
```

### 3. Using CLI Arguments (Highest Precedence)

```bash
# Override dialect from command line
lunaroute-server --dialect anthropic

# Or combine with config file
lunaroute-server --config config.yaml --dialect openai
```

### Configuration Precedence

Settings are applied in this order (later overrides earlier):

1. **Config file** (`--config config.yaml`)
2. **Environment variables** (`LUNAROUTE_DIALECT=anthropic`)
3. **CLI arguments** (`--dialect anthropic`) ← Highest precedence

Example:
```bash
# Config file says openai, but CLI overrides to anthropic
lunaroute-server --config config.yaml --dialect anthropic
```

## Configuration Reference

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `LUNAROUTE_CONFIG` | Path to config file | none |
| `LUNAROUTE_DIALECT` | API format to accept (anthropic/openai) | anthropic |
| `ANTHROPIC_API_KEY` | Anthropic API key (no prefix) | none |
| `OPENAI_API_KEY` | OpenAI API key (no prefix) | none |
| `LUNAROUTE_HOST` | Server bind address | 127.0.0.1 |
| `LUNAROUTE_PORT` | Server port | 3000 |
| `LUNAROUTE_ENABLE_SESSION_RECORDING` | Enable session recording | false |
| `LUNAROUTE_SESSIONS_DIR` | Session storage directory | ~/.lunaroute/sessions |
| `LUNAROUTE_LOG_REQUESTS` | Log requests to stdout | false |
| `LUNAROUTE_LOG_LEVEL` | Log level (trace/debug/info/warn/error) | info |

**Note**: When `LUNAROUTE_LOG_LEVEL=debug`, HTTP request and response headers to/from providers will be logged, along with detailed timing metrics and session statistics on shutdown.

### Config File Format

Supports both YAML and TOML. Extension determines format (`.yaml`/`.yml` or `.toml`).

See `config.example.yaml` in the repository root for a complete example.

## Using with Claude Code

To proxy Claude Code through LunaRoute:

1. Start the server:

```bash
ANTHROPIC_API_KEY=your-key \
LUNAROUTE_DIALECT=anthropic \
LUNAROUTE_LOG_REQUESTS=true \
cargo run --package lunaroute-server

# For more verbose logging including HTTP headers:
ANTHROPIC_API_KEY=your-key \
LUNAROUTE_DIALECT=anthropic \
LUNAROUTE_LOG_REQUESTS=true \
LUNAROUTE_LOG_LEVEL=debug \
cargo run --package lunaroute-server
```

2. Configure Claude Code to use the proxy:

```bash
export ANTHROPIC_BASE_URL=http://localhost:3000
```

3. Run Claude Code normally - all requests will be logged to stdout

## API Endpoints

Once running, the server exposes:

- `POST /v1/messages` - Anthropic-compatible endpoint
- `POST /v1/chat/completions` - OpenAI-compatible endpoint
- `GET /healthz` - Liveness check
- `GET /readyz` - Readiness check (includes provider status)
- `GET /metrics` - Prometheus metrics

If session recording is enabled:
- `GET /sessions` - List sessions (with filters)
- `GET /sessions/:id` - Get session details

## Features

- ✅ **Zero-downtime routing**: Automatic failover between providers
- ✅ **Passthrough mode**: When dialect matches provider, zero-copy routing with 100% API fidelity (with session recording support)
- ✅ **Circuit breakers**: Prevent cascading failures
- ✅ **Health monitoring**: Track provider availability
- ✅ **Session recording**: Optional request/response capture
- ✅ **Session statistics**: Per-session tracking of tokens (input/output/thinking), requests, tool usage, and proxy overhead
- ✅ **Request logging**: Print all traffic to stdout
- ✅ **Detailed timing metrics**: Pre/post proxy overhead, provider response time (DEBUG level)
- ✅ **Prometheus metrics**: Request rates, latencies, tokens, tool calls, proxy overhead
- ✅ **OpenTelemetry tracing**: Distributed tracing support

### Session Statistics

When running with `LUNAROUTE_LOG_LEVEL=debug`, detailed session statistics are printed on shutdown:

- **Per-session metrics**: Request count, input/output/thinking tokens, tool usage, processing overhead
- **Aggregate statistics**: Total tokens across sessions, average processing time
- **Thinking token tracking**: Separate tracking for Anthropic extended thinking usage
- **Tool call tracking**: Per-session and aggregate statistics on tool usage (e.g., Read, Write, Bash calls)
- **Proxy overhead analysis**: Exact time spent in pre/post processing vs provider response

Configure max sessions tracked in config file:
```yaml
session_stats_max_sessions: 100  # Default: 100
```

## Building

```bash
# Development build
cargo build --package lunaroute-server

# Release build
cargo build --release --package lunaroute-server

# Binary will be at: target/release/lunaroute-server
```

## Production Deployment

1. Build release binary
2. Create config file with production settings
3. Run with systemd or equivalent:

```bash
/path/to/lunaroute-server --config /etc/lunaroute/config.yaml
```

See deployment docs for container and Kubernetes examples.
