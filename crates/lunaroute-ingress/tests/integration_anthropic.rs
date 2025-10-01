//! Integration tests for Anthropic ingress adapter
//!
//! These tests verify the full HTTP request/response flow through the ingress layer.

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use futures::stream;
use lunaroute_core::{
    normalized::{
        Choice, FinishReason, Message, MessageContent, NormalizedRequest, NormalizedResponse,
        NormalizedStreamEvent, Role, Usage,
    },
    provider::{Provider, ProviderCapabilities},
};
use lunaroute_ingress::anthropic;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;

/// Mock provider that returns a fixed response
struct MockProvider {
    response: NormalizedResponse,
}

impl MockProvider {
    fn new(text: &str) -> Self {
        Self {
            response: NormalizedResponse {
                id: "test-id".to_string(),
                model: "claude-3-opus".to_string(),
                choices: vec![Choice {
                    index: 0,
                    message: Message {
                        role: Role::Assistant,
                        content: MessageContent::Text(text.to_string()),
                        name: None,
                        tool_calls: vec![],
                        tool_call_id: None,
                    },
                    finish_reason: Some(FinishReason::Stop),
                }],
                usage: Usage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    total_tokens: 15,
                },
                created: 1234567890,
                metadata: std::collections::HashMap::new(),
            },
        }
    }
}

#[async_trait]
impl Provider for MockProvider {
    async fn send(
        &self,
        _request: NormalizedRequest,
    ) -> lunaroute_core::Result<NormalizedResponse> {
        Ok(self.response.clone())
    }

    async fn stream(
        &self,
        _request: NormalizedRequest,
    ) -> lunaroute_core::Result<
        Box<
            dyn futures::Stream<Item = lunaroute_core::Result<NormalizedStreamEvent>>
                + Send
                + Unpin,
        >,
    > {
        Ok(Box::new(stream::empty()))
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_streaming: false,
            supports_tools: false,
            supports_vision: false,
        }
    }
}

/// Mock streaming provider that returns real stream events
struct StreamingMockProvider {
    events: Vec<NormalizedStreamEvent>,
}

impl StreamingMockProvider {
    fn new_text_stream() -> Self {
        use lunaroute_core::normalized::Delta;

        Self {
            events: vec![
                NormalizedStreamEvent::Start {
                    id: "msg-123".to_string(),
                    model: "claude-3-opus".to_string(),
                },
                NormalizedStreamEvent::Delta {
                    index: 0,
                    delta: Delta {
                        role: Some(Role::Assistant),
                        content: Some("Hello".to_string()),
                    },
                },
                NormalizedStreamEvent::Delta {
                    index: 0,
                    delta: Delta {
                        role: None,
                        content: Some(" from".to_string()),
                    },
                },
                NormalizedStreamEvent::Delta {
                    index: 0,
                    delta: Delta {
                        role: None,
                        content: Some(" Claude".to_string()),
                    },
                },
                NormalizedStreamEvent::Usage {
                    usage: Usage {
                        prompt_tokens: 12,
                        completion_tokens: 8,
                        total_tokens: 20,
                    },
                },
                NormalizedStreamEvent::End {
                    finish_reason: FinishReason::Stop,
                },
            ],
        }
    }
}

#[async_trait]
impl Provider for StreamingMockProvider {
    async fn send(
        &self,
        _request: NormalizedRequest,
    ) -> lunaroute_core::Result<NormalizedResponse> {
        Err(lunaroute_core::Error::Provider(
            "Streaming provider - use stream() instead".to_string(),
        ))
    }

    async fn stream(
        &self,
        _request: NormalizedRequest,
    ) -> lunaroute_core::Result<
        Box<
            dyn futures::Stream<Item = lunaroute_core::Result<NormalizedStreamEvent>>
                + Send
                + Unpin,
        >,
    > {
        let events = self.events.clone();
        Ok(Box::new(stream::iter(
            events.into_iter().map(Ok),
        )))
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_streaming: true,
            supports_tools: false,
            supports_vision: false,
        }
    }
}

/// Mock provider that returns an error
struct ErrorProvider;

#[async_trait]
impl Provider for ErrorProvider {
    async fn send(
        &self,
        _request: NormalizedRequest,
    ) -> lunaroute_core::Result<NormalizedResponse> {
        Err(lunaroute_core::Error::Provider(
            "Mock provider error".to_string(),
        ))
    }

    async fn stream(
        &self,
        _request: NormalizedRequest,
    ) -> lunaroute_core::Result<
        Box<
            dyn futures::Stream<Item = lunaroute_core::Result<NormalizedStreamEvent>>
                + Send
                + Unpin,
        >,
    > {
        Ok(Box::new(stream::empty()))
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_streaming: false,
            supports_tools: false,
            supports_vision: false,
        }
    }
}

#[tokio::test]
async fn test_messages_success() {
    let provider = Arc::new(MockProvider::new("Hello from Anthropic integration test!"));
    let app = anthropic::router(provider);

    let request = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "claude-3-opus",
                "messages": [
                    {"role": "user", "content": "Hello!"}
                ]
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["model"], "claude-3-opus");
    assert_eq!(json["type"], "message");
    assert_eq!(json["role"], "assistant");
    assert_eq!(json["content"][0]["text"], "Hello from Anthropic integration test!");
    assert_eq!(json["stop_reason"], "end_turn");
    assert_eq!(json["usage"]["input_tokens"], 10);
    assert_eq!(json["usage"]["output_tokens"], 5);
}

