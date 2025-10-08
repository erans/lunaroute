//! Integration test: Custom Headers & Body Modifications
//!
//! Tests the Phase 11b feature: custom request headers and body modifications
//! including template variable substitution.

use lunaroute_core::{
    normalized::{Message, MessageContent, NormalizedRequest, Role},
    provider::Provider,
    template::TemplateContext,
};
use lunaroute_egress::openai::{OpenAIConfig, OpenAIConnector, RequestBodyModConfig};
use serde_json::json;
use std::collections::HashMap;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_custom_headers_with_template_substitution() {
    // Setup: Mock OpenAI server
    let mock_server = MockServer::start().await;

    // Verify that custom headers with template variables are correctly substituted
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer test-api-key"))
        .and(header("content-type", "application/json"))
        // These headers should have template variables substituted
        .and(header("X-Provider", "openai"))
        .and(header("X-Model", "gpt-4"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello!"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        })))
        .mount(&mock_server)
        .await;

    // Create OpenAI connector with custom headers
    let mut custom_headers = HashMap::new();
    custom_headers.insert("X-Provider".to_string(), "${provider}".to_string());
    custom_headers.insert("X-Model".to_string(), "${model}".to_string());

    let config = OpenAIConfig {
        api_key: "test-api-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: Some(custom_headers),
        request_body_config: None,
        response_body_config: None,
    };
    let connector = OpenAIConnector::new(config).unwrap();

    // Create normalized request
    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("Hello".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        model: "gpt-4".to_string(),
        max_tokens: Some(50),
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: HashMap::new(),
    };

    // Send request
    let response = connector.send(request).await.unwrap();

    // Verify response
    assert_eq!(response.model, "gpt-4");
    assert_eq!(
        response.choices[0].message.content,
        MessageContent::Text("Hello!".to_string())
    );
}

#[tokio::test]
async fn test_request_body_defaults() {
    let mock_server = MockServer::start().await;

    // Verify that defaults are applied when not present in request
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_partial_json(json!({
            "model": "gpt-4",
            "temperature": 0.7,  // Should be added from defaults
            "max_tokens": 100    // Should be added from defaults
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "Response"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        })))
        .mount(&mock_server)
        .await;

    let defaults = json!({
        "temperature": 0.7,
        "max_tokens": 100
    });

    let config = OpenAIConfig {
        api_key: "test-api-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: Some(RequestBodyModConfig {
            defaults: Some(defaults),
            overrides: None,
            prepend_messages: None,
        }),
        response_body_config: None,
    };
    let connector = OpenAIConnector::new(config).unwrap();

    // Create request WITHOUT temperature or max_tokens
    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("Test".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        model: "gpt-4".to_string(),
        max_tokens: None,  // Will be filled by defaults
        temperature: None, // Will be filled by defaults
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: HashMap::new(),
    };

    let response = connector.send(request).await.unwrap();
    assert_eq!(response.model, "gpt-4");
}

#[tokio::test]
async fn test_request_body_overrides() {
    let mock_server = MockServer::start().await;

    // Verify that overrides replace user-provided values
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_partial_json(json!({
            "model": "gpt-4",
            "temperature": 0.5,  // Should be overridden to 0.5 (not 0.9 from request)
            "max_tokens": 200    // Should be overridden to 200 (not 50 from request)
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "Response"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        })))
        .mount(&mock_server)
        .await;

    let overrides = json!({
        "temperature": 0.5,
        "max_tokens": 200
    });

    let config = OpenAIConfig {
        api_key: "test-api-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: Some(RequestBodyModConfig {
            defaults: None,
            overrides: Some(overrides),
            prepend_messages: None,
        }),
        response_body_config: None,
    };
    let connector = OpenAIConnector::new(config).unwrap();

    // Send request WITH temperature and max_tokens (should be overridden)
    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("Test".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        model: "gpt-4".to_string(),
        max_tokens: Some(50),   // Will be overridden to 200
        temperature: Some(0.9), // Will be overridden to 0.5
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: HashMap::new(),
    };

    let response = connector.send(request).await.unwrap();
    assert_eq!(response.model, "gpt-4");
}

#[tokio::test]
async fn test_prepend_messages() {
    let mock_server = MockServer::start().await;

    // Verify that messages are prepended correctly
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_partial_json(json!({
            "messages": [
                {"role": "system", "content": "You are a helpful assistant."},
                {"role": "user", "content": "Hello"}
            ]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "Hi!"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 20, "completion_tokens": 3, "total_tokens": 23}
        })))
        .mount(&mock_server)
        .await;

    let prepend_messages =
        vec![json!({"role": "system", "content": "You are a helpful assistant."})];

    let config = OpenAIConfig {
        api_key: "test-api-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: Some(RequestBodyModConfig {
            defaults: None,
            overrides: None,
            prepend_messages: Some(prepend_messages),
        }),
        response_body_config: None,
    };
    let connector = OpenAIConnector::new(config).unwrap();

    // Send request with only user message
    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("Hello".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        model: "gpt-4".to_string(),
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
    };

    let response = connector.send(request).await.unwrap();
    assert_eq!(response.model, "gpt-4");
}

#[test]
fn test_template_context_creation() {
    // Test that TemplateContext can be created and used
    let ctx = TemplateContext::new(
        "req-123".to_string(),
        "openai".to_string(),
        "gpt-4".to_string(),
    )
    .with_session_id("sess-456".to_string())
    .with_client_ip("192.168.1.100".to_string());

    assert_eq!(ctx.request_id, "req-123");
    assert_eq!(ctx.provider, "openai");
    assert_eq!(ctx.model, "gpt-4");
    assert_eq!(ctx.session_id, Some("sess-456".to_string()));
    assert_eq!(ctx.client_ip, Some("192.168.1.100".to_string()));
}

#[test]
fn test_sensitive_env_var_rejection() {
    use std::env;

    // Set some test environment variables
    unsafe {
        env::set_var("AWS_SECRET_ACCESS_KEY", "secret");
        env::set_var("GITHUB_TOKEN", "token");
        env::set_var("SAFE_VAR", "safe");
    }

    let mut ctx =
        TemplateContext::new("req-1".to_string(), "test".to_string(), "model".to_string());

    // Test that template substitution rejects sensitive vars
    let template1 = "${env.AWS_SECRET_ACCESS_KEY}";
    let result1 = lunaroute_core::template::substitute_string(template1, &mut ctx);
    assert_eq!(result1, "${env.AWS_SECRET_ACCESS_KEY}"); // Should be rejected

    let template2 = "${env.GITHUB_TOKEN}";
    let result2 = lunaroute_core::template::substitute_string(template2, &mut ctx);
    assert_eq!(result2, "${env.GITHUB_TOKEN}"); // Should be rejected

    let template3 = "${env.SAFE_VAR}";
    let result3 = lunaroute_core::template::substitute_string(template3, &mut ctx);
    assert_eq!(result3, "safe"); // Should be allowed

    unsafe {
        env::remove_var("AWS_SECRET_ACCESS_KEY");
        env::remove_var("GITHUB_TOKEN");
        env::remove_var("SAFE_VAR");
    }
}
