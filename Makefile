.PHONY: help build check test fmt lint clean run install-hooks

# Default target
help:
	@echo "LunaRoute Development Commands:"
	@echo "  make build         - Build all crates in release mode"
	@echo "  make check         - Check all crates for errors"
	@echo "  make test          - Run all tests"
	@echo "  make fmt           - Format code with rustfmt"
	@echo "  make lint          - Run clippy lints"
	@echo "  make clean         - Clean build artifacts"
	@echo "  make run           - Run the lunaroute CLI"
	@echo "  make install-hooks - Install git pre-commit hooks"

# Build all crates
build:
	cargo build --workspace --release

# Check all crates
check:
	cargo check --workspace --all-features

# Run all tests
test:
	cargo test --workspace --all-features

# Format code
fmt:
	cargo fmt --all

# Check formatting
fmt-check:
	cargo fmt --all -- --check

# Run clippy
lint:
	cargo clippy --workspace --all-features -- -D warnings

# Clean build artifacts
clean:
	cargo clean

# Run the CLI
run:
	cargo run --bin lunaroute -- $(ARGS)

# Install pre-commit hooks
install-hooks:
	@echo "Installing pre-commit hooks..."
	@mkdir -p .git/hooks
	@cp scripts/pre-commit .git/hooks/pre-commit
	@chmod +x .git/hooks/pre-commit
	@echo "Pre-commit hooks installed!"

# Development workflow
dev: fmt lint test

# CI workflow
ci: fmt-check lint test
