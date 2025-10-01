//! Integration tests for observability
//!
//! These tests verify that metrics, health checks, and tracing work
//! correctly when integrated together.

use lunaroute_observability::{health_router, HealthState, Metrics, ProviderStatus, ReadinessChecker};
use std::sync::Arc;

// Mock readiness checker that can be controlled
struct ControllableReadinessChecker {
    ready: std::sync::atomic::AtomicBool,
    providers: Arc<std::sync::Mutex<Vec<ProviderStatus>>>,
}

impl ControllableReadinessChecker {
    fn new(ready: bool) -> Self {
        Self {
            ready: std::sync::atomic::AtomicBool::new(ready),
            providers: Arc::new(std::sync::Mutex::new(vec![])),
        }
    }

    fn set_ready(&self, ready: bool) {
        self.ready.store(ready, std::sync::atomic::Ordering::SeqCst);
    }

    fn add_provider(&self, status: ProviderStatus) {
        self.providers.lock().unwrap().push(status);
    }
}

impl ReadinessChecker for ControllableReadinessChecker {
    fn is_ready(&self) -> bool {
        self.ready.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn get_provider_statuses(&self) -> Vec<ProviderStatus> {
        self.providers.lock().unwrap().clone()
    }
}

#[tokio::test]
async fn test_metrics_recording_workflow() {
    // Create metrics instance
    let metrics = Arc::new(Metrics::new().unwrap());

    // Record some request metrics
    metrics.record_request_success("openai", "gpt-5-mini", "openai", 1.5);
    metrics.record_request_success("openai", "gpt-5-mini", "openai", 2.0);
    metrics.record_request_failure("openai", "gpt-5-mini", "openai", "timeout", 3.0);

    // Record token usage
    metrics.record_tokens("openai", "gpt-5-mini", 100, 50);
    metrics.record_tokens("openai", "gpt-5-mini", 200, 100);

    // Record fallback
    metrics.record_fallback("openai", "anthropic", "circuit_breaker_open");

    // Update circuit breaker state
    use lunaroute_observability::CircuitBreakerState;
    metrics.update_circuit_breaker_state("openai", CircuitBreakerState::Open);

    // Update health status
    use lunaroute_observability::HealthStatus;
    metrics.update_provider_health("openai", HealthStatus::Degraded, 0.67);

    // Gather metrics and verify
    let gathered = metrics.registry().gather();

    // Verify requests total
    let requests_total = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_requests_total")
        .expect("requests_total not found");
    assert_eq!(requests_total.get_metric()[0].get_counter().get_value(), 3.0);

    // Verify tokens
    let tokens_total = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_tokens_total")
        .expect("tokens_total not found");
    assert_eq!(tokens_total.get_metric()[0].get_counter().get_value(), 450.0);

    // Verify fallback
    let fallback = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_fallback_triggered_total")
        .expect("fallback not found");
    assert_eq!(fallback.get_metric()[0].get_counter().get_value(), 1.0);

    // Verify circuit breaker state
    let cb_state = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_circuit_breaker_state")
        .expect("circuit_breaker_state not found");
    assert_eq!(cb_state.get_metric()[0].get_gauge().get_value(), 1.0); // Open = 1

    // Verify health status
    let health_status = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_provider_health_status")
        .expect("health_status not found");
    assert_eq!(health_status.get_metric()[0].get_gauge().get_value(), 2.0); // Degraded = 2

    let success_rate = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_provider_success_rate")
        .expect("success_rate not found");
    assert_eq!(success_rate.get_metric()[0].get_gauge().get_value(), 0.67);
}

#[tokio::test]
async fn test_health_and_metrics_integration() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    // Create metrics and health state
    let metrics = Arc::new(Metrics::new().unwrap());
    let checker = Arc::new(ControllableReadinessChecker::new(true));

    // Add provider statuses
    checker.add_provider(ProviderStatus {
        name: "openai".to_string(),
        status: "healthy".to_string(),
        success_rate: Some(0.98),
    });
    checker.add_provider(ProviderStatus {
        name: "anthropic".to_string(),
        status: "healthy".to_string(),
        success_rate: Some(0.95),
    });

    let health_state = HealthState::with_readiness_checker(metrics.clone(), checker.clone());
    let app = health_router(health_state);

    // Test healthz
    let response = app
        .clone()
        .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Test readyz (should be ready)
    let response = app
        .clone()
        .oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Mark as not ready
    checker.set_ready(false);

