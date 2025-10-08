//! Metrics collection with Prometheus
//!
//! This module provides Prometheus metrics for LunaRoute:
//! - Request counts (total, success, failure by provider and model)
//! - Latency histograms (p50, p95, p99 for different stages)
//! - Circuit breaker state metrics
//! - Health status metrics
//! - Token usage tracking
//! - Fallback trigger counts
//! - Streaming metrics (TTFT, chunk latencies, chunk counts, memory bounds)

use prometheus::{CounterVec, GaugeVec, Histogram, HistogramOpts, HistogramVec, Opts, Registry};
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

    // Tool call metrics
    /// Tool calls made during requests
    pub tool_calls_total: CounterVec,

    // Processing time metrics
    /// Post-processing duration (after provider response)
    pub post_processing_duration_seconds: Histogram,
    /// Total proxy overhead (pre + post processing)
    pub proxy_overhead_seconds: Histogram,

    // Streaming metrics
    /// Time-to-first-token (TTFT) for streaming requests
    pub streaming_ttft_seconds: HistogramVec,
    /// Chunk latency for streaming requests
    pub streaming_chunk_latency_seconds: HistogramVec,
    /// Total streaming requests
    pub streaming_requests_total: CounterVec,
    /// Chunk count per streaming request
    pub streaming_chunks_total: HistogramVec,
    /// Memory bound warnings (when limits hit)
    pub streaming_memory_bounds_hit: CounterVec,
    /// Total streaming duration (first to last chunk)
    pub streaming_duration_seconds: HistogramVec,

    // Connection pool metrics
    //
    // IMPORTANT LIMITATION: These metrics are defined and tested, but currently NOT populated
    // by production code. The underlying HTTP client (reqwest) does not expose connection pool
    // lifecycle events (connection created/reused/closed/idle count).
    //
    // Metrics status:
    // - pool_config: CAN be populated (static configuration at startup) - TODO: instrument
    // - pool_connections_created_total: CANNOT be populated without reqwest hooks
    // - pool_connections_reused_total: CANNOT be populated without reqwest hooks
    // - pool_connections_idle: CANNOT be populated without reqwest hooks
    // - pool_connection_lifetime_seconds: CANNOT be populated without reqwest hooks
    //
    // Options to populate dynamic metrics:
    // 1. Wait for reqwest to expose pool events (upstream feature request needed)
    // 2. Switch to hyper with custom Connector implementation (significant refactoring)
    // 3. Use a different HTTP client that exposes pool metrics
    // 4. Implement connection wrapper/middleware (complex, may not capture all events)
    //
    // See: https://github.com/seanmonstar/reqwest/issues - no existing issue for pool metrics
    //
    /// Total connections created (indicates pool churn)
    /// NOTE: Currently not populated - reqwest doesn't expose this event
    pub pool_connections_created_total: CounterVec,
    /// Total connections reused from pool
    /// NOTE: Currently not populated - reqwest doesn't expose this event
    pub pool_connections_reused_total: CounterVec,
    /// Current idle connections in pool
    /// NOTE: Currently not populated - reqwest doesn't expose this metric
    pub pool_connections_idle: GaugeVec,
    /// Connection lifetime distribution
    /// NOTE: Currently not populated - reqwest doesn't expose connection close events
    pub pool_connection_lifetime_seconds: HistogramVec,
    /// Pool configuration settings (max_idle_per_host, idle_timeout_secs, etc.)
    /// NOTE: This CAN be populated at startup - instrumentation TODO
    pub pool_config: GaugeVec,
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
            .buckets(vec![0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0]),
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
            Opts::new("lunaroute_tokens_prompt_total", "Total prompt tokens used"),
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

        // Tool call metrics
        let tool_calls_total = CounterVec::new(
            Opts::new(
                "lunaroute_tool_calls_total",
                "Total number of tool calls made",
            ),
            &["provider", "model", "tool_name"],
        )?;

        // Processing time metrics
        let post_processing_duration_seconds = Histogram::with_opts(
            HistogramOpts::new(
                "lunaroute_post_processing_duration_seconds",
                "Post-processing duration in seconds (after provider response)",
            )
            .buckets(vec![0.00001, 0.00005, 0.0001, 0.0005, 0.001, 0.005, 0.01]),
        )?;

        let proxy_overhead_seconds = Histogram::with_opts(
            HistogramOpts::new(
                "lunaroute_proxy_overhead_seconds",
                "Total proxy overhead in seconds (pre + post processing)",
            )
            .buckets(vec![
                0.00001, 0.00005, 0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05,
            ]),
        )?;

        // Streaming metrics
        let streaming_ttft_seconds = HistogramVec::new(
            HistogramOpts::new(
                "lunaroute_streaming_ttft_seconds",
                "Time-to-first-token (TTFT) for streaming requests in seconds",
            )
            .buckets(vec![0.01, 0.05, 0.1, 0.15, 0.2, 0.3, 0.5, 1.0, 2.0, 5.0]),
            &["provider", "model"],
        )?;

        let streaming_chunk_latency_seconds = HistogramVec::new(
            HistogramOpts::new(
                "lunaroute_streaming_chunk_latency_seconds",
                "Individual chunk latency for streaming requests in seconds",
            )
            .buckets(vec![0.01, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0]),
            &["provider", "model"],
        )?;

        let streaming_requests_total = CounterVec::new(
            Opts::new(
                "lunaroute_streaming_requests_total",
                "Total number of streaming requests",
            ),
            &["provider", "model"],
        )?;

        let streaming_chunks_total = HistogramVec::new(
            HistogramOpts::new(
                "lunaroute_streaming_chunks_total",
                "Number of chunks per streaming request",
            )
            .buckets(vec![
                1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 5000.0, 10000.0,
            ]),
            &["provider", "model"],
        )?;

        let streaming_memory_bounds_hit = CounterVec::new(
            Opts::new(
                "lunaroute_streaming_memory_bounds_hit_total",
                "Number of times streaming memory bounds were hit",
            ),
            &["provider", "model", "bound_type"],
        )?;

        let streaming_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "lunaroute_streaming_duration_seconds",
                "Total streaming duration (first to last chunk) in seconds",
            )
            .buckets(vec![0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0]),
            &["provider", "model"],
        )?;

        // Connection pool metrics
        let pool_connections_created_total = CounterVec::new(
            Opts::new(
                "http_pool_connections_created_total",
                "Total number of new connections created (indicates pool churn)",
            ),
            &["provider", "dialect"],
        )?;

        let pool_connections_reused_total = CounterVec::new(
            Opts::new(
                "http_pool_connections_reused_total",
                "Total number of connections reused from pool",
            ),
            &["provider", "dialect"],
        )?;

        let pool_connections_idle = GaugeVec::new(
            Opts::new(
                "http_pool_connections_idle",
                "Current number of idle connections in pool",
            ),
            &["provider", "dialect"],
        )?;

        let pool_connection_lifetime_seconds = HistogramVec::new(
            HistogramOpts::new(
                "http_pool_connection_lifetime_seconds",
                "Connection lifetime distribution in seconds",
            )
            .buckets(vec![1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0]),
            &["provider", "dialect"],
        )?;

        let pool_config = GaugeVec::new(
            Opts::new(
                "http_pool_config",
                "HTTP connection pool configuration settings",
            ),
            &["provider", "setting"],
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
        registry.register(Box::new(tool_calls_total.clone()))?;
        registry.register(Box::new(post_processing_duration_seconds.clone()))?;
        registry.register(Box::new(proxy_overhead_seconds.clone()))?;
        registry.register(Box::new(streaming_ttft_seconds.clone()))?;
        registry.register(Box::new(streaming_chunk_latency_seconds.clone()))?;
        registry.register(Box::new(streaming_requests_total.clone()))?;
        registry.register(Box::new(streaming_chunks_total.clone()))?;
        registry.register(Box::new(streaming_memory_bounds_hit.clone()))?;
        registry.register(Box::new(streaming_duration_seconds.clone()))?;
        registry.register(Box::new(pool_connections_created_total.clone()))?;
        registry.register(Box::new(pool_connections_reused_total.clone()))?;
        registry.register(Box::new(pool_connections_idle.clone()))?;
        registry.register(Box::new(pool_connection_lifetime_seconds.clone()))?;
        registry.register(Box::new(pool_config.clone()))?;

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
            tool_calls_total,
            post_processing_duration_seconds,
            proxy_overhead_seconds,
            streaming_ttft_seconds,
            streaming_chunk_latency_seconds,
            streaming_requests_total,
            streaming_chunks_total,
            streaming_memory_bounds_hit,
            streaming_duration_seconds,
            pool_connections_created_total,
            pool_connections_reused_total,
            pool_connections_idle,
            pool_connection_lifetime_seconds,
            pool_config,
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

    /// Record a tool call
    pub fn record_tool_call(&self, provider: &str, model: &str, tool_name: &str) {
        self.tool_calls_total
            .with_label_values(&[provider, model, tool_name])
            .inc();
    }

    /// Record post-processing duration
    pub fn record_post_processing(&self, duration_secs: f64) {
        self.post_processing_duration_seconds.observe(duration_secs);
    }

    /// Record total proxy overhead (pre + post processing)
    pub fn record_proxy_overhead(&self, duration_secs: f64) {
        self.proxy_overhead_seconds.observe(duration_secs);
    }

    /// Record streaming request completion with comprehensive metrics
    pub fn record_streaming_request(
        &self,
        provider: &str,
        model: &str,
        ttft_secs: f64,
        chunk_count: u32,
        streaming_duration_secs: f64,
    ) {
        self.streaming_requests_total
            .with_label_values(&[provider, model])
            .inc();
        self.streaming_ttft_seconds
            .with_label_values(&[provider, model])
            .observe(ttft_secs);
        self.streaming_chunks_total
            .with_label_values(&[provider, model])
            .observe(chunk_count as f64);
        self.streaming_duration_seconds
            .with_label_values(&[provider, model])
            .observe(streaming_duration_secs);
    }

    /// Record individual chunk latency
    pub fn record_chunk_latency(&self, provider: &str, model: &str, latency_secs: f64) {
        self.streaming_chunk_latency_seconds
            .with_label_values(&[provider, model])
            .observe(latency_secs);
    }

    /// Record memory bound hit (when streaming limits are reached)
    pub fn record_memory_bound_hit(&self, provider: &str, model: &str, bound_type: &str) {
        self.streaming_memory_bounds_hit
            .with_label_values(&[provider, model, bound_type])
            .inc();
    }

    /// Record a new connection creation (indicates pool churn)
    ///
    /// **NOTE**: This method is currently not called by production code.
    /// reqwest doesn't expose connection lifecycle events.
    /// See struct-level documentation for details and options.
    pub fn record_pool_connection_created(&self, provider: &str, dialect: &str) {
        self.pool_connections_created_total
            .with_label_values(&[provider, dialect])
            .inc();
    }

    /// Record a connection reused from the pool
    ///
    /// **NOTE**: This method is currently not called by production code.
    /// reqwest doesn't expose connection lifecycle events.
    /// See struct-level documentation for details and options.
    pub fn record_pool_connection_reused(&self, provider: &str, dialect: &str) {
        self.pool_connections_reused_total
            .with_label_values(&[provider, dialect])
            .inc();
    }

    /// Update the current number of idle connections in the pool
    ///
    /// **NOTE**: This method is currently not called by production code.
    /// reqwest doesn't expose pool state.
    /// See struct-level documentation for details and options.
    pub fn update_pool_connections_idle(&self, provider: &str, dialect: &str, count: usize) {
        self.pool_connections_idle
            .with_label_values(&[provider, dialect])
            .set(count as f64);
    }

    /// Record a connection's lifetime when it's closed
    ///
    /// **NOTE**: This method is currently not called by production code.
    /// reqwest doesn't expose connection close events.
    /// See struct-level documentation for details and options.
    pub fn record_pool_connection_lifetime(
        &self,
        provider: &str,
        dialect: &str,
        lifetime_secs: f64,
    ) {
        self.pool_connection_lifetime_seconds
            .with_label_values(&[provider, dialect])
            .observe(lifetime_secs);
    }

    /// Set pool configuration gauge (called once during initialization)
    ///
    /// **NOTE**: This method is currently not called by production code.
    /// This CAN be implemented - should be called when creating HTTP clients.
    /// See `record_pool_configuration` for the convenience method.
    pub fn set_pool_config(&self, provider: &str, setting: &str, value: f64) {
        self.pool_config
            .with_label_values(&[provider, setting])
            .set(value);
    }

    /// Record HTTP client pool configuration for a provider
    ///
    /// **NOTE**: This method is currently not called by production code.
    /// Unlike the dynamic pool metrics, this CAN be implemented easily.
    /// Should be called in `OpenAIConnector::new()` and `AnthropicConnector::new()`.
    ///
    /// This records static pool configuration settings that don't change after initialization.
    /// The `dialect` parameter should be the provider type (e.g., "openai_compatible", "anthropic").
    ///
    /// # Parameters
    /// - `provider`: Provider name (e.g., "openai", "anthropic", "groq")
    /// - `dialect`: Provider dialect/type (e.g., "openai_compatible", "anthropic")
    /// - `max_idle_per_host`: Maximum idle connections per host
    /// - `idle_timeout_secs`: Idle timeout in seconds
    /// - `timeout_secs`: Request timeout in seconds
    /// - `connect_timeout_secs`: Connection timeout in seconds
    /// - `tcp_keepalive_secs`: TCP keepalive interval in seconds
    ///
    /// # Example
    /// ```ignore
    /// // In OpenAIConnector::new():
    /// metrics.record_pool_configuration(
    ///     "openai",
    ///     "openai_compatible",
    ///     config.client_config.pool_max_idle_per_host,
    ///     config.client_config.pool_idle_timeout_secs,
    ///     config.client_config.timeout_secs,
    ///     config.client_config.connect_timeout_secs,
    ///     config.client_config.tcp_keepalive_secs,
    /// );
    /// ```
    #[allow(clippy::too_many_arguments)]
    pub fn record_pool_configuration(
        &self,
        provider: &str,
        dialect: &str,
        max_idle_per_host: usize,
        idle_timeout_secs: u64,
        timeout_secs: u64,
        connect_timeout_secs: u64,
        tcp_keepalive_secs: u64,
    ) {
        // Record each setting as a separate metric with provider+setting labels
        // This matches the documented format: http_pool_config{provider="openai", setting="max_idle_per_host"}
        let provider_dialect = format!("{}:{}", provider, dialect);
        self.set_pool_config(
            &provider_dialect,
            "max_idle_per_host",
            max_idle_per_host as f64,
        );
        self.set_pool_config(
            &provider_dialect,
            "idle_timeout_secs",
            idle_timeout_secs as f64,
        );
        self.set_pool_config(&provider_dialect, "timeout_secs", timeout_secs as f64);
        self.set_pool_config(
            &provider_dialect,
            "connect_timeout_secs",
            connect_timeout_secs as f64,
        );
        self.set_pool_config(
            &provider_dialect,
            "tcp_keepalive_secs",
            tcp_keepalive_secs as f64,
        );
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
        assert!(!metrics.registry().gather().is_empty());
    }

    #[test]
    fn test_record_request_success() {
        let metrics = Metrics::new().unwrap();
        metrics.record_request_success("openai", "gpt-5-mini", "openai", 1.5);

        let gathered = metrics.registry().gather();
        let total_metric = gathered
            .iter()
            .find(|m| m.name() == "lunaroute_requests_total")
            .expect("requests_total metric not found");

        assert_eq!(
            total_metric.metric[0]
                .counter
                .as_ref()
                .unwrap()
                .value
                .unwrap(),
            1.0
        );
    }

    #[test]
    fn test_record_request_failure() {
        let metrics = Metrics::new().unwrap();
        metrics.record_request_failure("openai", "gpt-5-mini", "openai", "timeout", 2.0);

        let gathered = metrics.registry().gather();
        let failure_metric = gathered
            .iter()
            .find(|m| m.name() == "lunaroute_requests_failure_total")
            .expect("requests_failure_total metric not found");

        assert_eq!(
            failure_metric.metric[0]
                .counter
                .as_ref()
                .unwrap()
                .value
                .unwrap(),
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
            .find(|m| m.name() == "lunaroute_tokens_prompt_total")
            .expect("tokens_prompt_total metric not found");

        assert_eq!(
            prompt_metric.metric[0]
                .counter
                .as_ref()
                .unwrap()
                .value
                .unwrap(),
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
            .find(|m| m.name() == "lunaroute_fallback_triggered_total")
            .expect("fallback_triggered_total metric not found");

        assert_eq!(
            fallback_metric.metric[0]
                .counter
                .as_ref()
                .unwrap()
                .value
                .unwrap(),
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
            .find(|m| m.name() == "lunaroute_circuit_breaker_state")
            .expect("circuit_breaker_state metric not found");

        assert_eq!(
            state_metric.metric[0]
                .gauge
                .as_ref()
                .unwrap()
                .value
                .unwrap(),
            1.0
        );
    }

    #[test]
    fn test_health_status() {
        let metrics = Metrics::new().unwrap();
        metrics.update_provider_health("openai", HealthStatus::Healthy, 0.95);

        let gathered = metrics.registry().gather();
        let health_metric = gathered
            .iter()
            .find(|m| m.name() == "lunaroute_provider_health_status")
            .expect("provider_health_status metric not found");

        assert_eq!(
            health_metric.metric[0]
                .gauge
                .as_ref()
                .unwrap()
                .value
                .unwrap(),
            1.0
        );

        let rate_metric = gathered
            .iter()
            .find(|m| m.name() == "lunaroute_provider_success_rate")
            .expect("provider_success_rate metric not found");

        assert_eq!(
            rate_metric.metric[0].gauge.as_ref().unwrap().value.unwrap(),
            0.95
        );
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
        assert!(!metrics.registry().gather().is_empty());
    }

    #[test]
    fn test_record_tool_call() {
        let metrics = Metrics::new().unwrap();
        metrics.record_tool_call("anthropic", "claude-sonnet-4-5", "Read");
        metrics.record_tool_call("anthropic", "claude-sonnet-4-5", "Read");
        metrics.record_tool_call("anthropic", "claude-sonnet-4-5", "Write");

        let gathered = metrics.registry().gather();
        let tool_metric = gathered
            .iter()
            .find(|m| m.name() == "lunaroute_tool_calls_total")
            .expect("tool_calls_total metric not found");

        // Should have 2 label sets (Read and Write)
        assert_eq!(tool_metric.metric.len(), 2);

        // Find the Read metric and verify count
        let read_metric = tool_metric
            .metric
            .iter()
            .find(|m| {
                m.label
                    .iter()
                    .any(|l| l.name() == "tool_name" && l.value() == "Read")
            })
            .expect("Read tool metric not found");

        assert_eq!(read_metric.counter.as_ref().unwrap().value.unwrap(), 2.0);
    }

    #[test]
    fn test_record_post_processing() {
        let metrics = Metrics::new().unwrap();
        metrics.record_post_processing(0.001);
        metrics.record_post_processing(0.002);

        let gathered = metrics.registry().gather();
        let post_metric = gathered
            .iter()
            .find(|m| m.name() == "lunaroute_post_processing_duration_seconds")
            .expect("post_processing_duration_seconds metric not found");

        let histogram = post_metric.metric[0].histogram.as_ref().unwrap();
        assert_eq!(histogram.sample_count.unwrap(), 2);
    }

    #[test]
    fn test_record_proxy_overhead() {
        let metrics = Metrics::new().unwrap();
        metrics.record_proxy_overhead(0.0001);
        metrics.record_proxy_overhead(0.0005);
        metrics.record_proxy_overhead(0.001);

        let gathered = metrics.registry().gather();
        let overhead_metric = gathered
            .iter()
            .find(|m| m.name() == "lunaroute_proxy_overhead_seconds")
            .expect("proxy_overhead_seconds metric not found");

        let histogram = overhead_metric.metric[0].histogram.as_ref().unwrap();
        assert_eq!(histogram.sample_count.unwrap(), 3);
    }

    #[test]
    fn test_record_pool_connection_created() {
        let metrics = Metrics::new().unwrap();
        metrics.record_pool_connection_created("openai", "openai_compatible");
        metrics.record_pool_connection_created("openai", "openai_compatible");
        metrics.record_pool_connection_created("anthropic", "anthropic");

        let gathered = metrics.registry().gather();
        let created_metric = gathered
            .iter()
            .find(|m| m.name() == "http_pool_connections_created_total")
            .expect("pool_connections_created_total metric not found");

        // Should have 2 label sets (openai:openai_compatible and anthropic:anthropic)
        assert_eq!(created_metric.metric.len(), 2);

        // Find openai metric and verify count
        let openai_metric = created_metric
            .metric
            .iter()
            .find(|m| {
                m.label
                    .iter()
                    .any(|l| l.name() == "provider" && l.value() == "openai")
            })
            .expect("openai pool metric not found");

        assert_eq!(openai_metric.counter.as_ref().unwrap().value.unwrap(), 2.0);
    }

    #[test]
    fn test_record_pool_connection_reused() {
        let metrics = Metrics::new().unwrap();
        metrics.record_pool_connection_reused("openai", "openai_compatible");
        metrics.record_pool_connection_reused("openai", "openai_compatible");
        metrics.record_pool_connection_reused("openai", "openai_compatible");

        let gathered = metrics.registry().gather();
        let reused_metric = gathered
            .iter()
            .find(|m| m.name() == "http_pool_connections_reused_total")
            .expect("pool_connections_reused_total metric not found");

        assert_eq!(
            reused_metric.metric[0]
                .counter
                .as_ref()
                .unwrap()
                .value
                .unwrap(),
            3.0
        );
    }

    #[test]
    fn test_update_pool_connections_idle() {
        let metrics = Metrics::new().unwrap();
        metrics.update_pool_connections_idle("openai", "openai_compatible", 5);
        metrics.update_pool_connections_idle("anthropic", "anthropic", 3);

        let gathered = metrics.registry().gather();
        let idle_metric = gathered
            .iter()
            .find(|m| m.name() == "http_pool_connections_idle")
            .expect("pool_connections_idle metric not found");

        // Should have 2 providers
        assert_eq!(idle_metric.metric.len(), 2);

        // Check openai has 5 idle connections
        let openai_metric = idle_metric
            .metric
            .iter()
            .find(|m| {
                m.label
                    .iter()
                    .any(|l| l.name() == "provider" && l.value() == "openai")
            })
            .expect("openai idle metric not found");

        assert_eq!(openai_metric.gauge.as_ref().unwrap().value.unwrap(), 5.0);
    }

    #[test]
    fn test_record_pool_connection_lifetime() {
        let metrics = Metrics::new().unwrap();
        metrics.record_pool_connection_lifetime("openai", "openai_compatible", 10.5);
        metrics.record_pool_connection_lifetime("openai", "openai_compatible", 30.2);
        metrics.record_pool_connection_lifetime("openai", "openai_compatible", 60.0);

        let gathered = metrics.registry().gather();
        let lifetime_metric = gathered
            .iter()
            .find(|m| m.name() == "http_pool_connection_lifetime_seconds")
            .expect("pool_connection_lifetime_seconds metric not found");

        let histogram = lifetime_metric.metric[0].histogram.as_ref().unwrap();
        assert_eq!(histogram.sample_count.unwrap(), 3);

        // Verify sum of observations
        let expected_sum = 10.5 + 30.2 + 60.0;
        assert!((histogram.sample_sum.unwrap() - expected_sum).abs() < 0.001);
    }

    #[test]
    fn test_set_pool_config() {
        let metrics = Metrics::new().unwrap();
        metrics.set_pool_config("openai:openai_compatible", "max_idle_per_host", 32.0);
        metrics.set_pool_config("openai:openai_compatible", "idle_timeout_secs", 90.0);
        metrics.set_pool_config("anthropic:anthropic", "max_idle_per_host", 16.0);

        let gathered = metrics.registry().gather();
        let config_metric = gathered
            .iter()
            .find(|m| m.name() == "http_pool_config")
            .expect("pool_config metric not found");

        // Should have 3 settings (2 for openai, 1 for anthropic)
        assert_eq!(config_metric.metric.len(), 3);

        // Find max_idle_per_host for openai
        let openai_max_idle = config_metric
            .metric
            .iter()
            .find(|m| {
                m.label
                    .iter()
                    .any(|l| l.name() == "provider" && l.value() == "openai:openai_compatible")
                    && m.label
                        .iter()
                        .any(|l| l.name() == "setting" && l.value() == "max_idle_per_host")
            })
            .expect("openai max_idle_per_host not found");

        assert_eq!(openai_max_idle.gauge.as_ref().unwrap().value.unwrap(), 32.0);
    }

    #[test]
    fn test_record_pool_configuration() {
        let metrics = Metrics::new().unwrap();
        metrics.record_pool_configuration(
            "openai",
            "openai_compatible",
            32,  // max_idle_per_host
            90,  // idle_timeout_secs
            600, // timeout_secs
            10,  // connect_timeout_secs
            60,  // tcp_keepalive_secs
        );

        let gathered = metrics.registry().gather();
        let config_metric = gathered
            .iter()
            .find(|m| m.name() == "http_pool_config")
            .expect("pool_config metric not found");

        // Should have 5 settings (all config params)
        assert_eq!(config_metric.metric.len(), 5);

        // Verify all settings are present
        let settings: Vec<&str> = config_metric
            .metric
            .iter()
            .filter_map(|m| {
                m.label
                    .iter()
                    .find(|l| l.name() == "setting")
                    .map(|l| l.value())
            })
            .collect();

        assert!(settings.contains(&"max_idle_per_host"));
        assert!(settings.contains(&"idle_timeout_secs"));
        assert!(settings.contains(&"timeout_secs"));
        assert!(settings.contains(&"connect_timeout_secs"));
        assert!(settings.contains(&"tcp_keepalive_secs"));

        // Verify timeout_secs value
        let timeout_setting = config_metric
            .metric
            .iter()
            .find(|m| {
                m.label
                    .iter()
                    .any(|l| l.name() == "setting" && l.value() == "timeout_secs")
            })
            .expect("timeout_secs setting not found");

        assert_eq!(
            timeout_setting.gauge.as_ref().unwrap().value.unwrap(),
            600.0
        );
    }
}
