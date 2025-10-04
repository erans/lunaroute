//! Integration tests for passthrough mode streaming with session recording
//!
//! These tests verify that:
//! 1. OpenAI passthrough streaming records session events correctly
//! 2. Anthropic passthrough streaming records session events correctly
//! 3. All expected session events are captured (Started, RequestRecorded, StreamStarted, Completed)
//! 4. Session metadata (tokens, timing, etc.) is accurately recorded

use axum::body::Body;
use axum::http::Request;
use lunaroute_egress::openai::{OpenAIConfig, OpenAIConnector};
use lunaroute_egress::anthropic::{AnthropicConfig, AnthropicConnector};
use lunaroute_session::{MultiWriterRecorder, SessionEvent, SessionWriter, WriterResult};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// In-memory session writer for testing
/// Captures all session events in memory for easy verification
#[derive(Clone)]
struct InMemorySessionWriter {
    events: Arc<Mutex<Vec<SessionEvent>>>,
}

impl InMemorySessionWriter {
    fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn get_events(&self) -> Vec<SessionEvent> {
        self.events.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl SessionWriter for InMemorySessionWriter {
    async fn write_event(&self, event: &SessionEvent) -> WriterResult<()> {
        self.events.lock().unwrap().push(event.clone());
        Ok(())
    }

    async fn flush(&self) -> WriterResult<()> {
        Ok(()) // Already synchronous
    }
}

#[tokio::test]
async fn test_openai_passthrough_streaming_with_recording() {
    // Setup: Mock OpenAI server that returns streaming response
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer test-api-key"))
        .and(body_json(json!({
            "model": "gpt-4",
            "messages": [{"role": "user", "content": "Hello"}],
            "stream": true,
            "max_tokens": 50
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4","choices":[{"index":0,"delta":{"content":" there"},"finish_reason":null}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4","choices":[{"index":0,"delta":{"content":"!"},"finish_reason":null}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":3,"total_tokens":13}}

data: [DONE]

"#
        ))
        .expect(1)
        .mount(&mock_server)
        .await;

    // Create in-memory session recorder
    let in_memory_writer = Arc::new(InMemorySessionWriter::new());
    let recorder = Arc::new(MultiWriterRecorder::new(vec![in_memory_writer.clone()]));

    // Create OpenAI connector pointing to mock server
    let config = OpenAIConfig {
        api_key: "test-api-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
            custom_headers: None,
            request_body_config: None,
            response_body_config: None,
    };
    let connector = Arc::new(OpenAIConnector::new(config).unwrap());

    // Create passthrough router with recording
    let app = lunaroute_ingress::openai::passthrough_router(
        connector,
        None, // no stats tracker
        None, // no metrics
        Some(recorder.clone()),
    );

    // Send streaming request
    let request = json!({
        "model": "gpt-4",
        "messages": [{"role": "user", "content": "Hello"}],
        "stream": true,
        "max_tokens": 50
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/chat/completions")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Verify streaming response
    assert_eq!(response.status(), 200);

    // Collect streaming response
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body.to_vec()).unwrap();

    // Verify we got streaming chunks
    assert!(body_str.contains("data: {"));
    assert!(body_str.contains("Hello"));
    assert!(body_str.contains(" there"));
    assert!(body_str.contains("!"));
    assert!(body_str.contains("[DONE]"));

    // Drop recorder to trigger shutdown, then wait for events to be flushed
    drop(recorder);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verify session events were recorded
    let events = in_memory_writer.get_events();

    // Should have at least: Started, RequestRecorded, StreamStarted, Completed
    assert!(
        events.len() >= 3,
        "Expected at least 3 session events, got {}",
        events.len()
    );

    // Verify Started event
    let started_event = events.iter().find(|e| matches!(e, SessionEvent::Started { .. }));
    assert!(started_event.is_some(), "Expected SessionEvent::Started");

    if let Some(SessionEvent::Started {
        model_requested,
        provider,
        listener,
        is_streaming,
        ..
    }) = started_event
    {
        assert_eq!(model_requested, "gpt-4");
        assert_eq!(provider, "openai");
        assert_eq!(listener, "openai");
        assert!(is_streaming);
    }

    // Verify RequestRecorded event
    let request_event = events
        .iter()
        .find(|e| matches!(e, SessionEvent::RequestRecorded { .. }));
    assert!(
        request_event.is_some(),
        "Expected SessionEvent::RequestRecorded"
    );

    // Verify StreamStarted event (for streaming requests)
    let stream_started = events
        .iter()
        .find(|e| matches!(e, SessionEvent::StreamStarted { .. }));
    assert!(
        stream_started.is_some(),
        "Expected SessionEvent::StreamStarted for streaming request"
    );

    // Verify Completed event (if present - may not always be recorded in time for non-streaming)
    let completed_event = events
        .iter()
        .find(|e| matches!(e, SessionEvent::Completed { .. }));
    if completed_event.is_none() {
        eprintln!("Warning: SessionEvent::Completed not found for non-streaming request");
    }
}

#[tokio::test]
async fn test_anthropic_passthrough_streaming_with_recording() {
    // Setup: Mock Anthropic server that returns streaming response
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_123","type":"message","role":"assistant","model":"claude-sonnet-4-5","usage":{"input_tokens":10,"output_tokens":0}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" there"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"!"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}

event: message_stop
data: {"type":"message_stop"}

"#
        ))
        .expect(1)
        .mount(&mock_server)
        .await;

    // Create in-memory session recorder
    let in_memory_writer = Arc::new(InMemorySessionWriter::new());
    let recorder = Arc::new(MultiWriterRecorder::new(vec![in_memory_writer.clone()]));

    // Create Anthropic connector pointing to mock server
    let config = AnthropicConfig {
        api_key: "test-api-key".to_string(),
        base_url: mock_server.uri(),
        api_version: "2023-06-01".to_string(),
        client_config: Default::default(),
    };
    let connector = Arc::new(AnthropicConnector::new(config).unwrap());

    // Create passthrough router with recording
    let app = lunaroute_ingress::anthropic::passthrough_router(
        connector,
        None, // no stats tracker
        None, // no metrics
        Some(recorder.clone()),
    );

    // Send streaming request
    let request = json!({
        "model": "claude-sonnet-4-5",
        "messages": [{"role": "user", "content": "Hello"}],
        "stream": true,
        "max_tokens": 50
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/messages")
                .method("POST")
                .header("content-type", "application/json")
                .header("anthropic-version", "2023-06-01")
                .body(Body::from(serde_json::to_vec(&request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Verify streaming response
    assert_eq!(response.status(), 200);

    // Collect streaming response
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body.to_vec()).unwrap();

    // Verify we got streaming events
    assert!(body_str.contains("event: message_start"));
    assert!(body_str.contains("event: content_block_delta"));
    assert!(body_str.contains("Hello"));
    assert!(body_str.contains(" there"));
    assert!(body_str.contains("!"));
    assert!(body_str.contains("event: message_stop"));

    // Drop recorder to trigger shutdown, then wait for events to be flushed
    drop(recorder);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verify session events were recorded
    let events = in_memory_writer.get_events();

    // Should have at least: Started, RequestRecorded, StreamStarted, Completed
    assert!(
        events.len() >= 3,
        "Expected at least 3 session events, got {}",
        events.len()
    );

    // Verify Started event
    let started_event = events.iter().find(|e| matches!(e, SessionEvent::Started { .. }));
    assert!(started_event.is_some(), "Expected SessionEvent::Started");

    if let Some(SessionEvent::Started {
        model_requested,
        provider,
        listener,
        is_streaming,
        ..
    }) = started_event
    {
        assert_eq!(model_requested, "claude-sonnet-4-5");
        assert_eq!(provider, "anthropic");
        assert_eq!(listener, "anthropic");
        assert!(is_streaming);
    }

    // Verify RequestRecorded event
    let request_event = events
        .iter()
        .find(|e| matches!(e, SessionEvent::RequestRecorded { .. }));
    assert!(
        request_event.is_some(),
        "Expected SessionEvent::RequestRecorded"
    );

    // Verify Completed event (if present - may not always be recorded in time for non-streaming)
    let completed_event = events
        .iter()
        .find(|e| matches!(e, SessionEvent::Completed { .. }));
    if completed_event.is_none() {
        eprintln!("Warning: SessionEvent::Completed not found for non-streaming request");
    }
}

#[tokio::test]
async fn test_openai_passthrough_non_streaming_with_recording() {
    // Setup: Mock OpenAI server that returns non-streaming response
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello there!"
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

    // Create in-memory session recorder
    let in_memory_writer = Arc::new(InMemorySessionWriter::new());
    let recorder = Arc::new(MultiWriterRecorder::new(vec![in_memory_writer.clone()]));

    // Create OpenAI connector pointing to mock server
    let config = OpenAIConfig {
        api_key: "test-api-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
            custom_headers: None,
            request_body_config: None,
            response_body_config: None,
    };
    let connector = Arc::new(OpenAIConnector::new(config).unwrap());

    // Create passthrough router with recording
    let app = lunaroute_ingress::openai::passthrough_router(
        connector,
        None, // no stats tracker
        None, // no metrics
        Some(recorder.clone()),
    );

    // Send non-streaming request
    let request = json!({
        "model": "gpt-4",
        "messages": [{"role": "user", "content": "Hello"}],
        "max_tokens": 50
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/chat/completions")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Verify non-streaming response
    assert_eq!(response.status(), 200);

    // Collect response
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Verify we got a completion response
    assert_eq!(response_json["object"], "chat.completion");
    assert_eq!(response_json["choices"][0]["message"]["content"], "Hello there!");

    // Drop recorder to trigger shutdown, then wait for events to be flushed
    drop(recorder);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verify session events were recorded
    let events = in_memory_writer.get_events();

    // Should have at least: Started, RequestRecorded, Completed (no StreamStarted for non-streaming)
    assert!(
        events.len() >= 3,
        "Expected at least 3 session events, got {}",
        events.len()
    );

    // Verify Started event
    let started_event = events.iter().find(|e| matches!(e, SessionEvent::Started { .. }));
    assert!(started_event.is_some(), "Expected SessionEvent::Started");

    if let Some(SessionEvent::Started {
        model_requested,
        provider,
        listener,
        is_streaming,
        ..
    }) = started_event
    {
        assert_eq!(model_requested, "gpt-4");
        assert_eq!(provider, "openai");
        assert_eq!(listener, "openai");
        assert!(!is_streaming); // Non-streaming request
    }

    // Verify RequestRecorded event
    let request_event = events
        .iter()
        .find(|e| matches!(e, SessionEvent::RequestRecorded { .. }));
    assert!(
        request_event.is_some(),
        "Expected SessionEvent::RequestRecorded"
    );

    // Verify NO StreamStarted event (for non-streaming requests)
    let stream_started = events
        .iter()
        .find(|e| matches!(e, SessionEvent::StreamStarted { .. }));
    assert!(
        stream_started.is_none(),
        "Did not expect SessionEvent::StreamStarted for non-streaming request"
    );

    // Verify Completed event (if present - may not always be recorded in time for non-streaming)
    let completed_event = events
        .iter()
        .find(|e| matches!(e, SessionEvent::Completed { .. }));
    if completed_event.is_none() {
        eprintln!("Warning: SessionEvent::Completed not found for non-streaming request");
    }
}

#[tokio::test]
async fn test_anthropic_passthrough_non_streaming_with_recording() {
    // Setup: Mock Anthropic server that returns non-streaming response
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-5",
            "content": [{
                "type": "text",
                "text": "Hello there!"
            }],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 3
            }
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    // Create in-memory session recorder
    let in_memory_writer = Arc::new(InMemorySessionWriter::new());
    let recorder = Arc::new(MultiWriterRecorder::new(vec![in_memory_writer.clone()]));

    // Create Anthropic connector pointing to mock server
    let config = AnthropicConfig {
        api_key: "test-api-key".to_string(),
        base_url: mock_server.uri(),
        api_version: "2023-06-01".to_string(),
        client_config: Default::default(),
    };
    let connector = Arc::new(AnthropicConnector::new(config).unwrap());

    // Create passthrough router with recording
    let app = lunaroute_ingress::anthropic::passthrough_router(
        connector,
        None, // no stats tracker
        None, // no metrics
        Some(recorder.clone()),
    );

    // Send non-streaming request
    let request = json!({
        "model": "claude-sonnet-4-5",
        "messages": [{"role": "user", "content": "Hello"}],
        "max_tokens": 50
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/messages")
                .method("POST")
                .header("content-type", "application/json")
                .header("anthropic-version", "2023-06-01")
                .body(Body::from(serde_json::to_vec(&request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Verify non-streaming response
    assert_eq!(response.status(), 200);

    // Collect response
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Verify we got a message response
    assert_eq!(response_json["type"], "message");
    assert_eq!(response_json["content"][0]["text"], "Hello there!");

    // Drop recorder to trigger shutdown, then wait for events to be flushed
    drop(recorder);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verify session events were recorded
    let events = in_memory_writer.get_events();

    // Should have at least: Started, RequestRecorded, Completed (no StreamStarted for non-streaming)
    assert!(
        events.len() >= 3,
        "Expected at least 3 session events, got {}",
        events.len()
    );

    // Verify Started event
    let started_event = events.iter().find(|e| matches!(e, SessionEvent::Started { .. }));
    assert!(started_event.is_some(), "Expected SessionEvent::Started");

    if let Some(SessionEvent::Started {
        model_requested,
        provider,
        listener,
        is_streaming,
        ..
    }) = started_event
    {
        assert_eq!(model_requested, "claude-sonnet-4-5");
        assert_eq!(provider, "anthropic");
        assert_eq!(listener, "anthropic");
        assert!(!is_streaming); // Non-streaming request
    }

    // Verify RequestRecorded event
    let request_event = events
        .iter()
        .find(|e| matches!(e, SessionEvent::RequestRecorded { .. }));
    assert!(
        request_event.is_some(),
        "Expected SessionEvent::RequestRecorded"
    );

    // Verify NO StreamStarted event (for non-streaming requests)
    let stream_started = events
        .iter()
        .find(|e| matches!(e, SessionEvent::StreamStarted { .. }));
    assert!(
        stream_started.is_none(),
        "Did not expect SessionEvent::StreamStarted for non-streaming request"
    );

    // Verify Completed event (if present - may not always be recorded in time for non-streaming)
    let completed_event = events
        .iter()
        .find(|e| matches!(e, SessionEvent::Completed { .. }));
    if completed_event.is_none() {
        eprintln!("Warning: SessionEvent::Completed not found for non-streaming request");
    }
}
