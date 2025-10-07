//! Integration tests for Anthropic connector using wiremock
//!
//! These tests mock the Anthropic API to verify the egress connector's HTTP behavior.

use lunaroute_core::{
    normalized::{Message, MessageContent, NormalizedRequest, Role},
    provider::Provider,
};
use lunaroute_egress::anthropic::{AnthropicConfig, AnthropicConnector};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

#[tokio::test]
async fn test_anthropic_send_success() {
    // Start mock server
    let mock_server = MockServer::start().await;

    // Mock successful Anthropic response
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-key"))
        .and(header("anthropic-version", "2023-06-01"))
        .and(header("content-type", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "model": "claude-3-opus",
            "content": [{
                "type": "text",
                "text": "Hello from mock Anthropic API!"
            }],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 8
            }
        })))
        .mount(&mock_server)
        .await;

    // Create connector pointing to mock server
    let config = AnthropicConfig {
        api_key: "test-key".to_string(),
        base_url: mock_server.uri(),
        api_version: "2023-06-01".to_string(),
        client_config: Default::default(),
    };
    let connector = AnthropicConnector::new(config).unwrap();

    // Create request
    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("Hello!".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        model: "claude-3-opus".to_string(),
        max_tokens: Some(100),
        temperature: Some(0.7),
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    // Send request
    let response = connector.send(request).await.unwrap();

    // Verify response
    assert_eq!(response.model, "claude-3-opus");
    assert_eq!(response.choices.len(), 1);
    assert_eq!(
        response.choices[0].message.content,
        MessageContent::Text("Hello from mock Anthropic API!".to_string())
    );
    assert_eq!(response.usage.prompt_tokens, 10);
    assert_eq!(response.usage.completion_tokens, 8);
    assert_eq!(response.usage.total_tokens, 18);
}

#[tokio::test]
async fn test_anthropic_send_with_system() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "model": "claude-3-opus",
            "content": [{
                "type": "text",
                "text": "I am a helpful assistant"
            }],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 15,
                "output_tokens": 5
            }
        })))
        .mount(&mock_server)
        .await;

    let config = AnthropicConfig {
        api_key: "test-key".to_string(),
        base_url: mock_server.uri(),
        api_version: "2023-06-01".to_string(),
        client_config: Default::default(),
    };
    let connector = AnthropicConnector::new(config).unwrap();

    let request = NormalizedRequest {
        messages: vec![
            Message {
                role: Role::System,
                content: MessageContent::Text("You are helpful".to_string()),
                name: None,
                tool_calls: vec![],
                tool_call_id: None,
            },
            Message {
                role: Role::User,
                content: MessageContent::Text("Hello".to_string()),
                name: None,
                tool_calls: vec![],
                tool_call_id: None,
            },
        ],
        system: None,
        model: "claude-3-opus".to_string(),
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    let response = connector.send(request).await;
    assert!(response.is_ok());
}

#[tokio::test]
async fn test_anthropic_send_rate_limit_error() {
    let mock_server = MockServer::start().await;

    // Mock rate limit response (429)
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
            "type": "error",
            "error": {
                "type": "rate_limit_error",
                "message": "Rate limit exceeded"
            }
        })))
        .mount(&mock_server)
        .await;

    let config = AnthropicConfig {
        api_key: "test-key".to_string(),
        base_url: mock_server.uri(),
        api_version: "2023-06-01".to_string(),
        client_config: Default::default(),
    };
    let connector = AnthropicConnector::new(config).unwrap();

    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("Test".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        model: "claude-3-opus".to_string(),
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    // Should retry and eventually fail
    let result = connector.send(request).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_anthropic_send_server_error_with_retry() {
    let mock_server = MockServer::start().await;

    // First two requests fail with 500
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
            "type": "error",
            "error": {
                "type": "api_error",
                "message": "Internal server error"
            }
        })))
        .up_to_n_times(2)
        .mount(&mock_server)
        .await;

    // Third request succeeds
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "model": "claude-3-opus",
            "content": [{
                "type": "text",
                "text": "Success after retry"
            }],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 5,
                "output_tokens": 3
            }
        })))
        .mount(&mock_server)
        .await;

    let config = AnthropicConfig {
        api_key: "test-key".to_string(),
        base_url: mock_server.uri(),
        api_version: "2023-06-01".to_string(),
        client_config: Default::default(),
    };
    let connector = AnthropicConnector::new(config).unwrap();

    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("Test".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        model: "claude-3-opus".to_string(),
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    // Should succeed after retries
    let response = connector.send(request).await.unwrap();
    assert_eq!(
        response.choices[0].message.content,
        MessageContent::Text("Success after retry".to_string())
    );
}

#[tokio::test]
async fn test_anthropic_send_invalid_api_key() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "type": "error",
            "error": {
                "type": "authentication_error",
                "message": "Invalid API key"
            }
        })))
        .mount(&mock_server)
        .await;

    let config = AnthropicConfig {
        api_key: "invalid-key".to_string(),
        base_url: mock_server.uri(),
        api_version: "2023-06-01".to_string(),
        client_config: Default::default(),
    };
    let connector = AnthropicConnector::new(config).unwrap();

    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("Test".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        model: "claude-3-opus".to_string(),
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    // Should fail with authentication error
    let result = connector.send(request).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_anthropic_send_with_tool_calls() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "model": "claude-3-opus",
            "content": [
                {
                    "type": "text",
                    "text": "Let me check the weather"
                },
                {
                    "type": "tool_use",
                    "id": "call_123",
                    "name": "get_weather",
                    "input": {"location": "NYC"}
                }
            ],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 20,
                "output_tokens": 15
            }
        })))
        .mount(&mock_server)
        .await;

    let config = AnthropicConfig {
        api_key: "test-key".to_string(),
        base_url: mock_server.uri(),
        api_version: "2023-06-01".to_string(),
        client_config: Default::default(),
    };
    let connector = AnthropicConnector::new(config).unwrap();

    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("What's the weather in NYC?".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        model: "claude-3-opus".to_string(),
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    let response = connector.send(request).await.unwrap();
    assert_eq!(response.choices[0].message.tool_calls.len(), 1);
    assert_eq!(
        response.choices[0].message.tool_calls[0].function.name,
        "get_weather"
    );
}
