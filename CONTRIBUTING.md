# Contributing to LunaRoute

We welcome contributions! Whether it's:
- Bug reports and fixes
- New PII detectors
- Additional metrics
- Documentation improvements
- Performance optimizations

## Development Setup

```bash
# Clone the repository
git clone https://github.com/yourusername/lunaroute.git
cd lunaroute

# Build the project
cargo build

# Run tests
cargo test --workspace --all-features

# Run with logging
RUST_LOG=debug cargo run --package lunaroute-server
```

## Code Quality

Before submitting a PR, ensure:

```bash
# Format code
cargo fmt --all

# Run clippy
cargo clippy --workspace --all-features -- -D warnings

# Run tests
cargo test --workspace --all-features

# Check coverage (optional)
cargo install cargo-tarpaulin
cargo tarpaulin --workspace --all-features
```

## CI/CD

### Continuous Integration

The CI pipeline runs automatically on every push and pull request:

- **Check**: `cargo check` on all features
- **Test**: Full test suite on Ubuntu and macOS (stable + beta)
- **Format**: `cargo fmt --check`
- **Clippy**: Linting with warnings treated as errors
- **Coverage**: Code coverage via tarpaulin (uploaded to Codecov)

View workflow: [.github/workflows/ci.yml](.github/workflows/ci.yml)

### Release Process

LunaRoute uses automated releases via GitHub Actions. To create a new release:

#### Using the Release Script (Recommended)

```bash
# Run the release script
./release.sh

# Follow the prompts:
# 1. Enter version number (e.g., 1.2.3)
# 2. Confirm the release
# 3. Script creates tag and pushes to GitHub
```

The script will:
- ✅ Validate your working directory is clean
- ✅ Check version format (semver)
- ✅ Create an annotated git tag (e.g., `v1.2.3`)
- ✅ Push the tag to GitHub
- ✅ Trigger GitHub Actions release workflow

#### Manual Release

If you prefer to do it manually:

```bash
# Create a semver tag
git tag -a v1.2.3 -m "Release v1.2.3"

# Push the tag to GitHub
git push origin v1.2.3
```

#### Release Workflow

Once the tag is pushed, GitHub Actions automatically:

1. **Builds binaries** for all platforms:
   - Linux (x86_64, ARM64)
   - macOS (x86_64 Intel, ARM64 Apple Silicon)
   - Windows (x86_64, ARM64)

2. **Creates GitHub Release**:
   - Attaches all binaries as release assets
   - Uses the tag message as release notes

3. **Binary naming**:
   - `lunaroute-server-linux-amd64`
   - `lunaroute-server-linux-arm64`
   - `lunaroute-server-darwin-amd64`
   - `lunaroute-server-darwin-arm64`
   - `lunaroute-server-windows-amd64.exe`
   - `lunaroute-server-windows-arm64.exe`

View workflow: [.github/workflows/release.yml](.github/workflows/release.yml)

**Build time:** Typically 10-15 minutes for all platforms.

**Monitor progress:** https://github.com/yourusername/lunaroute/actions

## Pull Request Guidelines

1. **Create a feature branch** from `main` or `develop`
2. **Write descriptive commit messages**
3. **Add tests** for new functionality
4. **Update documentation** if needed
5. **Ensure CI passes** before requesting review

## Project Structure

```
lunaroute/
├── crates/
│   ├── lunaroute-core/          # Core types and traits
│   ├── lunaroute-ingress/       # HTTP endpoints (OpenAI, Anthropic)
│   ├── lunaroute-egress/        # Provider connectors
│   ├── lunaroute-session/       # Recording and search
│   ├── lunaroute-session-sqlite/# SQLite session store
│   ├── lunaroute-pii/           # PII detection/redaction
│   ├── lunaroute-observability/ # Metrics and health
│   └── lunaroute-server/        # Production binary
├── examples/configs/            # Example configurations
├── .github/workflows/           # CI/CD workflows
└── release.sh                   # Release automation script
```

## Questions?

- Open an issue for bugs or feature requests
- Join discussions for questions and ideas

---

**Thank you for contributing to LunaRoute!** 🌕
