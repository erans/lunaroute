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

**Phase 1-17**: In progress

See [TODO.md](TODO.md) for the complete implementation roadmap.

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.

## Contributing

Contributions are welcome! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.
