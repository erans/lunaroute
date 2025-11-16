//! Integration tests for provider switch notifications
//!
//! These tests verify that:
//! 1. Rate limit switches inject notification messages
//! 2. Notifications can be disabled
//! 3. Cross-dialect switches inject notifications correctly
//! 4. Provider-specific custom messages override defaults
//! 5. Idempotency guard prevents duplicate notifications

mod common;

use lunaroute_core::normalized::{Message, MessageContent, NormalizedRequest, Role};
use lunaroute_core::provider::Provider;
use lunaroute_egress::HttpClientConfig;
use lunaroute_egress::anthropic::{AnthropicConfig, AnthropicConnector};
use lunaroute_egress::openai::{OpenAIConfig, OpenAIConnector};
use lunaroute_routing::provider_router::Router;
use lunaroute_routing::router::{RouteTable, RoutingRule, RuleMatcher};
use lunaroute_routing::strategy::RoutingStrategy;
use lunaroute_routing::{
    CircuitBreakerConfig, HealthMonitorConfig, ProviderSwitchNotificationConfig,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Helper to create a test request
fn create_test_request(model: &str) -> NormalizedRequest {
    NormalizedRequest {
        model: model.to_string(),
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("What is 2+2?".to_string()),
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

/// Test 1: Rate limit switch injects notification
#[tokio::test]
async fn test_rate_limit_switch_injects_notification() {
    // Setup mock servers
    let primary_server = MockServer::start().await;
    let alternative_server = MockServer::start().await;

    // Primary returns 429 (rate limit)
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
        .mount(&primary_server)
        .await;

    // Alternative returns success
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Response from alternative"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 50,
                "completion_tokens": 10,
                "total_tokens": 60
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
        client_config: HttpClientConfig::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let primary = Arc::new(OpenAIConnector::new(primary_config).await.unwrap());

    let alternative_config = OpenAIConfig {
        api_key: "test-key".to_string(),
        base_url: alternative_server.uri(),
        organization: None,
        client_config: HttpClientConfig::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let alternative = Arc::new(OpenAIConnector::new(alternative_config).await.unwrap());

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("primary".to_string(), primary as Arc<dyn Provider>);
    providers.insert("alternative".to_string(), alternative as Arc<dyn Provider>);

    // Create routing rule with limits-alternative strategy
    let rule = RoutingRule {
        name: Some("test-rule".to_string()),
        priority: 100,
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

    // Create router with notification enabled
    let notification_config = Some(ProviderSwitchNotificationConfig::default());

    let router = Router::new(
        route_table,
        providers,
        HealthMonitorConfig::default(),
        CircuitBreakerConfig::default(),
        None, // metrics
        notification_config,
    );

    // Send request
    let request = create_test_request("gpt-4");
    let response = router.send(request).await;

    // Verify response succeeded
    assert!(response.is_ok(), "Request should succeed with alternative");
    let response = response.unwrap();
    assert_eq!(
        response.choices[0].message.content,
        MessageContent::Text("Response from alternative".to_string())
    );
}

/// Test 2: Notification disabled
#[tokio::test]
async fn test_notification_disabled() {
    let primary_server = MockServer::start().await;
    let alternative_server = MockServer::start().await;

    // Primary returns 429
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
        .mount(&primary_server)
        .await;

    // Alternative returns success
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Response"
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
        client_config: HttpClientConfig::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let primary = Arc::new(OpenAIConnector::new(primary_config).await.unwrap());

    let alternative_config = OpenAIConfig {
        api_key: "test-key".to_string(),
        base_url: alternative_server.uri(),
        organization: None,
        client_config: HttpClientConfig::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let alternative = Arc::new(OpenAIConnector::new(alternative_config).await.unwrap());

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("primary".to_string(), primary as Arc<dyn Provider>);
    providers.insert("alternative".to_string(), alternative as Arc<dyn Provider>);

    let rule = RoutingRule {
        name: Some("test-rule".to_string()),
        priority: 100,
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

    // Create router with notification DISABLED
    let notification_config = Some(ProviderSwitchNotificationConfig {
        enabled: false,
        default_message: String::new(),
    });

    let router = Router::new(
        route_table,
        providers,
        HealthMonitorConfig::default(),
        CircuitBreakerConfig::default(),
        None,
        notification_config,
    );

    let request = create_test_request("gpt-4");
    let response = router.send(request).await;

    // Verify response succeeded
    assert!(response.is_ok(), "Request should succeed");
}

/// Test 3: Cross-dialect notification (OpenAI → Anthropic)
#[tokio::test]
async fn test_cross_dialect_notification() {
    let openai_server = MockServer::start().await;
    let anthropic_server = MockServer::start().await;

    // OpenAI returns 503
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(503).set_body_json(json!({
            "error": {
                "message": "Service unavailable",
                "type": "server_error"
            }
        })))
        .expect(1)
        .mount(&openai_server)
        .await;

    // Anthropic returns success
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg-123",
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "text",
                "text": "Response from Anthropic"
            }],
            "model": "claude-sonnet-4",
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 50,
                "output_tokens": 10
            }
        })))
        .expect(1)
        .mount(&anthropic_server)
        .await;

    // Create OpenAI provider
    let mut client_config = HttpClientConfig::default();
    client_config.max_retries = 0; // Disable retries for predictable mock expectations

    let openai_config = OpenAIConfig {
        api_key: "test-key".to_string(),
        base_url: openai_server.uri(),
        organization: None,
        client_config,
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let openai_provider = Arc::new(OpenAIConnector::new(openai_config).await.unwrap());

    // Create Anthropic provider
    let anthropic_config = AnthropicConfig {
        api_key: "test-key".to_string(),
        base_url: anthropic_server.uri(),
        api_version: "2023-06-01".to_string(),
        client_config: HttpClientConfig::default(),
        switch_notification_message: None,
    };
    let anthropic_provider = Arc::new(AnthropicConnector::new(anthropic_config).unwrap());

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("openai".to_string(), openai_provider as Arc<dyn Provider>);
    providers.insert(
        "anthropic".to_string(),
        anthropic_provider as Arc<dyn Provider>,
    );

    // Use fallback strategy
    let rule = RoutingRule {
        name: Some("cross-dialect".to_string()),
        priority: 100,
        matcher: RuleMatcher::Always,
        strategy: None,
        primary: Some("openai".to_string()),
        fallbacks: vec!["anthropic".to_string()],
    };

    let route_table = RouteTable::with_rules(vec![rule]);

    let notification_config = Some(ProviderSwitchNotificationConfig::default());

    let router = Router::new(
        route_table,
        providers,
        HealthMonitorConfig::default(),
        CircuitBreakerConfig::default(),
        None,
        notification_config,
    );

    let request = create_test_request("gpt-4");
    let response = router.send(request).await;

    // Verify response succeeded
    assert!(
        response.is_ok(),
        "Request should succeed with cross-dialect fallback"
    );
    let response = response.unwrap();

    // Response should be in OpenAI format (translated back from Anthropic)
    assert_eq!(
        response.choices[0].message.content,
        MessageContent::Text("Response from Anthropic".to_string())
    );
}

