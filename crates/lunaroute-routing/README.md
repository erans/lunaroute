# LunaRoute Routing Engine

Intelligent routing engine with provider selection strategies, health monitoring, and circuit breakers.

## Features

### Routing Strategies

The routing engine supports multiple provider selection strategies for intelligent load distribution:

#### Round-Robin Strategy

Distributes requests equally across all providers in the list:

```yaml
routing:
  rules:
    - name: "gpt-models"
      priority: 10
      matcher:
        model_pattern: "^gpt-.*"
      strategy:
        type: "round-robin"
        providers:
          - "openai-primary"
          - "openai-backup"
          - "openai-fallback"
      fallbacks:
        - "anthropic-emergency"
```

**Characteristics:**
- **Equal distribution**: Each provider gets 1/N of the traffic
- **Lock-free**: Uses atomic counters with DashMap for zero contention
- **Overflow safe**: Counter wraps safely at `usize::MAX` using `wrapping_add`
- **Thread-safe**: AcqRel memory ordering ensures visibility across cores
- **Stateful**: Counter persists across requests for consistent distribution

**Use cases:**
- Load balancing across identical providers
- Round-robin for testing/debugging
- Equal cost distribution

#### Weighted Round-Robin Strategy

Distributes requests based on provider capacity/weight:

```yaml
routing:
  rules:
    - name: "claude-models"
      priority: 10
      matcher:
        model_pattern: "^claude-.*"
      strategy:
        type: "weighted-round-robin"
        providers:
          - id: "anthropic-primary"
            weight: 70  # 70% of traffic
          - id: "anthropic-backup"
            weight: 20  # 20% of traffic
          - id: "anthropic-cheap"
            weight: 10  # 10% of traffic
      fallbacks:
        - "openai-emergency"
```

**Characteristics:**
- **Capacity-based**: Distribute by relative weights (70/20/10 = 70%, 20%, 10%)
- **Overflow protection**: Checked arithmetic prevents weight overflow
- **Exact distribution**: Over 100 requests, you get exactly 70/20/10 split
- **Flexible weighting**: Use any u32 values (e.g., 7/2/1 = same as 70/20/10)

**Use cases:**
- Primary/backup with specific ratios
- Cost optimization (send most to cheaper provider)
- Capacity-based distribution (higher weights for more powerful instances)
- A/B testing with traffic percentages

#### Limits-Alternative Strategy

Automatically switches to alternative providers when rate limits are encountered, with intelligent backoff and automatic recovery:

```yaml
routing:
  rules:
    - name: "gpt-with-rate-limit-protection"
      priority: 100
      matcher:
        model_pattern: "^gpt-.*"
      strategy:
        type: "limits-alternative"
        primary_providers:
          - "openai-primary"
          - "openai-backup"
        alternative_providers:
          - "anthropic-primary"
          - "anthropic-backup"
        exponential_backoff_base_secs: 60  # Optional, default: 60
```

**Characteristics:**
- **Automatic rate limit detection**: Monitors HTTP 429 responses with `retry-after` header parsing
- **Immediate failover**: Switches to alternatives within same request when rate limit detected
- **Cross-dialect support**: Can failover from OpenAI → Anthropic (with automatic translation)
- **Auto-recovery**: Returns to primary providers when rate limits expire
- **Cascading alternatives**: Tries all alternatives sequentially if multiple are rate-limited
- **Smart timing**: Prioritizes `retry-after` header, falls back to exponential backoff (60s, 120s, 240s, etc.)
- **Security hardened**:
  - Bounded memory with MAX_RATE_LIMIT_ENTRIES (1000)
  - Capped retry-after at 48 hours to prevent indefinite blocking
  - Automatic cleanup at 90% capacity
  - Type-safe error handling eliminates string parsing vulnerabilities

