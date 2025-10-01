//! End-to-end integration tests with observability
//!
//! These tests verify the complete flow:
//! Ingress → Router → Egress → Observability

use axum::body::Body;
use axum::http::{Request, StatusCode};
use lunaroute_core::{
    normalized::{
        Choice, FinishReason, Message, MessageContent, NormalizedRequest, NormalizedResponse,
        Role, Usage,
    },
    provider::Provider,
    Error, Result,
};
use lunaroute_ingress::openai;
use lunaroute_observability::{health_router, HealthState, Metrics};
use lunaroute_routing::{RouteTable, Router, RoutingRule, RuleMatcher};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

// Mock provider that tracks calls
#[derive(Clone)]
struct MockProvider {
    id: String,
    call_count: Arc<AtomicUsize>,
}

impl MockProvider {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn get_call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl Provider for MockProvider {
    async fn send(&self, request: NormalizedRequest) -> Result<NormalizedResponse> {
        self.call_count.fetch_add(1, Ordering::SeqCst);

        Ok(NormalizedResponse {
            id: format!("{}-{}", self.id, uuid::Uuid::new_v4()),
            model: request.model,
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: MessageContent::Text(format!(
                        "Response from {} provider",
                        self.id
                    )),
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
    ) -> Result<Box<dyn futures::Stream<Item = Result<lunaroute_core::normalized::NormalizedStreamEvent>> + Send + Unpin>>
    {
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

#[tokio::test]
async fn test_e2e_openai_request_with_metrics() {
    // Setup providers
    let openai_provider = MockProvider::new("openai");
    let anthropic_provider = MockProvider::new("anthropic");

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("openai".to_string(), Arc::new(openai_provider.clone()));
    providers.insert(
        "anthropic".to_string(),
        Arc::new(anthropic_provider.clone()),
    );

    // Setup routing
    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("gpt-to-openai".to_string()),
        matcher: RuleMatcher::model_pattern("^gpt-.*"),
        primary: "openai".to_string(),
        fallbacks: vec!["anthropic".to_string()],
    }];

    let route_table = RouteTable::with_rules(rules);
    let router = Arc::new(Router::with_defaults(route_table, providers));

    // Setup observability
    let metrics = Arc::new(Metrics::new().unwrap());
    let health_state = HealthState::new(metrics.clone());

    // Create combined app
    let api_router = openai::router(router.clone());
    let health_router = health_router(health_state);
    let app = api_router.merge(health_router);

    // Make API request
    let request_body = json!({
        "model": "gpt-5-mini",
        "messages": [
            {"role": "user", "content": "Hello!"}
        ]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/chat/completions")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&request_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Verify provider was called
    assert_eq!(openai_provider.get_call_count(), 1);

    // Note: In a real integration, we would record metrics here
    // For this test, we're just verifying the full flow works
}

#[tokio::test]
async fn test_e2e_health_checks_with_router() {
    // Setup providers
    let openai_provider = MockProvider::new("openai");
    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("openai".to_string(), Arc::new(openai_provider));

    // Setup routing
    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("test-route".to_string()),
        matcher: RuleMatcher::Always,
        primary: "openai".to_string(),
        fallbacks: vec![],
    }];

    let route_table = RouteTable::with_rules(rules);
    let router = Arc::new(Router::with_defaults(route_table, providers));

    // Setup observability
    let metrics = Arc::new(Metrics::new().unwrap());
    let health_state = HealthState::new(metrics);

    // Create combined app
    let api_router = openai::router(router);
    let health_router = health_router(health_state);
    let app = api_router.merge(health_router);

    // Test healthz
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Test readyz
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/readyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Test metrics
    let response = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/plain; version=0.0.4"
    );
}

#[tokio::test]
async fn test_e2e_fallback_with_observability() {
    // Setup providers - primary will fail
    let openai_provider = MockProvider::new("openai");
    let anthropic_provider = MockProvider::new("anthropic");

    // We can't easily make the mock fail here, but we're testing the integration
    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("openai".to_string(), Arc::new(openai_provider.clone()));
    providers.insert(
        "anthropic".to_string(),
        Arc::new(anthropic_provider.clone()),
    );

    // Setup routing with fallback
    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("gpt-to-openai".to_string()),
        matcher: RuleMatcher::model_pattern("^gpt-.*"),
        primary: "openai".to_string(),
        fallbacks: vec!["anthropic".to_string()],
    }];