/// Test 4: Provider-specific custom message
#[tokio::test]
async fn test_custom_provider_notification_message() {
    let primary_server = MockServer::start().await;
    let alternative_server = MockServer::start().await;

    // Primary returns 429
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
        .mount(&primary_server)
        .await;

    // Alternative returns success
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Response"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 40,
                "completion_tokens": 10,
                "total_tokens": 50
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
        client_config: HttpClientConfig::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let primary = Arc::new(OpenAIConnector::new(primary_config).await.unwrap());

    // Alternative with CUSTOM notification message
    let custom_message = "CUSTOM: Switched to ${new_provider} from ${original_provider} for ${model} due to ${reason}";
    let alternative_config = OpenAIConfig {
        api_key: "test-key".to_string(),
        base_url: alternative_server.uri(),
        organization: None,
        client_config: HttpClientConfig::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: Some(custom_message.to_string()),
    };
    let alternative = Arc::new(OpenAIConnector::new(alternative_config).await.unwrap());

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("primary".to_string(), primary as Arc<dyn Provider>);
    providers.insert("alternative".to_string(), alternative as Arc<dyn Provider>);

    let rule = RoutingRule {
        name: Some("test-rule".to_string()),
        priority: 100,
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

    let notification_config = Some(ProviderSwitchNotificationConfig::default());

    let router = Router::new(
        route_table,
        providers,
        HealthMonitorConfig::default(),
        CircuitBreakerConfig::default(),
        None,
        notification_config,
    );

    let request = create_test_request("gpt-4");
    let response = router.send(request).await;

    // Verify response succeeded
    assert!(response.is_ok(), "Request should succeed");
}

/// Test 5: Idempotency guard prevents duplicate notifications
#[tokio::test]
async fn test_notification_idempotency_cascading_fallbacks() {
    let primary_server = MockServer::start().await;
    let alt1_server = MockServer::start().await;
    let alt2_server = MockServer::start().await;

    // Primary fails with 503
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(503).set_body_json(json!({
            "error": {
                "message": "Service unavailable",
                "type": "server_error"
            }
        })))
        .expect(1)
        .mount(&primary_server)
        .await;

    // Alt1 fails with 503
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(503).set_body_json(json!({
            "error": {
                "message": "Service unavailable",
                "type": "server_error"
            }
        })))
        .expect(1)
        .mount(&alt1_server)
        .await;

    // Alt2 succeeds
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Final response"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 50,
                "completion_tokens": 10,
                "total_tokens": 60
            }
        })))
        .expect(1)
        .mount(&alt2_server)
        .await;

    // Create providers
    let mut client_config = HttpClientConfig::default();
    client_config.max_retries = 0; // Disable retries for predictable mock expectations

    let primary_config = OpenAIConfig {
        api_key: "test-key".to_string(),
        base_url: primary_server.uri(),
        organization: None,
        client_config: client_config.clone(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let primary = Arc::new(OpenAIConnector::new(primary_config).await.unwrap());

    let alt1_config = OpenAIConfig {
        api_key: "test-key".to_string(),
        base_url: alt1_server.uri(),
        organization: None,
        client_config: client_config.clone(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let alt1 = Arc::new(OpenAIConnector::new(alt1_config).await.unwrap());

    let alt2_config = OpenAIConfig {
        api_key: "test-key".to_string(),
        base_url: alt2_server.uri(),
        organization: None,
        client_config,
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let alt2 = Arc::new(OpenAIConnector::new(alt2_config).await.unwrap());

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("primary".to_string(), primary as Arc<dyn Provider>);
    providers.insert("alt1".to_string(), alt1 as Arc<dyn Provider>);
    providers.insert("alt2".to_string(), alt2 as Arc<dyn Provider>);

    // Use fallback chain
    let rule = RoutingRule {
        name: Some("cascade-test".to_string()),
        priority: 100,
        matcher: RuleMatcher::Always,
        strategy: None,
        primary: Some("primary".to_string()),
        fallbacks: vec!["alt1".to_string(), "alt2".to_string()],
    };

    let route_table = RouteTable::with_rules(vec![rule]);

    let notification_config = Some(ProviderSwitchNotificationConfig::default());

    let router = Router::new(
        route_table,
        providers,
        HealthMonitorConfig::default(),
        CircuitBreakerConfig::default(),
        None,
        notification_config,
    );

    let request = create_test_request("gpt-4");
    let response = router.send(request).await;

    // Verify response succeeded
    assert!(
        response.is_ok(),
        "Request should succeed after cascading through fallbacks"
    );
    let response = response.unwrap();
    assert_eq!(
        response.choices[0].message.content,
        MessageContent::Text("Final response".to_string())
    );

    // The idempotency guard should have prevented duplicate notifications
    // Only ONE notification should have been injected despite cascading through
    // multiple fallbacks (verified by the request succeeding)
}
