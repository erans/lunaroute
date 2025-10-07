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
                    id: "stream-123".to_string(),
                    model: "gpt-4".to_string(),
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
                        content: Some(" world".to_string()),
                    },
                },
                NormalizedStreamEvent::Delta {
                    index: 0,
                    delta: Delta {
                        role: None,
                        content: Some("!".to_string()),
                    },
                },
                NormalizedStreamEvent::Usage {
                    usage: Usage {
                        prompt_tokens: 10,
                        completion_tokens: 15,
                        total_tokens: 25,
                    },
                },
                NormalizedStreamEvent::End {
                    finish_reason: FinishReason::Stop,
                },
            ],
        }
    }

    fn new_tool_call_stream() -> Self {
        use lunaroute_core::normalized::FunctionCallDelta;

        Self {
            events: vec![
                NormalizedStreamEvent::Start {
                    id: "stream-456".to_string(),
                    model: "gpt-4".to_string(),
                },
                NormalizedStreamEvent::ToolCallDelta {
                    index: 0,
                    tool_call_index: 0,
                    id: Some("call_abc123".to_string()),
                    function: Some(FunctionCallDelta {
                        name: Some("get_weather".to_string()),
                        arguments: None,
                    }),
                },
                NormalizedStreamEvent::ToolCallDelta {
                    index: 0,
                    tool_call_index: 0,
                    id: None,
                    function: Some(FunctionCallDelta {
                        name: None,
                        arguments: Some("{\"location\"".to_string()),
                    }),
                },
                NormalizedStreamEvent::ToolCallDelta {
                    index: 0,
                    tool_call_index: 0,
                    id: None,
                    function: Some(FunctionCallDelta {
                        name: None,
                        arguments: Some(": \"San Francisco\"}".to_string()),
                    }),
                },
                NormalizedStreamEvent::End {
                    finish_reason: FinishReason::ToolCalls,
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
        Ok(Box::new(stream::iter(events.into_iter().map(Ok))))
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

/// Mock streaming provider that returns an error in the stream
struct StreamingErrorProvider;

#[async_trait]
impl Provider for StreamingErrorProvider {
    async fn send(
        &self,
        _request: NormalizedRequest,
    ) -> lunaroute_core::Result<NormalizedResponse> {
        Err(lunaroute_core::Error::Provider(
            "Use stream() instead".to_string(),
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
        use lunaroute_core::normalized::Delta;

        // Return a stream that has some events then an error
        let events: Vec<lunaroute_core::Result<NormalizedStreamEvent>> = vec![
            Ok(NormalizedStreamEvent::Start {
                id: "stream-err".to_string(),
                model: "gpt-4".to_string(),
            }),
            Ok(NormalizedStreamEvent::Delta {
                index: 0,
                delta: Delta {
                    role: Some(Role::Assistant),
                    content: Some("Starting to respond...".to_string()),
                },
            }),
            Err(lunaroute_core::Error::Provider(
                "Connection lost midstream".to_string(),
            )),
        ];

        Ok(Box::new(stream::iter(events)))
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_streaming: true,
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
    assert_eq!(
        json["choices"][0]["message"]["content"],
        "Hello from integration test!"
    );
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

    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("temperature")
    );
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

    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("messages array cannot be empty")
    );
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

    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Mock provider error")
    );
}

#[tokio::test]
async fn test_chat_completions_streaming_basic() {
    let provider = Arc::new(StreamingMockProvider::new_text_stream());
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
async fn test_chat_completions_streaming_content() {
    use futures::StreamExt;

    let provider = Arc::new(StreamingMockProvider::new_text_stream());
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
            if let Some(data) = line.strip_prefix("data: ")
                && data != "[DONE]"
            {
                let json: serde_json::Value = serde_json::from_str(data).unwrap();
                events.push(json);
            }
        }
    }

    // Verify we got events
    assert!(!events.is_empty(), "Should have received stream events");

    // Verify first event has role
    let first = &events[0];
    assert_eq!(first["object"], "chat.completion.chunk");
    assert_eq!(first["choices"][0]["delta"]["role"], "assistant");

    // Verify we got content deltas
    let content_events: Vec<_> = events
        .iter()
        .filter(|e| e["choices"][0]["delta"]["content"].is_string())
        .collect();
    assert!(!content_events.is_empty(), "Should have content deltas");

    // Verify final event has finish_reason
    let last = events.last().unwrap();
    assert_eq!(last["choices"][0]["finish_reason"], "stop");

    // Verify accumulated content
    let mut accumulated = String::new();
    for event in &events {
        if let Some(content) = event["choices"][0]["delta"]["content"].as_str() {
            accumulated.push_str(content);
        }
    }
    assert_eq!(
        accumulated, "Hello world!",
        "Content should be accumulated correctly"
    );
}

#[tokio::test]
async fn test_chat_completions_streaming_tool_calls() {
    use futures::StreamExt;

    let provider = Arc::new(StreamingMockProvider::new_tool_call_stream());
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

        for line in text.lines() {
            if let Some(data) = line.strip_prefix("data: ")
                && data != "[DONE]"
            {
                let json: serde_json::Value = serde_json::from_str(data).unwrap();
                events.push(json);
            }
        }
    }

    // Verify we got events
    assert!(!events.is_empty(), "Should have received stream events");

    // Verify we have tool_calls in deltas
    let tool_events: Vec<_> = events
        .iter()
        .filter(|e| !e["choices"][0]["delta"]["tool_calls"].is_null())
        .collect();
    assert!(!tool_events.is_empty(), "Should have tool call events");

    // Verify tool call structure
    let first_tool = &tool_events[0]["choices"][0]["delta"]["tool_calls"][0];
    assert_eq!(first_tool["id"], "call_abc123");
    assert_eq!(first_tool["function"]["name"], "get_weather");

    // Verify finish_reason is tool_calls
    let last = events.last().unwrap();
    assert_eq!(last["choices"][0]["finish_reason"], "tool_calls");

    // Verify accumulated arguments
    let mut accumulated_args = String::new();
    for event in &events {
        if let Some(tool_calls) = event["choices"][0]["delta"]["tool_calls"].as_array() {
            for tool_call in tool_calls {
                if let Some(args) = tool_call["function"]["arguments"].as_str() {
                    accumulated_args.push_str(args);
                }
            }
        }
    }
    assert_eq!(accumulated_args, "{\"location\": \"San Francisco\"}");
}

#[tokio::test]
async fn test_chat_completions_streaming_error_handling() {
    use futures::StreamExt;

    let provider = Arc::new(StreamingErrorProvider);
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
    let mut had_error = false;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.unwrap();
        let text = String::from_utf8(chunk.to_vec()).unwrap();

        for line in text.lines() {
            if let Some(data) = line.strip_prefix("data: ")
                && data != "[DONE]"
            {
                let json: serde_json::Value = serde_json::from_str(data).unwrap();

                // Check if it's an error event
                if json.get("error").is_some() {
                    had_error = true;
                    assert!(
                        json["error"]["message"]
                            .as_str()
                            .unwrap()
                            .contains("Connection lost midstream")
                    );
                } else {
                    events.push(json);
                }
            }
        }
    }

    // Verify we got some events before the error
    assert!(
        !events.is_empty(),
        "Should have received events before error"
    );

    // Verify we got an error event
    assert!(had_error, "Should have received an error event in stream");

    // Verify we got at least one content delta before the error
    let has_content = events
        .iter()
        .any(|e| e["choices"][0]["delta"]["content"].is_string());
    assert!(has_content, "Should have gotten content before error");
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

    assert_eq!(
        json["choices"][0]["message"]["content"],
        "Let me check the weather"
    );
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
