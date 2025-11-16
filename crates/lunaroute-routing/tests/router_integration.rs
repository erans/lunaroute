//! Integration tests for Router
//!
//! These tests verify Router behavior in more realistic scenarios than unit tests.

use lunaroute_core::{
    Error, Result,
    normalized::{
        Choice, FinishReason, Message, MessageContent, NormalizedRequest, NormalizedResponse,
        NormalizedStreamEvent, Role, Usage,
    },
    provider::Provider,
};
use lunaroute_routing::{
    CircuitBreakerConfig, HealthMonitorConfig, RouteTable, Router, RoutingRule, RuleMatcher,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

// Test provider that can be configured to succeed or fail
#[derive(Clone)]
struct TestProvider {
    id: String,
    should_fail: Arc<AtomicBool>,
    call_count: Arc<AtomicUsize>,
}

impl TestProvider {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            should_fail: Arc::new(AtomicBool::new(false)),
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn set_should_fail(&self, fail: bool) {
        self.should_fail.store(fail, Ordering::SeqCst);
    }

    fn get_call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }

    fn reset_call_count(&self) {
        self.call_count.store(0, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl Provider for TestProvider {
    async fn send(&self, request: NormalizedRequest) -> Result<NormalizedResponse> {
        self.call_count.fetch_add(1, Ordering::SeqCst);

        if self.should_fail.load(Ordering::SeqCst) {
            return Err(Error::Provider(format!("Provider {} failed", self.id)));
        }

        Ok(NormalizedResponse {
            id: format!("{}-response", self.id),
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
        Err(Error::Provider(
            "Streaming not supported in test".to_string(),
        ))
    }

    fn capabilities(&self) -> lunaroute_core::provider::ProviderCapabilities {
        lunaroute_core::provider::ProviderCapabilities {
            supports_streaming: false,
            supports_tools: false,
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
        tool_results: vec![],
        tool_choice: None,
        metadata: HashMap::new(),
    }
}

#[tokio::test]
async fn test_routing_with_fallback_recovery() {
    // Setup: Create router with primary and fallback providers
    let primary = TestProvider::new("primary");
    let fallback = TestProvider::new("fallback");

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("primary".to_string(), Arc::new(primary.clone()));
    providers.insert("fallback".to_string(), Arc::new(fallback.clone()));

    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("test-route".to_string()),
        matcher: RuleMatcher::Always,
        strategy: None,
        primary: Some("primary".to_string()),
        fallbacks: vec!["fallback".to_string()],
    }];

    let route_table = RouteTable::with_rules(rules);
    let router = Router::with_defaults(route_table, providers);

    // Test 1: Primary succeeds
    let request = create_test_request("test-model");
    let response = router.send(request.clone()).await.unwrap();
    assert_eq!(response.id, "primary-response");
    assert_eq!(primary.get_call_count(), 1);
    assert_eq!(fallback.get_call_count(), 0);

    // Test 2: Primary fails, fallback succeeds
    primary.set_should_fail(true);
    let response = router.send(request.clone()).await.unwrap();
    assert_eq!(response.id, "fallback-response");
    assert_eq!(primary.get_call_count(), 2);
    assert_eq!(fallback.get_call_count(), 1);

    // Test 3: Primary recovers
    primary.set_should_fail(false);
    let response = router.send(request.clone()).await.unwrap();
    assert_eq!(response.id, "primary-response");
    assert_eq!(primary.get_call_count(), 3);
    assert_eq!(fallback.get_call_count(), 1);
}

#[tokio::test]
async fn test_circuit_breaker_opens_and_closes() {
    // Setup: Router with aggressive circuit breaker settings
    let provider = TestProvider::new("test-provider");
    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("test-provider".to_string(), Arc::new(provider.clone()));

    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("test-route".to_string()),
        matcher: RuleMatcher::Always,
        strategy: None,
        primary: Some("test-provider".to_string()),
        fallbacks: vec![],
    }];

    let route_table = RouteTable::with_rules(rules);

    // Circuit breaker: 2 failures opens, 1 success closes
    let cb_config = CircuitBreakerConfig {
        failure_threshold: 2,
        success_threshold: 1,
        timeout: Duration::from_millis(100),
    };

    let health_config = HealthMonitorConfig::default();
    let router = Router::new(route_table, providers, health_config, cb_config, None, None);

    let request = create_test_request("test-model");

    // Phase 1: Cause 2 failures to open circuit breaker
    provider.set_should_fail(true);
    let _ = router.send(request.clone()).await;
    let _ = router.send(request.clone()).await;

    // Phase 2: Circuit breaker should be open, request fails immediately
    let result = router.send(request.clone()).await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("All providers failed"));
    assert_eq!(provider.get_call_count(), 2); // No new calls due to open circuit

    // Phase 3: Wait for timeout, then send successful request to close circuit
    tokio::time::sleep(Duration::from_millis(150)).await;
    provider.set_should_fail(false);
    let response = router.send(request.clone()).await.unwrap();
    assert_eq!(response.id, "test-provider-response");

    // Phase 4: Verify circuit is closed and working normally
    let response = router.send(request.clone()).await.unwrap();
    assert_eq!(response.id, "test-provider-response");
}

#[tokio::test]
async fn test_health_monitoring_tracks_failures() {
    // Setup: Router with custom health monitoring config
    let provider = TestProvider::new("test-provider");
    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("test-provider".to_string(), Arc::new(provider.clone()));

    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("test-route".to_string()),
        matcher: RuleMatcher::Always,
        strategy: None,
        primary: Some("test-provider".to_string()),
        fallbacks: vec![],
    }];

    let route_table = RouteTable::with_rules(rules);

    let health_config = HealthMonitorConfig {
        healthy_threshold: 0.8,
        unhealthy_threshold: 0.5,
        failure_window: Duration::from_secs(60),
        min_requests: 5,
    };

    let cb_config = CircuitBreakerConfig::default();
    let router = Router::new(route_table, providers, health_config, cb_config, None, None);

    let request = create_test_request("test-model");

    // Send 10 requests: 8 success, 2 failures
    provider.set_should_fail(false);
    for _ in 0..8 {
        let _ = router.send(request.clone()).await;
    }

    provider.set_should_fail(true);
    for _ in 0..2 {
        let _ = router.send(request.clone()).await;
    }

    // Check health metrics
    let metrics = router.get_health_metrics("test-provider").unwrap();
    assert_eq!(metrics.total_count, 10);
    assert_eq!(metrics.success_count, 8);
    assert_eq!(metrics.failure_count, 2);
    assert!((metrics.success_rate - 0.8).abs() < 0.01);

    // Check health status (should be healthy with 80% success rate)
    use lunaroute_routing::HealthStatus;
    let status = router.get_health_status("test-provider");
    assert_eq!(status, HealthStatus::Healthy);
}

