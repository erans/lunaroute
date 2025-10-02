//! LunaRoute Routing Engine
//!
//! This crate provides intelligent routing with provider selection strategies:
//!
//! ## Features
//!
//! - **Routing Strategies**: Round-robin and weighted round-robin provider selection
//! - **Route Table**: Rule-based routing with model patterns and listener matching
//! - **Health Monitoring**: Track provider success rates and health states
//! - **Circuit Breakers**: Automatic failover with state machine (Closed/Open/Half-Open)
//! - **Provider Configuration**: Type-based API detection, env var resolution, custom headers
//! - **Thread Safety**: Lock-free concurrent access with DashMap and atomic operations
//!
//! ## Quick Example
//!
//! ```rust,no_run
//! use lunaroute_routing::{
//!     Router, RouteTable, RoutingRule, RuleMatcher,
//!     RoutingStrategy, WeightedProvider,
//!     HealthMonitorConfig, CircuitBreakerConfig,
//! };
//! use std::time::Duration;
//!
//! // Define routing rule with weighted strategy
//! let rule = RoutingRule {
//!     priority: 10,
//!     name: Some("gpt-weighted".to_string()),
//!     matcher: RuleMatcher::model_pattern("^gpt-.*"),
//!     strategy: Some(RoutingStrategy::WeightedRoundRobin {
//!         providers: vec![
//!             WeightedProvider { id: "primary".to_string(), weight: 70 },
//!             WeightedProvider { id: "backup".to_string(), weight: 30 },
//!         ],
//!     }),
//!     primary: None,
//!     fallbacks: vec![],
//! };
//!
//! // Create route table
//! let route_table = RouteTable::with_rules(vec![rule]);
//!
//! // Configure health monitoring and circuit breakers
//! let health_config = HealthMonitorConfig {
//!     healthy_threshold: 0.95,
//!     unhealthy_threshold: 0.50,
//!     failure_window: Duration::from_secs(60),
//!     min_requests: 10,
//! };
//!
//! let circuit_config = CircuitBreakerConfig {
//!     failure_threshold: 5,
//!     success_threshold: 2,
//!     timeout: Duration::from_secs(30),
//! };
//!
//! // Router implements Provider trait and can be used like any provider
//! // let router = Router::new(route_table, providers, health_config, circuit_config);
//! ```
//!
//! See the [README](https://github.com/yourusername/lunaroute/blob/main/crates/lunaroute-routing/README.md) for detailed documentation.

pub mod circuit_breaker;
pub mod health;
pub mod provider_config;
pub mod provider_router;
pub mod router;
pub mod strategy;

// Re-export commonly used types
pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitState, SharedCircuitBreaker};
pub use health::{HealthMetrics, HealthMonitor, HealthMonitorConfig, HealthStatus};
pub use provider_config::{ProviderConfig, ProviderConfigError, ProviderType};
pub use provider_router::Router;
pub use router::{
    ListenerType, RouteTable, RoutingContext, RoutingDecision, RoutingRule, RuleMatcher,
};
pub use strategy::{RoutingStrategy, StrategyError, StrategyState, WeightedProvider};