**Use cases:**
- Rate limit protection for high-traffic applications
- Cross-provider failover (OpenAI ↔ Anthropic)
- Quota management across multiple accounts
- Resilient production deployments
- Multi-region rate limit handling

**Observability:**
The strategy exposes three Prometheus metrics:
- `lunaroute_rate_limits_total{provider, model}`: Total rate limit events
- `lunaroute_rate_limit_alternatives_used{primary_provider, alternative_provider, model}`: Alternative usage
- `lunaroute_rate_limit_backoff_seconds{provider}`: Backoff durations

**How it works:**
1. **Normal operation**: Requests go to primary providers in order
2. **Rate limit detected**: Provider returns HTTP 429 with `retry-after` header
3. **Immediate switch**: Router tries alternatives within same request (no retry needed)
4. **State tracking**: Provider marked as rate-limited with expiration timestamp
5. **Alternative cascade**: If alternative is also rate-limited, try next alternative
6. **Automatic recovery**: Primary providers become available again after rate limit expires
7. **Backoff fallback**: If no `retry-after` header, uses exponential backoff (60s, 120s, 240s, etc.)

**Example flow:**
```
Request 1: openai-primary (200 OK) ✓
Request 2: openai-primary (200 OK) ✓
Request 3: openai-primary (429 Rate Limit) → anthropic-primary (200 OK) ✓
Request 4: anthropic-primary (200 OK) ✓  [primary still rate-limited]
Request 5: anthropic-primary (200 OK) ✓  [primary still rate-limited]
[Rate limit expires after retry-after seconds]
Request 6: openai-primary (200 OK) ✓  [recovered automatically]
```

### Provider Configuration

Each provider can be configured with type, credentials, and custom settings:

```yaml
providers:
  openai-primary:
    type: "openai"
    api_key: "$OPENAI_API_KEY"  # Environment variable
    base_url: "https://api.openai.com/v1"  # Optional, uses default
    headers:
      X-Custom-Header: "value"
      X-API-Version: "$API_VERSION"  # Env vars work in headers too
    timeout_secs: 60

  anthropic-backup:
    type: "anthropic"
    api_key: "${ANTHROPIC_API_KEY}"  # ${VAR} syntax also supported
    base_url: "https://api.anthropic.com"
```

**Environment Variable Resolution:**
- Supports `$VAR_NAME` and `${VAR_NAME}` syntax
- Resolved at configuration load time
- Works in `api_key` and all header values
- Error if environment variable not found

**Provider Types:**
- `openai`: OpenAI-compatible APIs
- `anthropic`: Anthropic Claude APIs

The `type` field determines:
- API request/response format
- Default base URL if not specified
- Authentication header format

### Health Monitoring

Track provider health based on success rates:

```yaml
routing:
  health_monitor:
    healthy_threshold: 0.95     # 95%+ success = healthy
    unhealthy_threshold: 0.50   # <50% success = unhealthy
    failure_window_secs: 60     # Track last 60 seconds
    min_requests: 10            # Need 10+ requests for status
```

**Health States:**
- **Healthy**: Success rate ≥ 95%
- **Degraded**: Success rate between 50-95%
- **Unhealthy**: Success rate < 50%
- **Unknown**: Not enough requests to determine

### Circuit Breakers

Automatic failover when providers fail:

```yaml
routing:
  circuit_breaker:
    failure_threshold: 5        # Open after 5 failures
    success_threshold: 2        # Close after 2 successes
    timeout_secs: 30           # Try again after 30s
```

**States:**
- **Closed**: Normal operation, requests pass through
- **Open**: Too many failures, reject requests immediately
- **Half-Open**: Testing if provider recovered

### Backwards Compatibility

Old-style configuration still works:

```yaml
routing:
  rules:
    - name: "legacy-rule"
      matcher:
        model_pattern: "^gpt-.*"
      primary: "openai-primary"    # Old style
      fallbacks:
        - "openai-backup"
```

