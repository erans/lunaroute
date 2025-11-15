//! Integration tests for limits-alternative routing strategy
//!
//! These tests verify that:
//! 1. Rate limits trigger automatic failover to alternative providers
//! 2. Cross-dialect alternatives work (OpenAI → Anthropic with translation)
//! 3. Cascading through multiple alternatives when sequential rate limits occur
//! 4. Automatic recovery to primary providers after retry-after expires
//! 5. All providers rate-limited returns appropriate error
//! 6. Exponential backoff works when retry-after header is missing

mod common;

use lunaroute_core::normalized::{Message, MessageContent, NormalizedRequest, Role};
use lunaroute_core::provider::Provider;
use lunaroute_egress::anthropic::{AnthropicConfig, AnthropicConnector};
use lunaroute_egress::openai::{OpenAIConfig, OpenAIConnector};
use lunaroute_observability::metrics::Metrics;
use lunaroute_routing::provider_router::Router;
use lunaroute_routing::router::{RouteTable, RoutingRule, RuleMatcher};
use lunaroute_routing::strategy::RoutingStrategy;
use lunaroute_routing::{CircuitBreakerConfig, HealthMonitorConfig};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Helper to create a test request
fn create_test_request(model: &str) -> NormalizedRequest {
    NormalizedRequest {
        model: model.to_string(),
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("test message".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        max_tokens: Some(100),
        temperature: None,
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

/// Test 1: Basic rate limit switching to alternative provider
#[tokio::test]
async fn test_basic_rate_limit_switch() {
    // Setup: Primary returns 429, alternative returns 200
    let primary_server = MockServer::start().await;
    let alternative_server = MockServer::start().await;

    // Primary returns rate limit with retry-after
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "60")
                .set_body_json(json!({
                    "error": {
                        "message": "Rate limit exceeded",
                        "type": "rate_limit_error",
                        "code": "rate_limit_exceeded"
                    }
                })),
        )
        .expect(1)
        .mount(&primary_server)
        .await;

    // Alternative succeeds
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello from alternative!"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        })))
        .expect(1)
        .mount(&alternative_server)
        .await;

    // Create providers
    let primary_config = OpenAIConfig {
        api_key: "test-key".to_string(),
        base_url: primary_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let primary = Arc::new(OpenAIConnector::new(primary_config).await.unwrap());

    let alt_config = OpenAIConfig {
        api_key: "test-key".to_string(),
        base_url: alternative_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let alternative = Arc::new(OpenAIConnector::new(alt_config).await.unwrap());

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("primary".to_string(), primary as Arc<dyn Provider>);
    providers.insert("alternative".to_string(), alternative as Arc<dyn Provider>);

    // Create routing rule with limits-alternative strategy
    let rule = RoutingRule {
        priority: 10,
        name: Some("test-rule".to_string()),
        matcher: RuleMatcher::Always,
        strategy: Some(RoutingStrategy::LimitsAlternative {
            primary_providers: vec!["primary".to_string()],
            alternative_providers: vec!["alternative".to_string()],
            exponential_backoff_base_secs: 60,
        }),
        primary: None,
        fallbacks: vec![],
    };

    let route_table = RouteTable::with_rules(vec![rule]);

    // Create metrics to verify observability
    let metrics = Arc::new(Metrics::new().unwrap());

    let router = Router::new(
        route_table,
        providers,
        HealthMonitorConfig::default(),
        CircuitBreakerConfig::default(),
        Some(metrics.clone()),
    );

    // Send request - should succeed via alternative
    let request = create_test_request("gpt-4");
    let response = router.send(request).await.unwrap();

    assert_eq!(
        response.choices[0].message.content,
        MessageContent::Text("Hello from alternative!".to_string())
    );

    // Verify metrics were recorded
    let gathered = metrics.registry().gather();

    // Check rate_limits_total metric
    let rate_limit_metric = gathered
        .iter()
        .find(|m| m.name() == "lunaroute_rate_limits_total")
        .expect("rate_limits_total metric should exist");
    assert!(
        rate_limit_metric.metric[0]
            .counter
            .as_ref()
            .unwrap()
            .value
            .unwrap()
            >= 1.0,
        "rate_limits_total should be at least 1"
    );

    // Check rate_limit_alternatives_used metric
    let alternatives_metric = gathered
        .iter()
        .find(|m| m.name() == "lunaroute_rate_limit_alternatives_used_total")
        .expect("rate_limit_alternatives_used_total metric should exist");
    assert!(
        alternatives_metric.metric[0]
            .counter
            .as_ref()
            .unwrap()
            .value
            .unwrap()
            >= 1.0,
        "rate_limit_alternatives_used should be at least 1"
    );
}

/// Test 2: Cross-dialect alternative (OpenAI → Anthropic)
#[tokio::test]
async fn test_cross_dialect_alternative() {
    // Setup: OpenAI primary returns 429, Anthropic alternative returns 200
    let openai_server = MockServer::start().await;
    let anthropic_server = MockServer::start().await;

    // OpenAI returns rate limit
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "60")
                .set_body_json(json!({
                    "error": {
                        "message": "Rate limit exceeded",
                        "type": "rate_limit_error"
                    }
                })),
        )
        .expect(1)
        .mount(&openai_server)
        .await;

    // Anthropic succeeds (will receive translated request)
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg-test",
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "text",
                "text": "Hello from Anthropic!"
            }],
            "model": "claude-3-sonnet-20240229",
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5
            }
        })))
        .expect(1)
        .mount(&anthropic_server)
        .await;

    // Create providers
    let openai_config = OpenAIConfig {
        api_key: "test-key".to_string(),
        base_url: openai_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let openai = Arc::new(OpenAIConnector::new(openai_config).await.unwrap());

    let anthropic_config = AnthropicConfig {
        api_key: "test-key".to_string(),
        base_url: anthropic_server.uri(),
        api_version: "2023-06-01".to_string(),
        client_config: Default::default(),
        switch_notification_message: None,
    };
    let anthropic = Arc::new(AnthropicConnector::new(anthropic_config).unwrap());

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("openai".to_string(), openai as Arc<dyn Provider>);
    providers.insert("anthropic".to_string(), anthropic as Arc<dyn Provider>);

    // Create routing rule
    let rule = RoutingRule {
        priority: 10,
        name: Some("cross-dialect-rule".to_string()),
        matcher: RuleMatcher::Always,
        strategy: Some(RoutingStrategy::LimitsAlternative {
            primary_providers: vec!["openai".to_string()],
            alternative_providers: vec!["anthropic".to_string()],
            exponential_backoff_base_secs: 60,
        }),
        primary: None,
        fallbacks: vec![],
    };

    let route_table = RouteTable::with_rules(vec![rule]);
    let router = Router::new(
        route_table,
        providers,
        HealthMonitorConfig::default(),
        CircuitBreakerConfig::default(),
        None,
    );

    // Send request - should succeed via Anthropic with dialect translation
    let request = create_test_request("gpt-4");
    let response = router.send(request).await.unwrap();

    // Response should be translated back from Anthropic format
    assert_eq!(
        response.choices[0].message.content,
        MessageContent::Text("Hello from Anthropic!".to_string())
    );
}

