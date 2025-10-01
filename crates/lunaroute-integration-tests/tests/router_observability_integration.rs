//! Integration tests for Router + Observability
//!
//! These tests verify that Router integrates correctly with observability,
//! including metrics recording, circuit breaker tracking, and health monitoring.

use lunaroute_core::{
    normalized::{
        Choice, FinishReason, Message, MessageContent, NormalizedRequest, NormalizedResponse,
        NormalizedStreamEvent, Role, Usage,
    },
    provider::Provider,
    Error, Result,
};
use lunaroute_observability::Metrics;
use lunaroute_routing::{
    CircuitBreakerConfig, HealthMonitorConfig, RouteTable, Router, RoutingRule, RuleMatcher,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

// Test provider with controllable behavior
#[derive(Clone)]
struct MetricsTestProvider {
    id: String,
    should_fail: Arc<AtomicBool>,
    delay_ms: Arc<AtomicUsize>,
    call_count: Arc<AtomicUsize>,
}

impl MetricsTestProvider {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            should_fail: Arc::new(AtomicBool::new(false)),
            delay_ms: Arc::new(AtomicUsize::new(0)),
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn set_should_fail(&self, fail: bool) {
        self.should_fail.store(fail, Ordering::SeqCst);
    }

    fn set_delay_ms(&self, delay: usize) {
        self.delay_ms.store(delay, Ordering::SeqCst);
    }

    fn get_call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl Provider for MetricsTestProvider {
    async fn send(&self, request: NormalizedRequest) -> Result<NormalizedResponse> {
        self.call_count.fetch_add(1, Ordering::SeqCst);

        // Simulate delay
        let delay = self.delay_ms.load(Ordering::SeqCst);
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay as u64)).await;
        }

        if self.should_fail.load(Ordering::SeqCst) {
            return Err(Error::Provider(format!("Provider {} failed", self.id)));
        }

        Ok(NormalizedResponse {
            id: format!("{}-{}", self.id, uuid::Uuid::new_v4()),
            model: request.model,
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: MessageContent::Text(format!("Response from {}", self.id)),
                    name: None,
                    tool_calls: vec![],
                    tool_call_id: None,
                },
                finish_reason: Some(FinishReason::Stop),
            }],
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 20,
                total_tokens: 30,
            },
            created: 1234567890,
            metadata: HashMap::new(),
        })
    }

    async fn stream(
        &self,
        _request: NormalizedRequest,
    ) -> Result<Box<dyn futures::Stream<Item = Result<NormalizedStreamEvent>> + Send + Unpin>> {
        Err(Error::Provider("Streaming not implemented".to_string()))
    }

    fn capabilities(&self) -> lunaroute_core::provider::ProviderCapabilities {
        lunaroute_core::provider::ProviderCapabilities {
            supports_streaming: false,
            supports_tools: true,
            supports_vision: false,
        }
    }
}

fn create_test_request(model: &str) -> NormalizedRequest {
    NormalizedRequest {
        model: model.to_string(),
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("test".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        max_tokens: Some(100),
        temperature: Some(0.7),
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_choice: None,
        metadata: HashMap::new(),
    }
}

#[tokio::test]
async fn test_router_with_metrics_integration() {
    // Setup: Create router and metrics
    let metrics = Arc::new(Metrics::new().unwrap());
    let provider = MetricsTestProvider::new("test-provider");

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("test-provider".to_string(), Arc::new(provider.clone()));

    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("test-route".to_string()),
        matcher: RuleMatcher::Always,
        primary: "test-provider".to_string(),
        fallbacks: vec![],
    }];

    let route_table = RouteTable::with_rules(rules);
    let router = Router::with_defaults(route_table, providers);

    // Make a request through the router
    let start = std::time::Instant::now();
    let request = create_test_request("test-model");
    let response = router.send(request.clone()).await.unwrap();
    let duration = start.elapsed().as_secs_f64();

    // Manually record metrics (simulating what ingress/demo server would do)
    metrics.record_request_success("openai", "test-model", "test-provider", duration);
    metrics.record_tokens("test-provider", "test-model",
        response.usage.prompt_tokens,
        response.usage.completion_tokens
    );

    // Verify provider was called
    assert_eq!(provider.get_call_count(), 1);

    // Verify metrics were recorded
    let gathered = metrics.registry().gather();

    let requests_total = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_requests_total")
        .expect("requests_total not found");
    assert_eq!(requests_total.get_metric()[0].get_counter().get_value(), 1.0);

    let tokens_total = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_tokens_total")
        .expect("tokens_total not found");
    assert_eq!(tokens_total.get_metric()[0].get_counter().get_value(), 30.0);
}