**Migration path:**
1. Keep using `primary` + `fallbacks` (works as before)
2. Add `strategy` field when ready for intelligent routing
3. Can mix old and new styles in same config file

## API Usage

### Creating a Router

```rust
use lunaroute_routing::{
    Router, RouteTable, RoutingRule, RuleMatcher,
    RoutingStrategy, WeightedProvider,
    HealthMonitorConfig, CircuitBreakerConfig,
};
use std::collections::HashMap;
use std::sync::Arc;

// Define routing rules
let rules = vec![
    RoutingRule {
        priority: 10,
        name: Some("gpt-round-robin".to_string()),
        matcher: RuleMatcher::model_pattern("^gpt-.*"),
        strategy: Some(RoutingStrategy::RoundRobin {
            providers: vec![
                "openai-1".to_string(),
                "openai-2".to_string(),
            ],
        }),
        primary: None,
        fallbacks: vec![],
    },
    RoutingRule {
        priority: 10,
        name: Some("claude-weighted".to_string()),
        matcher: RuleMatcher::model_pattern("^claude-.*"),
        strategy: Some(RoutingStrategy::WeightedRoundRobin {
            providers: vec![
                WeightedProvider {
                    id: "anthropic-primary".to_string(),
                    weight: 80,
                },
                WeightedProvider {
                    id: "anthropic-backup".to_string(),
                    weight: 20,
                },
            ],
        }),
        primary: None,
        fallbacks: vec!["openai-fallback".to_string()],
    },
    RoutingRule {
        priority: 15,
        name: Some("gpt-rate-limit-protection".to_string()),
        matcher: RuleMatcher::model_pattern("^gpt-.*"),
        strategy: Some(RoutingStrategy::LimitsAlternative {
            primary_providers: vec![
                "openai-primary".to_string(),
                "openai-backup".to_string(),
            ],
            alternative_providers: vec![
                "anthropic-primary".to_string(),
            ],
            exponential_backoff_base_secs: 60,
        }),
        primary: None,
        fallbacks: vec![],
    },
];

// Create route table
let route_table = RouteTable::with_rules(rules);

// Configure health monitoring
let health_config = HealthMonitorConfig {
    healthy_threshold: 0.95,
    unhealthy_threshold: 0.50,
    failure_window: Duration::from_secs(60),
    min_requests: 10,
};

// Configure circuit breakers
let circuit_config = CircuitBreakerConfig {
    failure_threshold: 5,
    success_threshold: 2,
    timeout: Duration::from_secs(30),
};

// Create router (providers would be Arc<dyn Provider> implementations)
let router = Router::new(
    route_table,
    providers,  // HashMap<String, Arc<dyn Provider>>
    health_config,
    circuit_config,
);

// Router implements Provider trait, use it like any provider
let response = router.send(request).await?;
```

### Strategy Validation

Strategies are validated at creation time:

```rust
use lunaroute_routing::{RoutingStrategy, WeightedProvider};

// Valid strategy
let strategy = RoutingStrategy::WeightedRoundRobin {
    providers: vec![
        WeightedProvider { id: "p1".to_string(), weight: 70 },
        WeightedProvider { id: "p2".to_string(), weight: 30 },
    ],
};
assert!(strategy.validate().is_ok());

// Invalid: empty provider list
let strategy = RoutingStrategy::RoundRobin {
    providers: vec![],
};
assert!(strategy.validate().is_err());

// Invalid: zero total weight
let strategy = RoutingStrategy::WeightedRoundRobin {
    providers: vec![
        WeightedProvider { id: "p1".to_string(), weight: 0 },
    ],
};
assert!(strategy.validate().is_err());

// Invalid: weight overflow
let strategy = RoutingStrategy::WeightedRobin {
    providers: vec![
        WeightedProvider { id: "p1".to_string(), weight: u32::MAX },
        WeightedProvider { id: "p2".to_string(), weight: 1 },
    ],
};
assert!(strategy.validate().is_err());
```