/// Test 3: Cascade through multiple alternatives
#[tokio::test]
async fn test_cascade_through_alternatives() {
    // Setup: Primary and alt1 return 429, alt2 returns 200
    let primary_server = MockServer::start().await;
    let alt1_server = MockServer::start().await;
    let alt2_server = MockServer::start().await;

    // Primary returns rate limit
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "60")
                .set_body_json(json!({
                    "error": {"message": "Rate limit exceeded", "type": "rate_limit_error"}
                })),
        )
        .expect(1)
        .mount(&primary_server)
        .await;

    // Alt1 also returns rate limit
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "30")
                .set_body_json(json!({
                    "error": {"message": "Rate limit exceeded", "type": "rate_limit_error"}
                })),
        )
        .expect(1)
        .mount(&alt1_server)
        .await;

    // Alt2 succeeds
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "Hello from alt2!"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        })))
        .expect(1)
        .mount(&alt2_server)
        .await;

    // Create providers
    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();

    for (name, uri) in [
        ("primary", primary_server.uri()),
        ("alt1", alt1_server.uri()),
        ("alt2", alt2_server.uri()),
    ] {
        let config = OpenAIConfig {
            api_key: "test-key".to_string(),
            base_url: uri,
            organization: None,
            client_config: Default::default(),
            custom_headers: None,
            request_body_config: None,
            response_body_config: None,
            codex_auth: None,
            switch_notification_message: None,
        };
        providers.insert(
            name.to_string(),
            Arc::new(OpenAIConnector::new(config).await.unwrap()) as Arc<dyn Provider>,
        );
    }

    // Create routing rule with multiple alternatives
    let rule = RoutingRule {
        priority: 10,
        name: Some("cascade-rule".to_string()),
        matcher: RuleMatcher::Always,
        strategy: Some(RoutingStrategy::LimitsAlternative {
            primary_providers: vec!["primary".to_string()],
            alternative_providers: vec!["alt1".to_string(), "alt2".to_string()],
            exponential_backoff_base_secs: 60,
        }),
        primary: None,
        fallbacks: vec![],
    };

    let route_table = RouteTable::with_rules(vec![rule]);
    let router = Router::new(
        route_table,
        providers,
        HealthMonitorConfig::default(),
        CircuitBreakerConfig::default(),
        None,
    );

    // Send request - should succeed via alt2
    let request = create_test_request("gpt-4");
    let response = router.send(request).await.unwrap();

    assert_eq!(
        response.choices[0].message.content,
        MessageContent::Text("Hello from alt2!".to_string())
    );
}

