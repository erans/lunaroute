//! Integration test: OpenAI request → Anthropic API translation
//!
//! This test verifies that an OpenAI-formatted request is correctly
//! translated and sent to the Anthropic API in the correct format.

use axum::body::Body;
use axum::http::Request;
use lunaroute_egress::anthropic::{AnthropicConfig, AnthropicConnector};
use lunaroute_ingress::openai;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_openai_request_translates_to_anthropic_api() {
    // Setup: Mock Anthropic server
    let mock_server = MockServer::start().await;

    // Verify the outgoing Anthropic request has the correct format
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-api-key"))
        .and(header("content-type", "application/json"))
        // Verify Anthropic request structure
        .and(body_partial_json(json!({
            "model": "claude-sonnet-4-5",
            "messages": [{
                "role": "user",
                "content": "Hello from OpenAI format!"
            }],
            "max_tokens": 100
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "text",
                "text": "Hello! I received your message."
            }],
            "model": "claude-sonnet-4-5",
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 8
            }
        })))
        .expect(1) // Verify this endpoint is called exactly once
        .mount(&mock_server)
        .await;

    // Create Anthropic connector pointing to mock server
    let config = AnthropicConfig {
        api_key: "test-api-key".to_string(),
        base_url: mock_server.uri(),
        api_version: "2023-06-01".to_string(),
        client_config: Default::default(),
    };
    let anthropic_connector = AnthropicConnector::new(config).unwrap();

    // Create OpenAI ingress router with Anthropic as the egress provider
    let app = openai::router(Arc::new(anthropic_connector));

    // Send OpenAI-formatted request
    let openai_request = json!({
        "model": "claude-sonnet-4-5",
        "messages": [{
            "role": "user",
            "content": "Hello from OpenAI format!"
        }],
        "max_tokens": 100
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/chat/completions")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&openai_request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Verify the response is successful
    assert_eq!(response.status(), 200);

    // Verify the mock server received the request (checked by wiremock expectations)
    // The .expect(1) above ensures the Anthropic endpoint was called exactly once
}

#[tokio::test]
async fn test_openai_request_with_temperature_translates_to_anthropic() {
    // Setup: Mock Anthropic server
    let mock_server = MockServer::start().await;

    // Verify temperature parameter is correctly passed through
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(body_partial_json(json!({
            "model": "claude-sonnet-4-5",
            "temperature": 0.9,
            "max_tokens": 50
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_456",
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "text",
                "text": "Creative response!"
            }],
            "model": "claude-sonnet-4-5",
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 3
            }
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = AnthropicConfig {
        api_key: "test-api-key".to_string(),
        base_url: mock_server.uri(),
        api_version: "2023-06-01".to_string(),
        client_config: Default::default(),
    };
    let anthropic_connector = AnthropicConnector::new(config).unwrap();
    let app = openai::router(Arc::new(anthropic_connector));

    let openai_request = json!({
        "model": "claude-sonnet-4-5",
        "messages": [{
            "role": "user",
            "content": "Be creative!"
        }],
        "max_tokens": 50,
        "temperature": 0.9
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/chat/completions")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&openai_request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn test_openai_system_message_translates_to_anthropic() {
    // Setup: Mock Anthropic server
    let mock_server = MockServer::start().await;

    // Test that OpenAI system messages translate correctly to Anthropic format
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(body_partial_json(json!({
            "model": "claude-sonnet-4-5",
            "system": "You are a helpful assistant.",
            "messages": [{
                "role": "user",
                "content": "Hello"
            }]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_789",
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "text",
                "text": "Hi there!"
            }],
            "model": "claude-sonnet-4-5",
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 15,
                "output_tokens": 3
            }
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = AnthropicConfig {
        api_key: "test-api-key".to_string(),
        base_url: mock_server.uri(),
        api_version: "2023-06-01".to_string(),
        client_config: Default::default(),
    };
    let anthropic_connector = AnthropicConnector::new(config).unwrap();
    let app = openai::router(Arc::new(anthropic_connector));

    // OpenAI format with system message
    let openai_request = json!({
        "model": "claude-sonnet-4-5",
        "messages": [
            {"role": "system", "content": "You are a helpful assistant."},
            {"role": "user", "content": "Hello"}
        ],
        "max_tokens": 100
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/chat/completions")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&openai_request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn test_openai_request_streaming_translates_to_anthropic() {
    // Setup: Mock Anthropic server with streaming response
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-api-key"))
        .and(body_partial_json(json!({
            "model": "claude-sonnet-4-5",
            "stream": true
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_123","type":"message","role":"assistant","content":[],"model":"claude-sonnet-4-5","usage":{"input_tokens":10,"output_tokens":0}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Streaming"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" from"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" Claude"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}

event: message_stop
data: {"type":"message_stop"}

"#
        ))
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = AnthropicConfig {
        api_key: "test-api-key".to_string(),
        base_url: mock_server.uri(),
        api_version: "2023-06-01".to_string(),
        client_config: Default::default(),
    };
    let anthropic_connector = AnthropicConnector::new(config).unwrap();
    let app = openai::router(Arc::new(anthropic_connector));

    // Send OpenAI streaming request (should be translated to Anthropic format)
    let openai_request = json!({
        "model": "claude-sonnet-4-5",
        "messages": [{
            "role": "user",
            "content": "Stream this!"
        }],
        "max_tokens": 50,
        "stream": true
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/chat/completions")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&openai_request).unwrap()))
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