    // Test readyz (should be not ready)
    let response = app
        .clone()
        .oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    // Test metrics endpoint
    let response = app
        .oneshot(Request::builder().uri("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/plain; version=0.0.4"
    );
}

#[tokio::test]
async fn test_concurrent_metrics_recording() {
    let metrics = Arc::new(Metrics::new().unwrap());

    // Spawn 50 tasks that concurrently record metrics
    let mut handles = vec![];
    for i in 0..50 {
        let metrics_clone = metrics.clone();
        let handle = tokio::spawn(async move {
            let model = if i % 2 == 0 { "gpt-5-mini" } else { "claude-sonnet-4-5" };
            let provider = if i % 2 == 0 { "openai" } else { "anthropic" };

            metrics_clone.record_request_success("openai", model, provider, 1.0);
            metrics_clone.record_tokens(provider, model, 10, 5);
        });
        handles.push(handle);
    }

    // Wait for all tasks
    for handle in handles {
        handle.await.unwrap();
    }

    // Verify total requests
    let gathered = metrics.registry().gather();
    let requests_total = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_requests_total")
        .expect("requests_total not found");

    let total_requests: f64 = requests_total
        .get_metric()
        .iter()
        .map(|m| m.get_counter().get_value())
        .sum();

    assert_eq!(total_requests, 50.0);
}

#[tokio::test]
async fn test_circuit_breaker_state_transitions_tracked() {
    use lunaroute_observability::CircuitBreakerState;

    let metrics = Arc::new(Metrics::new().unwrap());

    // Simulate circuit breaker transitions
    metrics.update_circuit_breaker_state("openai", CircuitBreakerState::Closed);
    metrics.record_circuit_breaker_transition(
        "openai",
        CircuitBreakerState::Closed,
        CircuitBreakerState::Open,
    );

    metrics.update_circuit_breaker_state("openai", CircuitBreakerState::Open);
    metrics.record_circuit_breaker_transition(
        "openai",
        CircuitBreakerState::Open,
        CircuitBreakerState::HalfOpen,
    );

    metrics.update_circuit_breaker_state("openai", CircuitBreakerState::HalfOpen);
    metrics.record_circuit_breaker_transition(
        "openai",
        CircuitBreakerState::HalfOpen,
        CircuitBreakerState::Closed,
    );

    // Verify transitions were recorded
    let gathered = metrics.registry().gather();
    let transitions = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_circuit_breaker_transitions_total")
        .expect("transitions not found");

    let total_transitions: f64 = transitions
        .get_metric()
        .iter()
        .map(|m| m.get_counter().get_value())
        .sum();

    assert_eq!(total_transitions, 3.0);
}

#[tokio::test]
async fn test_health_status_changes_reflected() {
    use lunaroute_observability::HealthStatus;

    let metrics = Arc::new(Metrics::new().unwrap());

    // Simulate health status changes
    metrics.update_provider_health("openai", HealthStatus::Unknown, 0.0);
    metrics.update_provider_health("openai", HealthStatus::Healthy, 0.95);
    metrics.update_provider_health("openai", HealthStatus::Degraded, 0.75);
    metrics.update_provider_health("openai", HealthStatus::Unhealthy, 0.3);

    // Verify final health status
    let gathered = metrics.registry().gather();
    let health_status = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_provider_health_status")
        .expect("health_status not found");

    // Should be Unhealthy (3)
    assert_eq!(health_status.get_metric()[0].get_gauge().get_value(), 3.0);

    let success_rate = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_provider_success_rate")
        .expect("success_rate not found");

    assert_eq!(success_rate.get_metric()[0].get_gauge().get_value(), 0.3);
}

#[tokio::test]
async fn test_multiple_models_metrics_separation() {
    let metrics = Arc::new(Metrics::new().unwrap());

    // Record metrics for different models
    metrics.record_request_success("openai", "gpt-5-mini", "openai", 1.0);
    metrics.record_request_success("openai", "gpt-5-mini", "openai", 1.2);
    metrics.record_request_success("openai", "gpt-4o", "openai", 2.0);
    metrics.record_request_success("anthropic", "claude-sonnet-4-5", "anthropic", 1.5);

    // Verify metrics are separated by model
    let gathered = metrics.registry().gather();
    let requests_total = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_requests_total")
        .expect("requests_total not found");

    // Should have 3 different label combinations
    assert_eq!(requests_total.get_metric().len(), 3);

    // Verify each model has correct count
    for metric in requests_total.get_metric() {
        let labels = metric.get_label();
        let model_label = labels.iter().find(|l| l.get_name() == "model").unwrap();
        let count = metric.get_counter().get_value();

        match model_label.get_value() {
            "gpt-5-mini" => assert_eq!(count, 2.0),
            "gpt-4o" => assert_eq!(count, 1.0),
            "claude-sonnet-4-5" => assert_eq!(count, 1.0),
            _ => panic!("Unexpected model"),
        }
    }
}

#[tokio::test]
async fn test_latency_histogram_buckets() {
    let metrics = Arc::new(Metrics::new().unwrap());

    // Record various latencies
    metrics.record_request_success("openai", "gpt-5-mini", "openai", 0.01);  // 10ms
    metrics.record_request_success("openai", "gpt-5-mini", "openai", 0.1);   // 100ms
    metrics.record_request_success("openai", "gpt-5-mini", "openai", 1.0);   // 1s
    metrics.record_request_success("openai", "gpt-5-mini", "openai", 5.0);   // 5s

    // Verify histogram recorded all samples
    let gathered = metrics.registry().gather();
    let duration = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_request_duration_seconds")
        .expect("duration not found");

    let histogram = duration.get_metric()[0].get_histogram();
    assert_eq!(histogram.get_sample_count(), 4);

    // Verify sum of observations
    let expected_sum = 0.01 + 0.1 + 1.0 + 5.0;
    assert!((histogram.get_sample_sum() - expected_sum).abs() < 0.001);
}
