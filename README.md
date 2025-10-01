# LunaRoute

**Intelligent LLM API Gateway**

LunaRoute is a high-performance API gateway for Large Language Model providers, written in Rust. It provides intelligent routing, session recording, PII detection, budget management, and unified API translation between different LLM providers.

## Features

- **Unified API Translation**: Seamlessly translate between OpenAI and Anthropic API formats
- **Intelligent Routing**: Route requests based on rules, health, and cost optimization
- **Session Recording**: Capture and replay all LLM interactions
- **PII Detection & Redaction**: Automatically detect and redact sensitive information
- **Budget Management**: Track and enforce spending limits across providers
- **Circuit Breakers**: Automatic failover and retry logic
- **High Performance**: Built in Rust for minimal latency overhead (p95 < 35ms)
- **Observability**: Comprehensive metrics, tracing, and logging

## Architecture

LunaRoute is organized as a Rust workspace with the following crates:

- **lunaroute-core**: Core types and trait definitions
- **lunaroute-ingress**: HTTP ingress adapters (OpenAI, Anthropic)
- **lunaroute-egress**: Provider connectors (OpenAI, Anthropic)
- **lunaroute-routing**: Routing engine with health monitoring and circuit breakers
- **lunaroute-session**: Session recording and replay
- **lunaroute-storage**: Storage abstractions (config, sessions, state)
- **lunaroute-pii**: PII detection and redaction
- **lunaroute-observability**: Metrics, tracing, and health endpoints
- **lunaroute-cli**: Command-line interface (`luna`)

## Quick Start

### Prerequisites

- Rust 1.90+ with Rust 2024 edition support
- cargo

### Development

```bash
# Check all crates
make check

# Run tests
make test

# Format code
make fmt

# Run lints
make lint

# Build release binaries
make build

# Run the CLI
make run ARGS="--help"

# Install git hooks
make install-hooks
```

## Development Status

**Phase 0: Project Setup** ✅ Complete
- Workspace structure
- Development environment
- CI/CD workflows

**Phase 1: Core Types** ✅ Complete
- Normalized data models (requests, responses, streams)
- Core traits (Provider, Storage, PII detection)
- 100% test coverage

**Phase 3: Ingress Layer** ✅ Complete
- OpenAI-compatible HTTP endpoints (/v1/chat/completions)
- Anthropic-compatible HTTP endpoints (/v1/messages)
- Request/response normalization and validation
- Middleware (CORS, security headers, request tracing)
- Comprehensive input validation
- Production-ready security hardening
- 76 tests passing, 100% coverage

**Security Features:**
- Cryptographically secure RNG for trace/span IDs
- Configurable CORS with secure localhost-only default
- Zero panic-prone unwrap() calls
- Comprehensive request validation (temperature, tokens, penalties, etc.)
- Request size limits (1MB per message, 100K max tokens)
- API-specific validation (OpenAI vs Anthropic parameter ranges)

**Phase 4: Normalization Pipeline** ✅ Complete
- OpenAI ⇄ Normalized conversion
  - Request/response mapping with full tool support
  - Tool/function calling conversion
  - Message role and multimodal content handling
  - Proper text extraction from ContentPart arrays
- Anthropic ⇄ Normalized conversion
  - Multimodal content blocks (text, tool_use, tool_result)
  - Tool use mapping with input_schema validation
  - System message and parameter extraction
- Security & validation improvements
  - Tool argument size validation (1MB limit)
  - JSON Schema validation for tool definitions
  - Safe error propagation (no unwrap() in critical paths)
  - Message content size limits (1MB per message)
- Code quality improvements
  - Fixed all clippy warnings (idiomatic Rust patterns)
  - Let-chain syntax for nested conditions
  - RangeInclusive::contains() for range checks
  - ToolChoice enum with PartialEq/Eq for testability
- Comprehensive test coverage (76 tests)
  - Tool schema validation tests (4)
  - Tool argument size validation tests (3)
  - Multimodal content extraction tests (4)
  - Round-trip conversion tests (4)
  - Error path tests (4)
  - Edge case tests (4)
- Stream translation (deferred to Phase 6 with egress)

**Phase 6: Egress Layer** ✅ Complete
- OpenAI connector implementing Provider trait
  - Non-streaming send() with automatic retries
  - Streaming stream() with SSE event parsing
  - Full tool/function calling support
  - Multimodal content handling
- HTTP client with connection pooling
  - Configurable timeouts (60s request, 10s connect)
  - Connection pooling (32 idle connections per host)
  - Exponential backoff retry (100ms → 200ms → 400ms)
  - Smart retry for transient errors (429, 500-504)
- Request/response conversion
  - NormalizedRequest ⇄ OpenAI format
  - Tool and ToolChoice conversion
  - Role mapping (system, user, assistant, tool)
  - Finish reason mapping
- Error handling
  - Comprehensive EgressError enum
  - Provider, HTTP, parse, stream, timeout, rate limit errors
  - Automatic conversion to core Error type
- Security & quality
  - No unwrap() in error paths
  - Connection pooling for efficiency
  - Timeout protection
  - Proper resource cleanup
- 23 tests passing (100% coverage)
  - Tool conversion tests (6)
  - Edge case tests (5)
  - Request/response conversion (8)
  - Config & client tests (6)
  - Error handling tests (3)

**Phase 2: Storage Layer** ✅ Complete
- File-based config store (JSON/YAML/TOML support)
- File-based session store with compression (Zstd/LZ4)
- File-based state store with periodic persistence
- AES-256-GCM encryption utilities
- Argon2id key derivation from passwords
- Cross-platform file locking (Unix/Windows)
- Buffer pool for memory efficiency
- Atomic file writer and rolling file writer
- Session indexing for fast queries
- 88 tests passing, 100% coverage

**Storage Security Features:**
- Memory exhaustion protection (100MB file limit, 500MB state limit)
- Path traversal prevention (session ID validation)
- File watcher leak fix (proper cleanup)
- Atomic writes with parent directory fsync
- Concurrent write protection (advisory file locks)
- Secure key derivation (Argon2id with 64MB, 3 iterations)
- Cryptographically secure RNG for salts and keys

**Completed Phases:** 0, 1, 2, 3, 4, 6 ✅

**Phase 5: Routing Engine** - Not started
**Phase 7-17**: Not started

See [TODO.md](TODO.md) for the complete implementation roadmap.

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.

## Contributing

Contributions are welcome! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.