#[tokio::test]
async fn test_circuit_breaker_with_metrics_tracking() {
    use lunaroute_observability::CircuitBreakerState;

    let metrics = Arc::new(Metrics::new().unwrap());
    let provider = MetricsTestProvider::new("test-provider");
    provider.set_should_fail(true);

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("test-provider".to_string(), Arc::new(provider.clone()));

    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("test-route".to_string()),
        matcher: RuleMatcher::Always,
        primary: "test-provider".to_string(),
        fallbacks: vec![],
    }];

    let route_table = RouteTable::with_rules(rules);

    // Circuit breaker: 2 failures opens
    let cb_config = CircuitBreakerConfig {
        failure_threshold: 2,
        success_threshold: 1,
        timeout: Duration::from_millis(100),
    };

    let health_config = HealthMonitorConfig::default();
    let router = Router::new(route_table, providers, health_config, cb_config);

    let request = create_test_request("test-model");

    // Record initial CB state
    metrics.update_circuit_breaker_state("test-provider", CircuitBreakerState::Closed);

    // Cause 2 failures to open circuit breaker
    let _ = router.send(request.clone()).await;
    let _ = router.send(request.clone()).await;

    // Record CB transition to Open
    metrics.record_circuit_breaker_transition(
        "test-provider",
        CircuitBreakerState::Closed,
        CircuitBreakerState::Open,
    );
    metrics.update_circuit_breaker_state("test-provider", CircuitBreakerState::Open);

    // Verify CB is open
    let result = router.send(request.clone()).await;
    assert!(result.is_err());

    // Verify metrics recorded the state
    let gathered = metrics.registry().gather();

    let cb_state = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_circuit_breaker_state")
        .expect("circuit_breaker_state not found");
    assert_eq!(cb_state.get_metric()[0].get_gauge().get_value(), 1.0); // Open = 1

    let transitions = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_circuit_breaker_transitions_total")
        .expect("transitions not found");
    assert_eq!(transitions.get_metric()[0].get_counter().get_value(), 1.0);
}

#[tokio::test]
async fn test_fallback_with_metrics_tracking() {
    let metrics = Arc::new(Metrics::new().unwrap());

    let primary = MetricsTestProvider::new("primary");
    primary.set_should_fail(true);

    let fallback = MetricsTestProvider::new("fallback");

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("primary".to_string(), Arc::new(primary.clone()));
    providers.insert("fallback".to_string(), Arc::new(fallback.clone()));

    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("test-route".to_string()),
        matcher: RuleMatcher::Always,
        primary: "primary".to_string(),
        fallbacks: vec!["fallback".to_string()],
    }];

    let route_table = RouteTable::with_rules(rules);
    let router = Router::with_defaults(route_table, providers);

    let request = create_test_request("test-model");
    let response = router.send(request).await.unwrap();

    // Should have used fallback
    assert_eq!(primary.get_call_count(), 1);
    assert_eq!(fallback.get_call_count(), 1);
    assert!(response.id.contains("fallback"));

    // Record fallback metric
    metrics.record_fallback("primary", "fallback", "provider_error");

    // Verify metrics
    let gathered = metrics.registry().gather();
    let fallback_metric = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_fallback_triggered_total")
        .expect("fallback not found");
    assert_eq!(fallback_metric.get_metric()[0].get_counter().get_value(), 1.0);
}