## Implementation Details

### Thread Safety

All routing components are thread-safe and optimized for concurrent access:

- **Lock-free strategy state**: Uses `DashMap` instead of `RwLock<HashMap>` for zero contention
- **Atomic counters**: `AtomicUsize` with AcqRel ordering for cross-core visibility
- **Wrapping arithmetic**: Safe overflow handling with `wrapping_add` via `fetch_update`
- **Circuit breaker state**: Atomic state transitions with `compare_exchange`

### Performance Optimizations

- **Lazy initialization**: Circuit breakers and strategy states created on-demand
- **No allocations in hot path**: Provider selection uses references where possible
- **Cached regex**: Model patterns compiled once and cached with `OnceCell`
- **Lock-free reads**: DashMap allows concurrent reads without blocking
- **Saturating arithmetic**: Weight calculations use `saturating_add` to prevent overflow

### Memory Safety

- **Overflow protection**: All arithmetic uses checked/saturating operations
- **Input validation**: Weights, provider counts validated at config load
- **Error propagation**: No panics in production code paths
- **Resource cleanup**: Proper Drop implementations for all resources

## Testing

The routing crate has comprehensive test coverage:

### Unit Tests (103 tests)
- Round-robin distribution (basic, single provider, wrapping)
- Weighted distribution (equal, unequal, edge cases)
- Strategy validation (empty, zero weight, overflow)
- Provider configuration (type, env vars, serialization)
- Health monitoring (states, transitions, thresholds)
- Circuit breakers (state machine, recovery, overflow)
- Router integration (send, stream, fallbacks)

### Integration Tests (12 tests)
- Model-based routing with fallbacks
- Concurrent request handling (1000+ requests)
- Health monitoring integration
- Circuit breaker integration
- Streaming request routing

### Stress Tests
- **Concurrent access**: 10 threads × 100 requests
- **Many providers**: 100 providers in round-robin
- **Counter wrapping**: Tests at `usize::MAX - 1`
- **Weight overflow**: Tests with `u32::MAX` weights

Run tests:
```bash
# All tests
cargo test --package lunaroute-routing

# With output
cargo test --package lunaroute-routing -- --nocapture

# Specific test
cargo test --package lunaroute-routing test_round_robin_strategy
```

## Configuration Examples

### Example 1: Simple Round-Robin

```yaml
providers:
  openai-1:
    type: "openai"
    api_key: "$OPENAI_KEY_1"
  openai-2:
    type: "openai"
    api_key: "$OPENAI_KEY_2"

routing:
  rules:
    - name: "balance-gpt"
      matcher:
        model_pattern: "^gpt-.*"
      strategy:
        type: "round-robin"
        providers: ["openai-1", "openai-2"]
```

### Example 2: Weighted with Fallback

```yaml
providers:
  primary:
    type: "anthropic"
    api_key: "$ANTHROPIC_KEY"
  backup:
    type: "anthropic"
    api_key: "$ANTHROPIC_BACKUP_KEY"
  emergency:
    type: "openai"
    api_key: "$OPENAI_KEY"

routing:
  rules:
    - name: "claude-with-fallback"
      matcher:
        model_pattern: "^claude-.*"
      strategy:
        type: "weighted-round-robin"
        providers:
          - id: "primary"
            weight: 80
          - id: "backup"
            weight: 20
      fallbacks: ["emergency"]
```

### Example 3: Rate Limit Protection

```yaml
providers:
  openai-primary:
    type: "openai"
    api_key: "$OPENAI_API_KEY"
  openai-backup:
    type: "openai"
    api_key: "$OPENAI_BACKUP_KEY"
  anthropic-primary:
    type: "anthropic"
    api_key: "$ANTHROPIC_KEY"
  anthropic-backup:
    type: "anthropic"
    api_key: "$ANTHROPIC_BACKUP_KEY"

routing:
  rules:
    - name: "gpt-with-rate-limit-protection"
      priority: 100
      matcher:
        model_pattern: "^gpt-.*"
      strategy:
        type: "limits-alternative"
        primary_providers:
          - "openai-primary"
          - "openai-backup"
        alternative_providers:
          - "anthropic-primary"  # Cross-dialect alternative
          - "anthropic-backup"
        exponential_backoff_base_secs: 60
```

