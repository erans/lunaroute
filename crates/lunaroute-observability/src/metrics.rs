//! Metrics collection with Prometheus
//!
//! This module provides Prometheus metrics for LunaRoute:
//! - Request counts (total, success, failure by provider and model)
//! - Latency histograms (p50, p95, p99 for different stages)
//! - Circuit breaker state metrics
//! - Health status metrics
//! - Token usage tracking
//! - Fallback trigger counts

use prometheus::{
    CounterVec, GaugeVec, Histogram, HistogramOpts, HistogramVec, Opts, Registry,
};
use std::sync::Arc;

/// Metrics collector for LunaRoute
#[derive(Clone)]
pub struct Metrics {
    /// Prometheus registry
    registry: Arc<Registry>,

    // Request counters
    /// Total requests received
    pub requests_total: CounterVec,
    /// Successful requests
    pub requests_success: CounterVec,
    /// Failed requests
    pub requests_failure: CounterVec,

    // Latency histograms
    /// Total request duration (end-to-end)
    pub request_duration_seconds: HistogramVec,
    /// Ingress processing duration
    pub ingress_duration_seconds: Histogram,
    /// Routing decision duration
    pub routing_duration_seconds: Histogram,
    /// Egress (provider) request duration
    pub egress_duration_seconds: HistogramVec,

    // Circuit breaker metrics
    /// Circuit breaker state (0=closed, 1=open, 2=half-open)
    pub circuit_breaker_state: GaugeVec,
    /// Circuit breaker state changes
    pub circuit_breaker_transitions: CounterVec,

    // Health metrics
    /// Provider health status (0=unknown, 1=healthy, 2=degraded, 3=unhealthy)
    pub provider_health_status: GaugeVec,
    /// Provider success rate (0.0-1.0)
    pub provider_success_rate: GaugeVec,

    // Token metrics
    /// Prompt tokens used
    pub tokens_prompt: CounterVec,
    /// Completion tokens used
    pub tokens_completion: CounterVec,
    /// Total tokens used
    pub tokens_total: CounterVec,

    // Fallback metrics
    /// Fallback trigger count
    pub fallback_triggered: CounterVec,
}

impl Metrics {
    /// Create a new metrics collector
    pub fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();

        // Request counters
        let requests_total = CounterVec::new(
            Opts::new("lunaroute_requests_total", "Total number of requests"),
            &["listener", "model", "provider"],
        )?;

        let requests_success = CounterVec::new(
            Opts::new(
                "lunaroute_requests_success_total",
                "Total number of successful requests",
            ),
            &["listener", "model", "provider"],
        )?;

        let requests_failure = CounterVec::new(
            Opts::new(
                "lunaroute_requests_failure_total",
                "Total number of failed requests",
            ),
            &["listener", "model", "provider", "error_type"],
        )?;

