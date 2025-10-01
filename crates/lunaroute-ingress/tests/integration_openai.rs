//! Integration tests for OpenAI ingress adapter
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
use lunaroute_ingress::openai;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt; // for `oneshot`

/// Mock provider that returns a fixed response
struct MockProvider {
    response: NormalizedResponse,
}

impl MockProvider {
    fn new(text: &str) -> Self {
        Self {
            response: NormalizedResponse {
                id: "test-id".to_string(),
                model: "gpt-4".to_string(),
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
async fn test_chat_completions_success() {
    // Create app with mock provider
    let provider = Arc::new(MockProvider::new("Hello from integration test!"));
    let app = openai::router(provider);

    // Create request
    let request = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "gpt-4",
                "messages": [
                    {"role": "user", "content": "Hello!"}
                ]
            })
            .to_string(),
        ))
        .unwrap();

    // Send request
    let response = app.oneshot(request).await.unwrap();

    // Verify response
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["model"], "gpt-4");
    assert_eq!(json["choices"][0]["message"]["content"], "Hello from integration test!");
    assert_eq!(json["choices"][0]["message"]["role"], "assistant");
    assert_eq!(json["choices"][0]["finish_reason"], "stop");
    assert_eq!(json["usage"]["total_tokens"], 15);
}

#[tokio::test]
async fn test_chat_completions_invalid_request() {
    let provider = Arc::new(MockProvider::new("Should not be called"));
    let app = openai::router(provider);

    // Request with invalid temperature
    let request = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "gpt-4",
                "messages": [
                    {"role": "user", "content": "Hello!"}
                ],
                "temperature": 5.0  // Invalid: > 2.0
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
        .contains("temperature"));
}

#[tokio::test]
async fn test_chat_completions_empty_messages() {
    let provider = Arc::new(MockProvider::new("Should not be called"));
    let app = openai::router(provider);

    // Request with empty messages array
    let request = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "gpt-4",
                "messages": []  // Invalid: empty
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
async fn test_chat_completions_provider_error() {
    let provider = Arc::new(ErrorProvider);
    let app = openai::router(provider);

    let request = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "gpt-4",
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
async fn test_chat_completions_streaming_basic() {
    let provider = Arc::new(MockProvider::new("Hello from stream"));
    let app = openai::router(provider);

    // Request with streaming enabled
    let request = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "gpt-4",
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
async fn test_chat_completions_with_tools() {
    let provider = Arc::new(MockProvider::new("Let me check the weather"));
    let app = openai::router(provider);

    let request = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "gpt-4",
                "messages": [
                    {"role": "user", "content": "What's the weather?"}
                ],
                "tools": [
                    {
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "description": "Get weather info",
                            "parameters": {
                                "type": "object",
                                "properties": {
                                    "location": {"type": "string"}
                                }
                            }
                        }
                    }
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

    assert_eq!(json["choices"][0]["message"]["content"], "Let me check the weather");
}

#[tokio::test]
async fn test_chat_completions_malformed_json() {
    let provider = Arc::new(MockProvider::new("Should not be called"));
    let app = openai::router(provider);

    let request = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from("{invalid json"))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
