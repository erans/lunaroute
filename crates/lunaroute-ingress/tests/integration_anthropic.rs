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
async fn test_messages_streaming_not_supported() {
    let provider = Arc::new(MockProvider::new("Should not be called"));
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

    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(json["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Streaming not yet implemented"));
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
