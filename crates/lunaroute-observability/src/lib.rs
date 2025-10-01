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
