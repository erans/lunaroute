//! LunaRoute Observability
//!
//! This crate provides observability features:
//! - Metrics collection (Prometheus)
//! - Distributed tracing (OpenTelemetry)
//! - Structured logging
//! - Health endpoints

pub mod health;
pub mod metrics;
pub mod tracing;

// Re-export commonly used types
pub use health::{
    HealthResponse, HealthState, ProviderStatus, ReadinessChecker, ReadinessResponse, health_router,
};
pub use metrics::{CircuitBreakerState, HealthStatus, Metrics};
pub use tracing::{
    RequestSpanAttributes, TracerConfig, init_tracer_provider, record_error, record_success,
    record_token_usage,
};