        // Latency histograms
        let request_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "lunaroute_request_duration_seconds",
                "Request duration in seconds",
            )
            .buckets(vec![
                0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
            ]),
            &["listener", "model", "provider"],
        )?;

        let ingress_duration_seconds = Histogram::with_opts(
            HistogramOpts::new(
                "lunaroute_ingress_duration_seconds",
                "Ingress processing duration in seconds",
            )
            .buckets(vec![0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1]),
        )?;

        let routing_duration_seconds = Histogram::with_opts(
            HistogramOpts::new(
                "lunaroute_routing_duration_seconds",
                "Routing decision duration in seconds",
            )
            .buckets(vec![0.0001, 0.0005, 0.001, 0.0025, 0.005, 0.01]),
        )?;

        let egress_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "lunaroute_egress_duration_seconds",
                "Egress provider request duration in seconds",
            )
            .buckets(vec![
                0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0,
            ]),
            &["provider", "model"],
        )?;

        // Circuit breaker metrics
        let circuit_breaker_state = GaugeVec::new(
            Opts::new(
                "lunaroute_circuit_breaker_state",
                "Circuit breaker state (0=closed, 1=open, 2=half-open)",
            ),
            &["provider"],
        )?;

        let circuit_breaker_transitions = CounterVec::new(
            Opts::new(
                "lunaroute_circuit_breaker_transitions_total",
                "Circuit breaker state transitions",
            ),
            &["provider", "from_state", "to_state"],
        )?;

        // Health metrics
        let provider_health_status = GaugeVec::new(
            Opts::new(
                "lunaroute_provider_health_status",
                "Provider health status (0=unknown, 1=healthy, 2=degraded, 3=unhealthy)",
            ),
            &["provider"],
        )?;

        let provider_success_rate = GaugeVec::new(
            Opts::new(
                "lunaroute_provider_success_rate",
                "Provider success rate (0.0-1.0)",
            ),
            &["provider"],
        )?;

        // Token metrics
        let tokens_prompt = CounterVec::new(
            Opts::new(
                "lunaroute_tokens_prompt_total",
                "Total prompt tokens used",
            ),
            &["provider", "model"],
        )?;

        let tokens_completion = CounterVec::new(
            Opts::new(
                "lunaroute_tokens_completion_total",
                "Total completion tokens used",
            ),
            &["provider", "model"],
        )?;

        let tokens_total = CounterVec::new(
            Opts::new("lunaroute_tokens_total", "Total tokens used"),
            &["provider", "model"],
        )?;

        // Fallback metrics
        let fallback_triggered = CounterVec::new(
            Opts::new(
                "lunaroute_fallback_triggered_total",
                "Number of times fallback was triggered",
            ),
            &["from_provider", "to_provider", "reason"],
        )?;

        // Register all metrics
        registry.register(Box::new(requests_total.clone()))?;
        registry.register(Box::new(requests_success.clone()))?;
        registry.register(Box::new(requests_failure.clone()))?;
        registry.register(Box::new(request_duration_seconds.clone()))?;
        registry.register(Box::new(ingress_duration_seconds.clone()))?;
        registry.register(Box::new(routing_duration_seconds.clone()))?;
        registry.register(Box::new(egress_duration_seconds.clone()))?;
        registry.register(Box::new(circuit_breaker_state.clone()))?;
        registry.register(Box::new(circuit_breaker_transitions.clone()))?;
        registry.register(Box::new(provider_health_status.clone()))?;
        registry.register(Box::new(provider_success_rate.clone()))?;
        registry.register(Box::new(tokens_prompt.clone()))?;
        registry.register(Box::new(tokens_completion.clone()))?;
        registry.register(Box::new(tokens_total.clone()))?;
        registry.register(Box::new(fallback_triggered.clone()))?;

        Ok(Self {
            registry: Arc::new(registry),
            requests_total,
            requests_success,
            requests_failure,
            request_duration_seconds,
            ingress_duration_seconds,
            routing_duration_seconds,
            egress_duration_seconds,
            circuit_breaker_state,
            circuit_breaker_transitions,
            provider_health_status,
            provider_success_rate,
            tokens_prompt,
            tokens_completion,
            tokens_total,
            fallback_triggered,
        })
    }

    /// Get the Prometheus registry for exporting metrics
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Record a successful request
    pub fn record_request_success(
        &self,
        listener: &str,
        model: &str,
        provider: &str,
        duration_secs: f64,
    ) {
        self.requests_total
            .with_label_values(&[listener, model, provider])
            .inc();
        self.requests_success
            .with_label_values(&[listener, model, provider])
            .inc();
        self.request_duration_seconds
            .with_label_values(&[listener, model, provider])
            .observe(duration_secs);
    }

    /// Record a failed request
    pub fn record_request_failure(
        &self,
        listener: &str,
        model: &str,
        provider: &str,
        error_type: &str,
        duration_secs: f64,
    ) {
        self.requests_total
            .with_label_values(&[listener, model, provider])
            .inc();
        self.requests_failure
            .with_label_values(&[listener, model, provider, error_type])
            .inc();
        self.request_duration_seconds
            .with_label_values(&[listener, model, provider])
            .observe(duration_secs);
    }

    /// Record token usage
    pub fn record_tokens(
        &self,
        provider: &str,
        model: &str,
        prompt_tokens: u32,
        completion_tokens: u32,
    ) {
        self.tokens_prompt
            .with_label_values(&[provider, model])
            .inc_by(prompt_tokens as f64);
        self.tokens_completion
            .with_label_values(&[provider, model])
            .inc_by(completion_tokens as f64);
        self.tokens_total
            .with_label_values(&[provider, model])
            .inc_by((prompt_tokens + completion_tokens) as f64);
    }

    /// Record fallback trigger
    pub fn record_fallback(&self, from_provider: &str, to_provider: &str, reason: &str) {
        self.fallback_triggered
            .with_label_values(&[from_provider, to_provider, reason])
            .inc();
    }

    /// Update circuit breaker state
    pub fn update_circuit_breaker_state(&self, provider: &str, state: CircuitBreakerState) {
        self.circuit_breaker_state
            .with_label_values(&[provider])
            .set(state as i64 as f64);
    }

    /// Record circuit breaker transition
    pub fn record_circuit_breaker_transition(
        &self,
        provider: &str,
        from: CircuitBreakerState,
        to: CircuitBreakerState,
    ) {
        self.circuit_breaker_transitions
            .with_label_values(&[provider, from.as_str(), to.as_str()])
            .inc();
    }

    /// Update provider health status
    pub fn update_provider_health(&self, provider: &str, status: HealthStatus, success_rate: f64) {
        self.provider_health_status
            .with_label_values(&[provider])
            .set(status as i64 as f64);
        self.provider_success_rate
            .with_label_values(&[provider])
            .set(success_rate);
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new().expect("Failed to create metrics")
    }
}

