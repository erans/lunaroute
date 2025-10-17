# Configuration Examples

This directory contains example configurations for common LunaRoute deployment scenarios.

## Available Configurations

### `connection-pool-example.yaml` ⭐ NEW
HTTP client connection pool configuration for optimal performance.

```bash
export OPENAI_API_KEY=sk-...
lunaroute-server --config examples/configs/connection-pool-example.yaml
```

**Features:**
- **Configurable timeouts**: Request and connection timeouts
- **Pool size tuning**: Max idle connections per host
- **Idle connection management**: Automatic expiry before server timeout
- **TCP keepalive**: Keep long-running requests alive
- **Retry configuration**: Max retries for transient errors
- **Metrics toggle**: Enable/disable pool metrics

All settings also configurable via environment variables (`LUNAROUTE_<PROVIDER>_*`).

See [Connection Pool Configuration](../../docs/CONNECTION_POOL_ENV_VARS.md) for complete guide.

### `routing-strategies.yaml` ⭐ NEW
Intelligent routing with round-robin and weighted load balancing.

```bash
export OPENAI_API_KEY=sk-...
export OPENAI_BACKUP_KEY=sk-...
export ANTHROPIC_API_KEY=sk-ant-...
export ANTHROPIC_BACKUP_KEY=sk-ant-...
lunaroute-server --config examples/configs/routing-strategies.yaml
```

**Features:**
- **Round-robin**: Equal distribution across OpenAI endpoints for GPT models
- **Weighted round-robin**: 80/20 split for Claude models (primary/backup)
- Provider configuration with environment variables
- Health monitoring and circuit breakers
- Automatic fallback chains
- Production-ready observability

### `claude-code-proxy.yaml` ⭐ RECOMMENDED FOR CLAUDE
Optimized for Claude Code CLI with zero-copy passthrough mode.

```bash
# Option 1: Use environment variable
ANTHROPIC_API_KEY=sk-ant-... lunaroute-server --config examples/configs/claude-code-proxy.yaml
export ANTHROPIC_BASE_URL=http://localhost:8081

# Option 2: Let Claude Code provide auth (recommended - no env var needed)
lunaroute-server --config examples/configs/claude-code-proxy.yaml
export ANTHROPIC_BASE_URL=http://localhost:8081
# Claude Code will send its API key in the x-api-key header
```

**Features:**
- Anthropic passthrough mode (no normalization)
- Sub-millisecond overhead (~0.1-0.2ms)
- 100% API fidelity
- Session recording disabled for performance
- Works with client-provided authentication

### `anthropic-proxy.yaml`
Simple Anthropic proxy with debug logging.

```bash
# Option 1: Use environment variable
ANTHROPIC_API_KEY=sk-ant-... lunaroute-server --config examples/configs/anthropic-proxy.yaml

# Option 2: Client provides x-api-key header (no env var needed)
lunaroute-server --config examples/configs/anthropic-proxy.yaml
```

**Features:**
- Debug logging with request details
- No session recording
- Port 8081 (avoid conflicts)
- Supports client-provided authentication headers

### `openai-proxy.yaml`
OpenAI-compatible proxy server with Codex CLI support.

```bash
# Option 1: Use environment variable
OPENAI_API_KEY=sk-... lunaroute-server --config examples/configs/openai-proxy.yaml

# Option 2: Use with OpenAI Codex CLI (automatic auth from ~/.codex/auth.json)
lunaroute-server --config examples/configs/openai-proxy.yaml
export OPENAI_BASE_URL=http://localhost:8081/v1
codex exec "help me debug this function"
```

**Features:**
- Accepts OpenAI format requests
- Routes to OpenAI provider
- **Codex CLI integration** - Automatic auth from `~/.codex/auth.json`
- **Account ID header injection** for proper Codex routing
- Request logging enabled
- Works with all Codex commands (exec, chat, eval)

### `development.yaml`
Full-featured development setup.

```bash
ANTHROPIC_API_KEY=sk-ant-... lunaroute-server --config examples/configs/development.yaml
```

**Features:**
- Debug logging (HTTP headers, timing)
- Session recording to `./sessions`
- Session statistics tracking
- Tool call tracking
- Localhost binding

### `production.yaml`
Production-ready configuration.

```bash
ANTHROPIC_API_KEY=sk-ant-... lunaroute-server --config examples/configs/production.yaml
```

**Features:**
- Binds to all interfaces (0.0.0.0)
- Session recording to `~/.lunaroute/sessions`
- Info-level logging
- 1000 session stats tracking
- Health endpoints ready for Kubernetes

## Configuration Reference

All configurations support:

### Environment Variables
- `ANTHROPIC_API_KEY` - Anthropic API key
- `OPENAI_API_KEY` - OpenAI API key (not needed if using Codex CLI)
- `LUNAROUTE_*` - Override any config value (e.g., `LUNAROUTE_PORT=8080`)

### OpenAI Codex CLI Integration

LunaRoute provides automatic authentication for Codex CLI users:

1. **No API key needed** - Reads from `~/.codex/auth.json` automatically
2. **Account ID injection** - Adds `chatgpt-account-id` header from your profile
3. **Zero configuration** - Just point Codex at the proxy

Example:
```bash
# Start LunaRoute (no OPENAI_API_KEY needed!)
lunaroute-server --config examples/configs/openai-proxy.yaml

# Point Codex to the proxy
export OPENAI_BASE_URL=http://localhost:8081/v1

# Use Codex normally
codex exec "what is 2+2"
codex chat
```

### API Endpoints
- `POST /v1/messages` - Anthropic-compatible endpoint
- `POST /v1/chat/completions` - OpenAI-compatible endpoint
- `GET /healthz` - Liveness check
- `GET /readyz` - Readiness check
- `GET /metrics` - Prometheus metrics

### Configuration Precedence
1. Config file (lowest)
2. Environment variables
3. CLI arguments (highest)

Example:
```bash
# Config says port 8081, but override to 8080
lunaroute-server --config production.yaml --port 8080
```

## Creating Custom Configurations

See `config.example.yaml` in the repository root for a complete configuration template with all available options.

### Minimal Configuration

```yaml
api_dialect: "anthropic"
providers:
  anthropic:
    enabled: true
```

### Full Configuration Template

See `config.example.yaml` for:
- Provider settings (API keys, timeouts, retries)
- Session recording options (compression, directory)
- Logging levels and formats
- Server binding (host, port)
- Session statistics limits
