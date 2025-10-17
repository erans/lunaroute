//! Integration test: Anthropic request → OpenAI API translation
//!
//! This test verifies that an Anthropic-formatted request is correctly
//! translated and sent to the OpenAI API in the correct format.

use axum::body::Body;
use axum::http::Request;
use lunaroute_egress::openai::{OpenAIConfig, OpenAIConnector};
use lunaroute_ingress::anthropic;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_anthropic_request_translates_to_openai_api() {
    // Setup: Mock OpenAI server
    let mock_server = MockServer::start().await;

    // Verify the outgoing OpenAI request has the correct format
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer test-api-key"))
        .and(header("content-type", "application/json"))
        // Verify OpenAI request structure
        .and(body_partial_json(json!({
            "model": "gpt-4",
            "messages": [{
                "role": "user",
                "content": "Hello from Anthropic format!"
            }],
            "max_tokens": 100,
            "temperature": 0.7
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello! I received your message."
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 8,
                "total_tokens": 18
            }
        })))
        .expect(1) // Verify this endpoint is called exactly once
        .mount(&mock_server)
        .await;

    // Create OpenAI connector pointing to mock server
    let config = OpenAIConfig {
        api_key: "test-api-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
    };
    let openai_connector = OpenAIConnector::new(config).await.unwrap();

    // Create Anthropic ingress router with OpenAI as the egress provider
    let app = anthropic::router(Arc::new(openai_connector));

    // Send Anthropic-formatted request
    let anthropic_request = json!({
        "model": "gpt-4",
        "max_tokens": 100,
        "temperature": 0.7,
        "messages": [{
            "role": "user",
            "content": "Hello from Anthropic format!"
        }]
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/messages")
                .method("POST")
                .header("content-type", "application/json")
                .header("anthropic-version", "2023-06-01")
                .body(Body::from(serde_json::to_vec(&anthropic_request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Verify the response is successful
    assert_eq!(response.status(), 200);

    // Verify the mock server received the request (checked by wiremock expectations)
    // The .expect(1) above ensures the OpenAI endpoint was called exactly once
}

#[tokio::test]
async fn test_anthropic_request_with_temperature() {
    // Setup: Mock OpenAI server
    let mock_server = MockServer::start().await;

    // Verify temperature parameter is correctly passed through
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_partial_json(json!({
            "model": "gpt-4",
            "temperature": 0.9,
            "max_tokens": 50
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-456",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Creative response!"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 3,
                "total_tokens": 13
            }
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = OpenAIConfig {
        api_key: "test-api-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
    };
    let openai_connector = OpenAIConnector::new(config).await.unwrap();
    let app = anthropic::router(Arc::new(openai_connector));

    let anthropic_request = json!({
        "model": "gpt-4",
        "max_tokens": 50,
        "temperature": 0.9,
        "messages": [{
            "role": "user",
            "content": "Be creative!"
        }]
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/messages")
                .method("POST")
                .header("content-type", "application/json")
                .header("anthropic-version", "2023-06-01")
                .body(Body::from(serde_json::to_vec(&anthropic_request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn test_anthropic_assistant_message_in_conversation() {
    // Setup: Mock OpenAI server
    let mock_server = MockServer::start().await;

    // Test that conversation history (user + assistant + user) translates correctly
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_partial_json(json!({
            "model": "gpt-4",
            "messages": [
                {"role": "user", "content": "What's 2+2?"},
                {"role": "assistant", "content": "4"},
                {"role": "user", "content": "What's 3+3?"}
            ]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-789",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "6"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 20,
                "completion_tokens": 1,
                "total_tokens": 21
            }
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = OpenAIConfig {
        api_key: "test-api-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
    };
    let openai_connector = OpenAIConnector::new(config).await.unwrap();
    let app = anthropic::router(Arc::new(openai_connector));

    let anthropic_request = json!({
        "model": "gpt-4",
        "max_tokens": 100,
        "messages": [
            {"role": "user", "content": "What's 2+2?"},
            {"role": "assistant", "content": "4"},
            {"role": "user", "content": "What's 3+3?"}
        ]
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/messages")
                .method("POST")
                .header("content-type", "application/json")
                .header("anthropic-version", "2023-06-01")
                .body(Body::from(serde_json::to_vec(&anthropic_request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
}
#[tokio::test]
async fn test_anthropic_request_streaming_translates_to_openai() {
    // Setup: Mock OpenAI server with streaming response
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer test-api-key"))
        .and(body_partial_json(json!({
            "model": "gpt-4",
            "stream": true
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4","choices":[{"index":0,"delta":{"content":"Streaming"},"finish_reason":null}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4","choices":[{"index":0,"delta":{"content":" response"},"finish_reason":null}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}

data: [DONE]

"#
        ))
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = OpenAIConfig {
        api_key: "test-api-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
    };
    let openai_connector = OpenAIConnector::new(config).await.unwrap();
    let app = anthropic::router(Arc::new(openai_connector));

    // Send Anthropic streaming request (should be translated to OpenAI format)
    let anthropic_request = json!({
        "model": "gpt-4",
        "max_tokens": 50,
        "stream": true,
        "messages": [{
            "role": "user",
            "content": "Stream this!"
        }]
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/messages")
                .method("POST")
                .header("content-type", "application/json")
                .header("anthropic-version", "2023-06-01")
                .body(Body::from(serde_json::to_vec(&anthropic_request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Verify streaming response
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/event-stream"
    );
}
