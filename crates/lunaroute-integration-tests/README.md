# LunaRoute Integration Tests

This crate contains end-to-end integration tests for LunaRoute.

## Test Types

### 1. Mock API Tests (Default)

Tests in `src/lib.rs` use Wiremock to mock provider APIs. These run by default with `cargo test`.

```bash
cargo test --package lunaroute_integration_tests
```

### 2. Real API Tests (Ignored by Default)

Tests in `tests/real_api_tests.rs` make actual calls to OpenAI and Anthropic APIs. These require API keys and are marked with `#[ignore]` to prevent accidental execution.

**Requirements:**
- API keys must be set in `.env` file in the project root:
  ```
  OPENAI_API_KEY="sk-..."
  ANTHROPIC_API_KEY="sk-ant-..."
  ```

**Running all real API tests:**
```bash
# Run all ignored tests (requires API keys)
cargo test --package lunaroute_integration_tests -- --ignored

# Run with debug output
cargo test --package lunaroute_integration_tests -- --ignored --nocapture
```

**Running specific tests:**
```bash
# Test OpenAI only
cargo test --package lunaroute_integration_tests test_openai_real_api_simple_completion -- --ignored --nocapture

# Test Anthropic only
cargo test --package lunaroute_integration_tests test_anthropic_real_api_simple_completion -- --ignored --nocapture

# Test both providers sequentially
cargo test --package lunaroute_integration_tests test_both_providers_sequential -- --ignored --nocapture
```

## Available Real API Tests

1. **test_openai_real_api_simple_completion** - Basic OpenAI completion with GPT-5 mini
2. **test_anthropic_real_api_simple_completion** - Basic Anthropic completion with Claude Sonnet 4.5
3. **test_openai_with_system_message** - OpenAI with system message
4. **test_anthropic_with_system_message** - Anthropic with system message
5. **test_openai_error_handling_invalid_model** - Error handling for invalid model names
6. **test_both_providers_sequential** - Sequential test of both providers

## Models Tested

- **OpenAI**: `gpt-5-mini` - GPT-5 reasoning model (mini variant for cost efficiency)
- **Anthropic**: `claude-sonnet-4-5` - Claude Sonnet 4.5 (best for complex agents and coding)

## Cost Considerations

Real API tests make actual calls to OpenAI and Anthropic APIs, which incur costs:
- OpenAI gpt-5-mini: Varies by usage
- Anthropic claude-sonnet-4-5: $3/$15 per million tokens (input/output)

Tests use minimal token counts (max_tokens: 10-50) to minimize costs.

## CI/CD Integration

In CI/CD environments:
- Mock tests run automatically on every commit
- Real API tests only run when explicitly triggered with `--ignored` flag
- Set API keys as GitHub Actions secrets or CI environment variables