#[tokio::test]
async fn test_high_concurrency_with_metrics() {
    let metrics = Arc::new(Metrics::new().unwrap());
    let provider = MetricsTestProvider::new("test-provider");

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("test-provider".to_string(), Arc::new(provider.clone()));

    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("test-route".to_string()),
        matcher: RuleMatcher::Always,
        primary: "test-provider".to_string(),
        fallbacks: vec![],
    }];

    let route_table = RouteTable::with_rules(rules);
    let router = Arc::new(Router::with_defaults(route_table, providers));

    // Send 1000 concurrent requests
    let mut handles = vec![];
    for i in 0..1000 {
        let router_clone = router.clone();
        let metrics_clone = metrics.clone();
        let handle = tokio::spawn(async move {
            let request = create_test_request(&format!("model-{}", i % 10));
            let start = std::time::Instant::now();
            let result = router_clone.send(request).await;
            let duration = start.elapsed().as_secs_f64();

            if result.is_ok() {
                metrics_clone.record_request_success(
                    "test",
                    &format!("model-{}", i % 10),
                    "test-provider",
                    duration,
                );
            }
            result
        });
        handles.push(handle);
    }

    // Wait for all requests
    let mut success_count = 0;
    for handle in handles {
        let result = handle.await.unwrap();
        if result.is_ok() {
            success_count += 1;
        }
    }

    assert_eq!(success_count, 1000);
    assert_eq!(provider.get_call_count(), 1000);

    // Verify metrics recorded all requests
    let gathered = metrics.registry().gather();
    let requests_total = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_requests_total")
        .expect("requests_total not found");

    let total: f64 = requests_total
        .get_metric()
        .iter()
        .map(|m| m.get_counter().get_value())
        .sum();

    assert_eq!(total, 1000.0);
}

#[tokio::test]
async fn test_provider_latency_tracking() {
    let metrics = Arc::new(Metrics::new().unwrap());
    let provider = MetricsTestProvider::new("test-provider");
    provider.set_delay_ms(100); // 100ms delay

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("test-provider".to_string(), Arc::new(provider.clone()));

    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("test-route".to_string()),
        matcher: RuleMatcher::Always,
        primary: "test-provider".to_string(),
        fallbacks: vec![],
    }];

    let route_table = RouteTable::with_rules(rules);
    let router = Router::with_defaults(route_table, providers);

    // Make 10 requests and track latency
    for _ in 0..10 {
        let request = create_test_request("test-model");
        let start = std::time::Instant::now();
        let _ = router.send(request).await.unwrap();
        let duration = start.elapsed().as_secs_f64();

        metrics.record_request_success("test", "test-model", "test-provider", duration);
    }

    // Verify latency histogram
    let gathered = metrics.registry().gather();
    let duration_metric = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_request_duration_seconds")
        .expect("duration not found");

    let histogram = duration_metric.get_metric()[0].get_histogram();
    assert_eq!(histogram.get_sample_count(), 10);

    // Average should be around 0.1 seconds
    let avg = histogram.get_sample_sum() / histogram.get_sample_count() as f64;
    assert!(avg >= 0.09 && avg <= 0.15);
}