/// Test 4: Auto-recovery to primary after retry-after expires
#[tokio::test]
async fn test_auto_recovery_to_primary() {
    // Setup: Primary returns 429 with 1s retry-after on first request, then succeeds
    let primary_server = MockServer::start().await;
    let alternative_server = MockServer::start().await;

    // First request to primary: rate limit with 1 second retry-after
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "1")
                .set_body_json(json!({
                    "error": {"message": "Rate limit exceeded", "type": "rate_limit_error"}
                })),
        )
        .expect(1)
        .up_to_n_times(1)
        .mount(&primary_server)
        .await;

    // Alternative succeeds on first request
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "alt",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "From alternative"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        })))
        .expect(1)
        .up_to_n_times(1)
        .mount(&alternative_server)
        .await;

    // Second request to primary: succeeds (after retry-after expires)
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "primary-recovered",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "Primary recovered!"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        })))
        .expect(1)
        .mount(&primary_server)
        .await;

    // Create providers
    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();

    for (name, uri) in [
        ("primary", primary_server.uri()),
        ("alternative", alternative_server.uri()),
    ] {
        let config = OpenAIConfig {
            api_key: "test-key".to_string(),
            base_url: uri,
            organization: None,
            client_config: Default::default(),
            custom_headers: None,
            request_body_config: None,
            response_body_config: None,
            codex_auth: None,
            switch_notification_message: None,
        };
        providers.insert(
            name.to_string(),
            Arc::new(OpenAIConnector::new(config).await.unwrap()) as Arc<dyn Provider>,
        );
    }

    let rule = RoutingRule {
        priority: 10,
        name: Some("recovery-rule".to_string()),
        matcher: RuleMatcher::Always,
        strategy: Some(RoutingStrategy::LimitsAlternative {
            primary_providers: vec!["primary".to_string()],
            alternative_providers: vec!["alternative".to_string()],
            exponential_backoff_base_secs: 60,
        }),
        primary: None,
        fallbacks: vec![],
    };

    let route_table = RouteTable::with_rules(vec![rule]);
    let router = Router::new(
        route_table,
        providers,
        HealthMonitorConfig::default(),
        CircuitBreakerConfig::default(),
        None,
    );

    // First request: should use alternative due to rate limit
    let request1 = create_test_request("gpt-4");
    let response1 = router.send(request1).await.unwrap();
    assert_eq!(
        response1.choices[0].message.content,
        MessageContent::Text("From alternative".to_string())
    );

    // Wait for retry-after to expire (1 second + buffer)
    tokio::time::sleep(tokio::time::Duration::from_millis(1100)).await;

    // Second request: should recover to primary
    let request2 = create_test_request("gpt-4");
    let response2 = router.send(request2).await.unwrap();
    assert_eq!(
        response2.choices[0].message.content,
        MessageContent::Text("Primary recovered!".to_string())
    );
}

