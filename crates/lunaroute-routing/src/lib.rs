//! LunaRoute Routing Engine
//!
//! This crate provides the routing logic for LunaRoute:
//! - Route table and rule matching
//! - Health monitoring
//! - Circuit breakers
//! - Provider router with intelligent failover

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