#[tokio::test]
async fn test_multiple_fallbacks_in_sequence() {
    // Setup: Router with 3 providers in fallback chain
    let primary = TestProvider::new("primary");
    let fallback1 = TestProvider::new("fallback1");
    let fallback2 = TestProvider::new("fallback2");

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("primary".to_string(), Arc::new(primary.clone()));
    providers.insert("fallback1".to_string(), Arc::new(fallback1.clone()));
    providers.insert("fallback2".to_string(), Arc::new(fallback2.clone()));

    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("test-route".to_string()),
        matcher: RuleMatcher::Always,
        strategy: None,
        primary: Some("primary".to_string()),
        fallbacks: vec!["fallback1".to_string(), "fallback2".to_string()],
    }];

    let route_table = RouteTable::with_rules(rules);
    let router = Router::with_defaults(route_table, providers);

    let request = create_test_request("test-model");

    // Test 1: Primary and fallback1 fail, fallback2 succeeds
    primary.set_should_fail(true);
    fallback1.set_should_fail(true);
    fallback2.set_should_fail(false);

    let response = router.send(request.clone()).await.unwrap();
    assert_eq!(response.id, "fallback2-response");
    assert_eq!(primary.get_call_count(), 1);
    assert_eq!(fallback1.get_call_count(), 1);
    assert_eq!(fallback2.get_call_count(), 1);

    // Test 2: All providers fail
    fallback2.set_should_fail(true);
    primary.reset_call_count();
    fallback1.reset_call_count();
    fallback2.reset_call_count();

    let result = router.send(request.clone()).await;
    assert!(result.is_err());
    assert_eq!(primary.get_call_count(), 1);
    assert_eq!(fallback1.get_call_count(), 1);
    assert_eq!(fallback2.get_call_count(), 1);
}