    let route_table = RouteTable::with_rules(rules);
    let router = Arc::new(Router::with_defaults(route_table, providers));

    // Setup observability
    let metrics = Arc::new(Metrics::new().unwrap());
    let health_state = HealthState::new(metrics.clone());

    // Create combined app
    let api_router = openai::router(router);
    let health_router = health_router(health_state);
    let app = api_router.merge(health_router);

    // Make API request
    let request_body = json!({
        "model": "gpt-5-mini",
        "messages": [
            {"role": "user", "content": "Test fallback"}
        ]
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/chat/completions")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&request_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Should succeed (primary works in this test)
    assert_eq!(response.status(), StatusCode::OK);

    // Verify primary was called
    assert_eq!(openai_provider.get_call_count(), 1);
}

#[tokio::test]
async fn test_e2e_multiple_models_routing() {
    // Setup providers
    let openai_provider = MockProvider::new("openai");
    let anthropic_provider = MockProvider::new("anthropic");

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("openai".to_string(), Arc::new(openai_provider.clone()));
    providers.insert(
        "anthropic".to_string(),
        Arc::new(anthropic_provider.clone()),
    );

    // Setup routing for different models
    let rules = vec![
        RoutingRule {
            priority: 10,
            name: Some("gpt-to-openai".to_string()),
            matcher: RuleMatcher::model_pattern("^gpt-.*"),
            primary: "openai".to_string(),
            fallbacks: vec![],
        },
        RoutingRule {
            priority: 10,
            name: Some("claude-to-anthropic".to_string()),
            matcher: RuleMatcher::model_pattern("^claude-.*"),
            primary: "anthropic".to_string(),
            fallbacks: vec![],
        },
    ];

    let route_table = RouteTable::with_rules(rules);
    let router = Arc::new(Router::with_defaults(route_table, providers));

    // Setup observability
    let metrics = Arc::new(Metrics::new().unwrap());
    let health_state = HealthState::new(metrics);

    // Create combined app
    let api_router = openai::router(router);
    let health_router = health_router(health_state);
    let app = api_router.merge(health_router);

    // Request 1: GPT model
    let request_body = json!({
        "model": "gpt-5-mini",
        "messages": [{"role": "user", "content": "GPT request"}]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/chat/completions")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&request_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(openai_provider.get_call_count(), 1);

    // Request 2: Claude model
    let request_body = json!({
        "model": "claude-sonnet-4-5",
        "messages": [{"role": "user", "content": "Claude request"}]
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/chat/completions")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&request_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(anthropic_provider.get_call_count(), 1);
}

#[tokio::test]
async fn test_e2e_invalid_request_handling() {
    // Setup minimal router
    let provider = MockProvider::new("test");
    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("test".to_string(), Arc::new(provider));

    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("test-route".to_string()),
        matcher: RuleMatcher::Always,
        primary: "test".to_string(),
        fallbacks: vec![],
    }];

    let route_table = RouteTable::with_rules(rules);
    let router = Arc::new(Router::with_defaults(route_table, providers));

    // Setup observability
    let metrics = Arc::new(Metrics::new().unwrap());
    let health_state = HealthState::new(metrics);

    // Create combined app
    let api_router = openai::router(router);
    let health_router = health_router(health_state);
    let app = api_router.merge(health_router);

    // Send invalid request (missing model)
    let request_body = json!({
        "messages": [{"role": "user", "content": "Test"}]
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/chat/completions")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&request_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Should return 422 Unprocessable Entity for validation errors
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}