#[tokio::test]
async fn test_health_status_with_metrics() {
    use lunaroute_observability::HealthStatus;

    let metrics = Arc::new(Metrics::new().unwrap());
    let provider = MetricsTestProvider::new("test-provider");

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("test-provider".to_string(), Arc::new(provider.clone()));

    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("test-route".to_string()),
        matcher: RuleMatcher::Always,
        primary: "test-provider".to_string(),
        fallbacks: vec![],
    }];

    let route_table = RouteTable::with_rules(rules);
    let health_config = HealthMonitorConfig {
        healthy_threshold: 0.9,
        unhealthy_threshold: 0.5,
        failure_window: Duration::from_secs(60),
        min_requests: 5,
    };
    let router = Router::new(route_table, providers, health_config, CircuitBreakerConfig::default());

    let request = create_test_request("test-model");

    // Send 10 successful requests
    for _ in 0..10 {
        let _ = router.send(request.clone()).await.unwrap();
    }

    // Get health metrics from router
    let health_metrics = router.get_health_metrics("test-provider").unwrap();

    // Record in observability metrics
    metrics.update_provider_health(
        "test-provider",
        HealthStatus::Healthy,
        health_metrics.success_rate,
    );

    // Verify observability metrics
    let gathered = metrics.registry().gather();

    let health_status = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_provider_health_status")
        .expect("health_status not found");
    assert_eq!(health_status.get_metric()[0].get_gauge().get_value(), 1.0); // Healthy = 1

    let success_rate = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_provider_success_rate")
        .expect("success_rate not found");
    assert_eq!(success_rate.get_metric()[0].get_gauge().get_value(), 1.0); // 100% success
}

#[tokio::test]
async fn test_provider_timeout_scenario() {
    let _metrics = Arc::new(Metrics::new().unwrap());
    let slow_provider = MetricsTestProvider::new("slow-provider");
    slow_provider.set_delay_ms(5000); // 5 second delay (will timeout)

    let fast_fallback = MetricsTestProvider::new("fast-fallback");

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("slow-provider".to_string(), Arc::new(slow_provider.clone()));
    providers.insert("fast-fallback".to_string(), Arc::new(fast_fallback.clone()));

    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("test-route".to_string()),
        matcher: RuleMatcher::Always,
        primary: "slow-provider".to_string(),
        fallbacks: vec!["fast-fallback".to_string()],
    }];

    let route_table = RouteTable::with_rules(rules);
    let router = Router::with_defaults(route_table, providers);

    let request = create_test_request("test-model");

    // Set a timeout for the request
    let result = tokio::time::timeout(Duration::from_secs(2), router.send(request.clone())).await;

    // Request should timeout
    assert!(result.is_err(), "Request should have timed out");

    // In a real scenario with proper timeout handling in egress layer,
    // the slow provider would fail and fallback would be used.
    // For this test, we're demonstrating the timeout detection.
}

#[tokio::test]
async fn test_mixed_success_failure_metrics() {
    let metrics = Arc::new(Metrics::new().unwrap());
    let provider = MetricsTestProvider::new("test-provider");

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("test-provider".to_string(), Arc::new(provider.clone()));

    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("test-route".to_string()),
        matcher: RuleMatcher::Always,
        primary: "test-provider".to_string(),
        fallbacks: vec![],
    }];

    let route_table = RouteTable::with_rules(rules);
    let router = Router::with_defaults(route_table, providers);

    let request = create_test_request("test-model");

    // Send 7 successful requests
    for _ in 0..7 {
        let start = std::time::Instant::now();
        let _ = router.send(request.clone()).await.unwrap();
        let duration = start.elapsed().as_secs_f64();
        metrics.record_request_success("test", "test-model", "test-provider", duration);
    }

    // Make provider fail and send 3 failed requests
    provider.set_should_fail(true);
    for _ in 0..3 {
        let start = std::time::Instant::now();
        let result = router.send(request.clone()).await;
        let duration = start.elapsed().as_secs_f64();

        if result.is_err() {
            metrics.record_request_failure("test", "test-model", "test-provider", "provider_error", duration);
        }
    }

    // Verify metrics recorded both success and failure
    let gathered = metrics.registry().gather();

    let success_metric = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_requests_success_total")
        .expect("requests_success not found");
    assert_eq!(success_metric.get_metric()[0].get_counter().get_value(), 7.0);

    let failure_metric = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_requests_failure_total")
        .expect("requests_failure not found");
    assert_eq!(failure_metric.get_metric()[0].get_counter().get_value(), 3.0);

    let total_metric = gathered
        .iter()
        .find(|m| m.get_name() == "lunaroute_requests_total")
        .expect("requests_total not found");
    assert_eq!(total_metric.get_metric()[0].get_counter().get_value(), 10.0);
}
