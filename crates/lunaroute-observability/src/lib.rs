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
    health_router, HealthResponse, HealthState, ProviderStatus, ReadinessChecker,
    ReadinessResponse,
};
pub use metrics::{CircuitBreakerState, HealthStatus, Metrics};
pub use tracing::{
    init_tracer_provider, record_error, record_success, record_token_usage, RequestSpanAttributes,
    TracerConfig,
};