/// Circuit breaker state for metrics
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CircuitBreakerState {
    Closed = 0,
    Open = 1,
    HalfOpen = 2,
}

impl CircuitBreakerState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open => "open",
            Self::HalfOpen => "half_open",
        }
    }
}

/// Provider health status for metrics
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HealthStatus {
    Unknown = 0,
    Healthy = 1,
    Degraded = 2,
    Unhealthy = 3,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_creation() {
        let metrics = Metrics::new().unwrap();
        assert!(metrics.registry().gather().len() > 0);
    }

    #[test]
    fn test_record_request_success() {
        let metrics = Metrics::new().unwrap();
        metrics.record_request_success("openai", "gpt-5-mini", "openai", 1.5);

        let gathered = metrics.registry().gather();
        let total_metric = gathered
            .iter()
            .find(|m| m.get_name() == "lunaroute_requests_total")
            .expect("requests_total metric not found");

        assert_eq!(total_metric.get_metric()[0].get_counter().get_value(), 1.0);
    }

    #[test]
    fn test_record_request_failure() {
        let metrics = Metrics::new().unwrap();
        metrics.record_request_failure("openai", "gpt-5-mini", "openai", "timeout", 2.0);

        let gathered = metrics.registry().gather();
        let failure_metric = gathered
            .iter()
            .find(|m| m.get_name() == "lunaroute_requests_failure_total")
            .expect("requests_failure_total metric not found");

        assert_eq!(
            failure_metric.get_metric()[0].get_counter().get_value(),
            1.0
        );
    }

    #[test]
    fn test_record_tokens() {
        let metrics = Metrics::new().unwrap();
        metrics.record_tokens("openai", "gpt-5-mini", 100, 50);

        let gathered = metrics.registry().gather();
        let prompt_metric = gathered
            .iter()
            .find(|m| m.get_name() == "lunaroute_tokens_prompt_total")
            .expect("tokens_prompt_total metric not found");

        assert_eq!(
            prompt_metric.get_metric()[0].get_counter().get_value(),
            100.0
        );
    }

    #[test]
    fn test_record_fallback() {
        let metrics = Metrics::new().unwrap();
        metrics.record_fallback("openai", "anthropic", "circuit_breaker_open");

        let gathered = metrics.registry().gather();
        let fallback_metric = gathered
            .iter()
            .find(|m| m.get_name() == "lunaroute_fallback_triggered_total")
            .expect("fallback_triggered_total metric not found");

        assert_eq!(
            fallback_metric.get_metric()[0].get_counter().get_value(),
            1.0
        );
    }

    #[test]
    fn test_circuit_breaker_state() {
        let metrics = Metrics::new().unwrap();
        metrics.update_circuit_breaker_state("openai", CircuitBreakerState::Open);

        let gathered = metrics.registry().gather();
        let state_metric = gathered
            .iter()
            .find(|m| m.get_name() == "lunaroute_circuit_breaker_state")
            .expect("circuit_breaker_state metric not found");

        assert_eq!(state_metric.get_metric()[0].get_gauge().get_value(), 1.0);
    }

    #[test]
    fn test_health_status() {
        let metrics = Metrics::new().unwrap();
        metrics.update_provider_health("openai", HealthStatus::Healthy, 0.95);

        let gathered = metrics.registry().gather();
        let health_metric = gathered
            .iter()
            .find(|m| m.get_name() == "lunaroute_provider_health_status")
            .expect("provider_health_status metric not found");

        assert_eq!(health_metric.get_metric()[0].get_gauge().get_value(), 1.0);

        let rate_metric = gathered
            .iter()
            .find(|m| m.get_name() == "lunaroute_provider_success_rate")
            .expect("provider_success_rate metric not found");

        assert_eq!(rate_metric.get_metric()[0].get_gauge().get_value(), 0.95);
    }

    #[test]
    fn test_circuit_breaker_state_as_str() {
        assert_eq!(CircuitBreakerState::Closed.as_str(), "closed");
        assert_eq!(CircuitBreakerState::Open.as_str(), "open");
        assert_eq!(CircuitBreakerState::HalfOpen.as_str(), "half_open");
    }

    #[test]
    fn test_metrics_default() {
        let metrics = Metrics::default();
        assert!(metrics.registry().gather().len() > 0);
    }
}