### Example 4: Mixed Configuration

```yaml
routing:
  rules:
    # New-style: strategy-based routing
    - name: "gpt-load-balance"
      priority: 20
      matcher:
        model_pattern: "^gpt-.*"
      strategy:
        type: "round-robin"
        providers: ["openai-1", "openai-2"]

    # Old-style: primary + fallbacks (still works!)
    - name: "claude-simple"
      priority: 10
      matcher:
        model_pattern: "^claude-.*"
      primary: "anthropic-primary"
      fallbacks: ["anthropic-backup"]

    # Default catch-all
    - name: "default"
      priority: 1
      matcher:
        always: true
      primary: "openai-default"
```

## Architecture

```
Request
   ↓
RouteTable (find matching rule)
   ↓
RoutingDecision (strategy or primary + fallbacks)
   ↓
Router (implements Provider trait)
   ↓
   ├─ StrategyState (round-robin counter)
   │     ↓
   │  select_provider() → "provider-id"
   │
   ├─ CircuitBreaker (check if provider available)
   │     ↓
   │  allow_request() → true/false
   │
   └─ Provider (delegate to selected provider)
        ↓
     send() / stream()
        ↓
     Response
```

### Component Responsibilities

- **RouteTable**: Match requests to rules based on model patterns, listener type, etc.
- **RoutingStrategy**: Define how to select providers (round-robin, weighted, etc.)
- **StrategyState**: Maintain per-rule state (counters, positions) with thread safety
- **CircuitBreaker**: Protect against failing providers with automatic recovery
- **HealthMonitor**: Track provider success rates for intelligent routing
- **Router**: Coordinate all components and implement Provider trait

## Error Handling

All errors are propagated as `lunaroute_core::Error`:

```rust
// Strategy errors
match strategy.validate() {
    Ok(_) => println!("Strategy is valid"),
    Err(StrategyError::EmptyProviderList) => {
        eprintln!("No providers configured");
    }
    Err(StrategyError::ZeroTotalWeight) => {
        eprintln!("All weights are zero");
    }
    Err(StrategyError::WeightOverflow) => {
        eprintln!("Total weight exceeds u32::MAX");
    }
}

// Router errors
match router.send(request).await {
    Ok(response) => println!("Success: {:?}", response),
    Err(Error::Provider(msg)) => {
        eprintln!("Provider error: {}", msg);
    }
    Err(e) => eprintln!("Other error: {}", e),
}
```

## Observability

The router integrates with the observability system:

- **Metrics**: Strategy selection counts, circuit breaker states, health status
- **Tracing**: Span attributes for selected provider, rule name, strategy type
- **Health endpoints**: Provider health status via `/readyz`

## Production Checklist

Before deploying routing strategies in production:

- ✅ Validate all strategy configurations
- ✅ Set appropriate health monitor thresholds
- ✅ Configure circuit breaker timeouts
- ✅ Test failover behavior under load
- ✅ Monitor strategy distribution metrics
- ✅ Set up alerts for circuit breaker state changes
- ✅ Document provider weights and rationale
- ✅ Test environment variable resolution
- ✅ Verify concurrent access patterns
- ✅ Load test with realistic traffic

## Dependencies

- `dashmap`: Lock-free concurrent HashMap
- `tokio`: Async runtime
- `serde`: Configuration serialization
- `regex`: Model pattern matching
- `thiserror`: Error definitions
- `once_cell`: Regex caching

## License

Licensed under MIT OR Apache-2.0