#[tokio::test]
async fn test_concurrent_requests() {
    // Setup: Router with single provider
    let provider = TestProvider::new("test-provider");
    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("test-provider".to_string(), Arc::new(provider.clone()));

    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("test-route".to_string()),
        matcher: RuleMatcher::Always,
        strategy: None,
        primary: Some("test-provider".to_string()),
        fallbacks: vec![],
    }];

    let route_table = RouteTable::with_rules(rules);
    let router = Arc::new(Router::with_defaults(route_table, providers));

    // Send 50 concurrent requests
    let mut handles = vec![];
    for i in 0..50 {
        let router_clone = router.clone();
        let handle = tokio::spawn(async move {
            let request = create_test_request(&format!("model-{}", i));
            router_clone.send(request).await
        });
        handles.push(handle);
    }

    // Wait for all requests to complete
    let mut success_count = 0;
    for handle in handles {
        let result = handle.await.unwrap();
        if result.is_ok() {
            success_count += 1;
        }
    }

    // All requests should succeed
    assert_eq!(success_count, 50);
    assert_eq!(provider.get_call_count(), 50);
}

#[tokio::test]
async fn test_model_based_routing() {
    // Setup: Router with different providers for different model patterns
    let openai_provider = TestProvider::new("openai");
    let anthropic_provider = TestProvider::new("anthropic");

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("openai".to_string(), Arc::new(openai_provider.clone()));
    providers.insert(
        "anthropic".to_string(),
        Arc::new(anthropic_provider.clone()),
    );

    let rules = vec![
        RoutingRule {
            priority: 10,
            name: Some("gpt-to-openai".to_string()),
            matcher: RuleMatcher::model_pattern("^gpt-.*"),
            strategy: None,
            primary: Some("openai".to_string()),
            fallbacks: vec!["anthropic".to_string()],
        },
        RoutingRule {
            priority: 10,
            name: Some("claude-to-anthropic".to_string()),
            matcher: RuleMatcher::model_pattern("^claude-.*"),
            strategy: None,
            primary: Some("anthropic".to_string()),
            fallbacks: vec!["openai".to_string()],
        },
    ];

    let route_table = RouteTable::with_rules(rules);
    let router = Router::with_defaults(route_table, providers);

    // Test 1: GPT model routes to OpenAI
    let request = create_test_request("gpt-5-mini");
    let response = router.send(request).await.unwrap();
    assert_eq!(response.id, "openai-response");
    assert_eq!(openai_provider.get_call_count(), 1);
    assert_eq!(anthropic_provider.get_call_count(), 0);

    // Test 2: Claude model routes to Anthropic
    let request = create_test_request("claude-sonnet-4-5");
    let response = router.send(request).await.unwrap();
    assert_eq!(response.id, "anthropic-response");
    assert_eq!(openai_provider.get_call_count(), 1);
    assert_eq!(anthropic_provider.get_call_count(), 1);

    // Test 3: GPT model fails over to Anthropic
    openai_provider.set_should_fail(true);
    let request = create_test_request("gpt-5-mini");
    let response = router.send(request).await.unwrap();
    assert_eq!(response.id, "anthropic-response");
    assert_eq!(openai_provider.get_call_count(), 2);
    assert_eq!(anthropic_provider.get_call_count(), 2);
}