#[tokio::test]
async fn test_messages_with_system_prompt() {
    let provider = Arc::new(MockProvider::new("I am a helpful assistant"));
    let app = anthropic::router(provider);

    let request = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "claude-3-opus",
                "system": "You are a helpful assistant",
                "messages": [
                    {"role": "user", "content": "Hello!"}
                ]
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_messages_invalid_temperature() {
    let provider = Arc::new(MockProvider::new("Should not be called"));
    let app = anthropic::router(provider);

    // Anthropic temperature range is 0.0-1.0 (different from OpenAI)
    let request = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "claude-3-opus",
                "messages": [
                    {"role": "user", "content": "Hello!"}
                ],
                "temperature": 2.0  // Invalid for Anthropic: > 1.0
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(json["error"]["message"]
        .as_str()
        .unwrap()
        .contains("temperature must be between 0.0 and 1.0"));
}

#[tokio::test]
async fn test_messages_empty_messages() {
    let provider = Arc::new(MockProvider::new("Should not be called"));
    let app = anthropic::router(provider);

    let request = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "claude-3-opus",
                "messages": []
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(json["error"]["message"]
        .as_str()
        .unwrap()
        .contains("messages array cannot be empty"));
}

#[tokio::test]
async fn test_messages_provider_error() {
    let provider = Arc::new(ErrorProvider);
    let app = anthropic::router(provider);

    let request = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "claude-3-opus",
                "messages": [
                    {"role": "user", "content": "Hello!"}
                ]
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(json["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Mock provider error"));
}

#[tokio::test]
async fn test_messages_streaming_basic() {
    let provider = Arc::new(StreamingMockProvider::new_text_stream());
    let app = anthropic::router(provider);

    // Request with streaming enabled
    let request = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "claude-3-opus",
                "messages": [
                    {"role": "user", "content": "Hello!"}
                ],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    // Streaming returns 200 OK with text/event-stream content type
    assert_eq!(response.status(), StatusCode::OK);

    // Check content type is SSE
    let content_type = response.headers().get("content-type").unwrap();
    assert!(content_type.to_str().unwrap().contains("text/event-stream"));
}

#[tokio::test]
async fn test_messages_streaming_content() {
    use futures::StreamExt;

    let provider = Arc::new(StreamingMockProvider::new_text_stream());
    let app = anthropic::router(provider);

    let request = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "claude-3-opus",
                "messages": [
                    {"role": "user", "content": "Hello!"}
                ],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Read and parse SSE events
    let body = response.into_body();
    let mut stream = body.into_data_stream();
    let mut events = Vec::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.unwrap();
        let text = String::from_utf8(chunk.to_vec()).unwrap();

        // Parse SSE events (format: "data: {json}\n\n")
        for line in text.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                let json: serde_json::Value = serde_json::from_str(data).unwrap();
                events.push(json);
            }
        }
    }

    // Verify we got events
    assert!(!events.is_empty(), "Should have received stream events");

    // Verify first event is message_start
    let first = &events[0];
    assert_eq!(first["type"], "message_start");
    assert_eq!(first["message"]["role"], "assistant");
    assert_eq!(first["message"]["model"], "claude-3-opus");

    // Verify we have content_block_start
    let has_content_start = events
        .iter()
        .any(|e| e["type"] == "content_block_start");
    assert!(has_content_start, "Should have content_block_start event");

    // Verify we have content_block_delta events
    let content_deltas: Vec<_> = events
        .iter()
        .filter(|e| e["type"] == "content_block_delta")
        .collect();
    assert!(!content_deltas.is_empty(), "Should have content deltas");

    // Verify message_delta with stop_reason
    let message_delta = events
        .iter()
        .find(|e| e["type"] == "message_delta")
        .expect("Should have message_delta");
    assert_eq!(message_delta["delta"]["stop_reason"], "end_turn");

    // Verify last event is message_stop
    let last = events.last().unwrap();
    assert_eq!(last["type"], "message_stop");

    // Verify accumulated content
    let mut accumulated = String::new();
    for event in &events {
        if event["type"] == "content_block_delta" && event["delta"]["type"] == "text_delta"
            && let Some(text) = event["delta"]["text"].as_str()
        {
            accumulated.push_str(text);
        }
    }
    assert_eq!(accumulated, "Hello from Claude", "Content should be accumulated correctly");
}

#[tokio::test]
async fn test_messages_invalid_role() {
    let provider = Arc::new(MockProvider::new("Should not be called"));
    let app = anthropic::router(provider);

    // Anthropic only supports user and assistant roles (no system in messages)
    let request = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "claude-3-opus",
                "messages": [
                    {"role": "system", "content": "Invalid"}
                ]
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(json["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Invalid role"));
}

#[tokio::test]
async fn test_messages_malformed_json() {
    let provider = Arc::new(MockProvider::new("Should not be called"));
    let app = anthropic::router(provider);

    let request = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from("{invalid json"))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
