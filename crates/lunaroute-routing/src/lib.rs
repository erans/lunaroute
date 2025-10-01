//! LunaRoute Routing Engine
//!
//! This crate provides the routing logic for LunaRoute:
//! - Route table and rule matching
//! - Health monitoring
//! - Circuit breakers

pub mod circuit_breaker;
pub mod health;
pub mod router;

// Re-export commonly used types
pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitState, SharedCircuitBreaker};
pub use health::{HealthMetrics, HealthMonitor, HealthMonitorConfig, HealthStatus};
pub use router::{
    ListenerType, RouteTable, RoutingContext, RoutingDecision, RoutingRule, RuleMatcher,
};