/// Test 5: All providers rate-limited returns error
#[tokio::test]
async fn test_all_providers_rate_limited() {
    // Setup: All providers return 429
    let primary_server = MockServer::start().await;
    let alt_server = MockServer::start().await;

    // Both return rate limit
    for server in [&primary_server, &alt_server] {
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("retry-after", "60")
                    .set_body_json(json!({
                        "error": {"message": "Rate limit exceeded", "type": "rate_limit_error"}
                    })),
            )
            .expect(1)
            .mount(server)
            .await;
    }

    // Create providers
    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();

    for (name, uri) in [
        ("primary", primary_server.uri()),
        ("alternative", alt_server.uri()),
    ] {
        let config = OpenAIConfig {
            api_key: "test-key".to_string(),
            base_url: uri,
            organization: None,
            client_config: Default::default(),
            custom_headers: None,
            request_body_config: None,
            response_body_config: None,
            codex_auth: None,
            switch_notification_message: None,
        };
        providers.insert(
            name.to_string(),
            Arc::new(OpenAIConnector::new(config).await.unwrap()) as Arc<dyn Provider>,
        );
    }

    let rule = RoutingRule {
        priority: 10,
        name: Some("all-limited-rule".to_string()),
        matcher: RuleMatcher::Always,
        strategy: Some(RoutingStrategy::LimitsAlternative {
            primary_providers: vec!["primary".to_string()],
            alternative_providers: vec!["alternative".to_string()],
            exponential_backoff_base_secs: 60,
        }),
        primary: None,
        fallbacks: vec![],
    };

    let route_table = RouteTable::with_rules(vec![rule]);
    let router = Router::new(
        route_table,
        providers,
        HealthMonitorConfig::default(),
        CircuitBreakerConfig::default(),
        None,
    );

    // Send request - should fail with all providers rate-limited error
    let request = create_test_request("gpt-4");
    let result = router.send(request).await;

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("All providers failed") || err_msg.contains("rate limit"),
        "Expected rate limit error, got: {}",
        err_msg
    );
}

/// Test 6: Exponential backoff without retry-after header
#[tokio::test]
async fn test_exponential_backoff_without_retry_after() {
    // Setup: Primary returns 429 without retry-after header
    let primary_server = MockServer::start().await;
    let alternative_server = MockServer::start().await;

    // Primary returns rate limit WITHOUT retry-after header
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "error": {"message": "Rate limit exceeded", "type": "rate_limit_error"}
        })))
        .expect(1)
        .mount(&primary_server)
        .await;

    // Alternative succeeds
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "alt",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "From alternative"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        })))
        .expect(1)
        .mount(&alternative_server)
        .await;

    // Create providers
    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();

    for (name, uri) in [
        ("primary", primary_server.uri()),
        ("alternative", alternative_server.uri()),
    ] {
        let config = OpenAIConfig {
            api_key: "test-key".to_string(),
            base_url: uri,
            organization: None,
            client_config: Default::default(),
            custom_headers: None,
            request_body_config: None,
            response_body_config: None,
            codex_auth: None,
            switch_notification_message: None,
        };
        providers.insert(
            name.to_string(),
            Arc::new(OpenAIConnector::new(config).await.unwrap()) as Arc<dyn Provider>,
        );
    }

    let rule = RoutingRule {
        priority: 10,
        name: Some("backoff-rule".to_string()),
        matcher: RuleMatcher::Always,
        strategy: Some(RoutingStrategy::LimitsAlternative {
            primary_providers: vec!["primary".to_string()],
            alternative_providers: vec!["alternative".to_string()],
            exponential_backoff_base_secs: 30, // Custom base delay
        }),
        primary: None,
        fallbacks: vec![],
    };

    let route_table = RouteTable::with_rules(vec![rule]);
    let router = Router::new(
        route_table,
        providers,
        HealthMonitorConfig::default(),
        CircuitBreakerConfig::default(),
        None,
    );

    // Send request - should use alternative due to rate limit
    // The exponential backoff will be used (30s base delay) since no retry-after header
    let request = create_test_request("gpt-4");
    let response = router.send(request).await.unwrap();

    assert_eq!(
        response.choices[0].message.content,
        MessageContent::Text("From alternative".to_string())
    );

    // The rate limit state should have been recorded with exponential backoff
    // (We can't directly check the internal state, but the test passing proves
    // the alternative was used, which means rate limit detection worked)
}
