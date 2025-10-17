//! Integration tests for OpenAI connector using wiremock
//!
//! These tests mock the OpenAI API to verify the egress connector's HTTP behavior.

use lunaroute_core::{
    normalized::{Message, MessageContent, NormalizedRequest, Role},
    provider::Provider,
};
use lunaroute_egress::openai::{OpenAIConfig, OpenAIConnector};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

#[tokio::test]
async fn test_openai_send_success() {
    // Start mock server
    let mock_server = MockServer::start().await;

    // Mock successful OpenAI response
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .and(header("content-type", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello from mock API!"
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

    // Create connector pointing to mock server
    let config = OpenAIConfig {
        api_key: "test-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
    };
    let connector = OpenAIConnector::new(config).await.unwrap();

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
        model: "gpt-4".to_string(),
        max_tokens: Some(100),
        temperature: Some(0.7),
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    // Send request
    let response = connector.send(request).await.unwrap();

    // Verify response
    assert_eq!(response.model, "gpt-4");
    assert_eq!(response.choices.len(), 1);
    assert_eq!(
        response.choices[0].message.content,
        MessageContent::Text("Hello from mock API!".to_string())
    );
    assert_eq!(response.usage.total_tokens, 15);
}

#[tokio::test]
async fn test_openai_send_with_organization() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .and(header("openai-organization", "org-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1234567890,
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
                "prompt_tokens": 5,
                "completion_tokens": 3,
                "total_tokens": 8
            }
        })))
        .mount(&mock_server)
        .await;

    let config = OpenAIConfig {
        api_key: "test-key".to_string(),
        base_url: mock_server.uri(),
        organization: Some("org-123".to_string()),
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
    };
    let connector = OpenAIConnector::new(config).await.unwrap();

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
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    let response = connector.send(request).await;
    assert!(response.is_ok());
}

#[tokio::test]
async fn test_openai_send_rate_limit_error() {
    let mock_server = MockServer::start().await;

    // Mock rate limit response (429)
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(429)
                .set_body_json(serde_json::json!({
                    "error": {
                        "message": "Rate limit exceeded",
                        "type": "rate_limit_error",
                        "code": "rate_limit_exceeded"
                    }
                }))
                .insert_header("retry-after", "60"),
        )
        .mount(&mock_server)
        .await;

    let config = OpenAIConfig {
        api_key: "test-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
    };
    let connector = OpenAIConnector::new(config).await.unwrap();

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
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    // Should retry and eventually fail
    let result = connector.send(request).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_openai_send_server_error_with_retry() {
    let mock_server = MockServer::start().await;

    // First two requests fail with 500
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
            "error": {
                "message": "Internal server error",
                "type": "server_error"
            }
        })))
        .up_to_n_times(2)
        .mount(&mock_server)
        .await;

    // Third request succeeds
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Success after retry"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 5,
                "completion_tokens": 3,
                "total_tokens": 8
            }
        })))
        .mount(&mock_server)
        .await;

    let config = OpenAIConfig {
        api_key: "test-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
    };
    let connector = OpenAIConnector::new(config).await.unwrap();

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
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
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
async fn test_openai_send_invalid_api_key() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": {
                "message": "Invalid API key",
                "type": "invalid_request_error",
                "code": "invalid_api_key"
            }
        })))
        .mount(&mock_server)
        .await;

    let config = OpenAIConfig {
        api_key: "invalid-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
    };
    let connector = OpenAIConnector::new(config).await.unwrap();

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
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    // Should fail with authentication error
    let result = connector.send(request).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_openai_send_with_tools() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "call_123",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"location\":\"NYC\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 15,
                "completion_tokens": 10,
                "total_tokens": 25
            }
        })))
        .mount(&mock_server)
        .await;

    let config = OpenAIConfig {
        api_key: "test-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
    };
    let connector = OpenAIConnector::new(config).await.unwrap();

    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("What's the weather in NYC?".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        model: "gpt-4".to_string(),
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
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

// Codex authentication tests

#[tokio::test]
async fn test_openai_codex_auth_with_valid_token() {
    use lunaroute_egress::openai::CodexAuthConfig;
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;

    let mock_server = MockServer::start().await;
    let temp_dir = TempDir::new().unwrap();
    let auth_file = temp_dir.path().join("auth.json");

    // Create auth file with nested structure
    let mut file = fs::File::create(&auth_file).unwrap();
    file.write_all(br#"{"tokens": {"access_token": "codex-token-123"}}"#)
        .unwrap();

    // Mock expects the Codex token, not the configured API key
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer codex-token-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Response with Codex auth"
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

    let config = OpenAIConfig {
        api_key: "fallback-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: Some(CodexAuthConfig {
            account_id: None,
            enabled: true,
            auth_file,
            token_field: "tokens.access_token".to_string(),
        }),
    };
    let connector = OpenAIConnector::new(config).await.unwrap();

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
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    let response = connector.send(request).await.unwrap();
    assert_eq!(
        response.choices[0].message.content,
        MessageContent::Text("Response with Codex auth".to_string())
    );
}

#[tokio::test]
async fn test_openai_codex_auth_fallback_to_api_key() {
    use lunaroute_egress::openai::CodexAuthConfig;
    use tempfile::TempDir;

    let mock_server = MockServer::start().await;
    let temp_dir = TempDir::new().unwrap();
    let auth_file = temp_dir.path().join("nonexistent.json");

    // Mock expects fallback API key since Codex token file doesn't exist
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer fallback-api-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Response with fallback key"
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

    let config = OpenAIConfig {
        api_key: "fallback-api-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: Some(CodexAuthConfig {
            account_id: None,
            enabled: true,
            auth_file,
            token_field: "tokens.access_token".to_string(),
        }),
    };
    let connector = OpenAIConnector::new(config).await.unwrap();

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
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    let response = connector.send(request).await.unwrap();
    assert_eq!(
        response.choices[0].message.content,
        MessageContent::Text("Response with fallback key".to_string())
    );
}

#[tokio::test]
async fn test_openai_codex_auth_disabled() {
    use lunaroute_egress::openai::CodexAuthConfig;
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;

    let mock_server = MockServer::start().await;
    let temp_dir = TempDir::new().unwrap();
    let auth_file = temp_dir.path().join("auth.json");

    // Create auth file with token
    let mut file = fs::File::create(&auth_file).unwrap();
    file.write_all(br#"{"tokens": {"access_token": "codex-token-123"}}"#)
        .unwrap();

    // Mock expects configured API key since Codex auth is disabled
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer configured-api-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Response with configured key"
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

    let config = OpenAIConfig {
        api_key: "configured-api-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: Some(CodexAuthConfig {
            account_id: None,
            enabled: false, // Disabled
            auth_file,
            token_field: "tokens.access_token".to_string(),
        }),
    };
    let connector = OpenAIConnector::new(config).await.unwrap();

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
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    let response = connector.send(request).await.unwrap();
    assert_eq!(
        response.choices[0].message.content,
        MessageContent::Text("Response with configured key".to_string())
    );
}

#[tokio::test]
async fn test_openai_codex_auth_with_invalid_json() {
    use lunaroute_egress::openai::CodexAuthConfig;
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;

    let mock_server = MockServer::start().await;
    let temp_dir = TempDir::new().unwrap();
    let auth_file = temp_dir.path().join("auth.json");

    // Create auth file with invalid JSON
    let mut file = fs::File::create(&auth_file).unwrap();
    file.write_all(b"not valid json").unwrap();

    // Mock expects fallback API key since Codex token can't be read
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer fallback-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Response with fallback"
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

    let config = OpenAIConfig {
        api_key: "fallback-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: Some(CodexAuthConfig {
            account_id: None,
            enabled: true,
            auth_file,
            token_field: "tokens.access_token".to_string(),
        }),
    };
    let connector = OpenAIConnector::new(config).await.unwrap();

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
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    // Should succeed with fallback key despite invalid Codex auth file
    let response = connector.send(request).await.unwrap();
    assert_eq!(
        response.choices[0].message.content,
        MessageContent::Text("Response with fallback".to_string())
    );
}

#[tokio::test]
async fn test_openai_codex_auth_flat_json_structure() {
    use lunaroute_egress::openai::CodexAuthConfig;
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;

    let mock_server = MockServer::start().await;
    let temp_dir = TempDir::new().unwrap();
    let auth_file = temp_dir.path().join("auth.json");

    // Create auth file with flat structure (legacy format)
    let mut file = fs::File::create(&auth_file).unwrap();
    file.write_all(br#"{"access_token": "flat-token-456"}"#)
        .unwrap();

    // Mock expects the flat token
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer flat-token-456"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Response with flat token"
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

    let config = OpenAIConfig {
        api_key: "fallback-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: Some(CodexAuthConfig {
            account_id: None,
            enabled: true,
            auth_file,
            token_field: "access_token".to_string(), // Flat path
        }),
    };
    let connector = OpenAIConnector::new(config).await.unwrap();

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
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    let response = connector.send(request).await.unwrap();
    assert_eq!(
        response.choices[0].message.content,
        MessageContent::Text("Response with flat token".to_string())
    );
}
