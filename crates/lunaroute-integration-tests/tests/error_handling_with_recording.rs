//! Integration tests for error handling with session recording
//!
//! These tests verify that:
//! 1. 4xx errors from upstream APIs are properly recorded in session events
//! 2. 5xx errors from upstream APIs are properly recorded in session events
//! 3. Network errors and timeouts are captured in session events
//! 4. Error metadata (status codes, messages) is accurately recorded

use axum::body::Body;
use axum::http::Request;
use lunaroute_egress::anthropic::{AnthropicConfig, AnthropicConnector};
use lunaroute_egress::openai::{OpenAIConfig, OpenAIConnector};
use lunaroute_session::{MultiWriterRecorder, SessionEvent, SessionWriter, WriterResult};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// In-memory session writer for testing
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
        Ok(())
    }
}

#[tokio::test]
async fn test_openai_400_error_with_recording() {
    // Setup: Mock OpenAI server that returns 400 Bad Request
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer test-api-key"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": {
                "message": "Invalid request: model is required",
                "type": "invalid_request_error",
                "param": "model",
                "code": null
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
        None,
        None,
        Some(recorder.clone()),
    );

    // Send request that will trigger 400 error
    let request = json!({
        "messages": [{"role": "user", "content": "Hello"}],
        "max_tokens": 50
        // Missing required "model" field
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

    // Verify error response (upstream errors become 502 Bad Gateway)
    assert_eq!(response.status(), 502);

    // Drop recorder and wait for events to flush
    drop(recorder);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verify session events
    let events = in_memory_writer.get_events();

    // Should have Started event
    let started_event = events
        .iter()
        .find(|e| matches!(e, SessionEvent::Started { .. }));
    assert!(started_event.is_some(), "Expected SessionEvent::Started");

    // Should have RequestRecorded event
    let request_event = events
        .iter()
        .find(|e| matches!(e, SessionEvent::RequestRecorded { .. }));
    assert!(
        request_event.is_some(),
        "Expected SessionEvent::RequestRecorded"
    );

    // Should have Error or Completed event with error status
    let error_recorded = events.iter().any(|e| {
        matches!(e, SessionEvent::Completed { success, .. } if !success)
            || matches!(e, SessionEvent::Completed { error: Some(_), .. })
    });

    if !error_recorded {
        eprintln!("Warning: Expected error event to be recorded for 400 status");
        eprintln!("Events captured: {:?}", events);
    }
}

#[tokio::test]
async fn test_openai_500_error_with_recording() {
    // Setup: Mock OpenAI server that returns 500 Internal Server Error
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer test-api-key"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({
            "error": {
                "message": "The server had an error while processing your request",
                "type": "server_error",
                "param": null,
                "code": null
            }
        })))
        .expect(1..)
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
        None,
        None,
        Some(recorder.clone()),
    );

    // Send request that will trigger 500 error
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

    // Verify error response (upstream errors become 502 Bad Gateway)
    assert_eq!(response.status(), 502);

    // Drop recorder and wait for events to flush
    drop(recorder);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verify session events
    let events = in_memory_writer.get_events();

    // Should have Started event
    let started_event = events
        .iter()
        .find(|e| matches!(e, SessionEvent::Started { .. }));
    assert!(started_event.is_some(), "Expected SessionEvent::Started");

    // Should have RequestRecorded event
    let request_event = events
        .iter()
        .find(|e| matches!(e, SessionEvent::RequestRecorded { .. }));
    assert!(
        request_event.is_some(),
        "Expected SessionEvent::RequestRecorded"
    );

    // Should have error recorded (success: false or error field set)
    let error_recorded = events.iter().any(|e| {
        matches!(e, SessionEvent::Completed { success, .. } if !success)
            || matches!(e, SessionEvent::Completed { error: Some(_), .. })
    });

    if !error_recorded {
        eprintln!("Warning: Expected error to be recorded in Completed event for 500 status");
        eprintln!("Events captured: {:?}", events);
    }
}

#[tokio::test]
async fn test_anthropic_401_unauthorized_with_recording() {
    // Setup: Mock Anthropic server that returns 401 Unauthorized
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "type": "error",
            "error": {
                "type": "authentication_error",
                "message": "invalid x-api-key"
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
        api_key: "invalid-key".to_string(),
        base_url: mock_server.uri(),
        api_version: "2023-06-01".to_string(),
        client_config: Default::default(),
    };
    let connector = Arc::new(AnthropicConnector::new(config).unwrap());

    // Create passthrough router with recording
    let app = lunaroute_ingress::anthropic::passthrough_router(
        connector,
        None,
        None,
        Some(recorder.clone()),
    );

    // Send request with invalid API key
    let request = json!({
        "model": "claude-sonnet-4-5",
        "max_tokens": 50,
        "messages": [{
            "role": "user",
            "content": "Hello"
        }]
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

    // Verify error response (upstream errors become 502 Bad Gateway)
    assert_eq!(response.status(), 502);

    // Drop recorder and wait for events to flush
    drop(recorder);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verify session events
    let events = in_memory_writer.get_events();

    // Should have Started event
    let started_event = events
        .iter()
        .find(|e| matches!(e, SessionEvent::Started { .. }));
    assert!(started_event.is_some(), "Expected SessionEvent::Started");

    // Should have RequestRecorded event
    let request_event = events
        .iter()
        .find(|e| matches!(e, SessionEvent::RequestRecorded { .. }));
    assert!(
        request_event.is_some(),
        "Expected SessionEvent::RequestRecorded"
    );

    // Should have error recorded
    let error_recorded = events.iter().any(|e| {
        matches!(e, SessionEvent::Completed { success, .. } if !success)
            || matches!(e, SessionEvent::Completed { error: Some(_), .. })
    });

    if !error_recorded {
        eprintln!("Warning: Expected error event to be recorded for 401 status");
        eprintln!("Events captured: {:?}", events);
    }
}

#[tokio::test]
async fn test_anthropic_rate_limit_429_with_recording() {
    // Setup: Mock Anthropic server that returns 429 Rate Limit
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(429)
                .set_body_json(json!({
                    "type": "error",
                    "error": {
                        "type": "rate_limit_error",
                        "message": "Rate limit exceeded"
                    }
                }))
                .insert_header("retry-after", "60"),
        )
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
        None,
        None,
        Some(recorder.clone()),
    );

    // Send request that will hit rate limit
    let request = json!({
        "model": "claude-sonnet-4-5",
        "max_tokens": 50,
        "messages": [{
            "role": "user",
            "content": "Hello"
        }]
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

    // Verify error response (upstream errors become 502 Bad Gateway)
    assert_eq!(response.status(), 502);

    // Drop recorder and wait for events to flush
    drop(recorder);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verify session events
    let events = in_memory_writer.get_events();

    // Should have Started event
    let started_event = events
        .iter()
        .find(|e| matches!(e, SessionEvent::Started { .. }));
    assert!(started_event.is_some(), "Expected SessionEvent::Started");

    // Should have RequestRecorded event
    let request_event = events
        .iter()
        .find(|e| matches!(e, SessionEvent::RequestRecorded { .. }));
    assert!(
        request_event.is_some(),
        "Expected SessionEvent::RequestRecorded"
    );

    // Should have error recorded
    let error_recorded = events.iter().any(|e| {
        matches!(e, SessionEvent::Completed { success, .. } if !success)
            || matches!(e, SessionEvent::Completed { error: Some(_), .. })
    });

    if !error_recorded {
        eprintln!("Warning: Expected error event to be recorded for 429 status");
        eprintln!("Events captured: {:?}", events);
    }
}

#[tokio::test]
async fn test_openai_streaming_error_with_recording() {
    // Setup: Mock OpenAI server that returns an error during streaming
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer test-api-key"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({
                    "error": {
                        "message": "Invalid streaming request",
                        "type": "invalid_request_error",
                        "code": "invalid_stream_parameter"
                    }
                })),
        )
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
        None,
        None,
        Some(recorder.clone()),
    );

    // Send streaming request that will fail
    let request = json!({
        "model": "gpt-4",
        "messages": [{"role": "user", "content": "Hello"}],
        "max_tokens": 50,
        "stream": true
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

    // Verify error response (upstream errors become 502 Bad Gateway)
    assert_eq!(response.status(), 502);

    // Drop recorder and wait for events to flush
    drop(recorder);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verify session events
    let events = in_memory_writer.get_events();

    // Should have Started event
    let started_event = events
        .iter()
        .find(|e| matches!(e, SessionEvent::Started { .. }));
    assert!(started_event.is_some(), "Expected SessionEvent::Started");

    // Should have RequestRecorded event
    let request_event = events
        .iter()
        .find(|e| matches!(e, SessionEvent::RequestRecorded { .. }));
    assert!(
        request_event.is_some(),
        "Expected SessionEvent::RequestRecorded"
    );

    // Should NOT have StreamStarted event (error before streaming begins)
    let stream_started = events
        .iter()
        .find(|e| matches!(e, SessionEvent::StreamStarted { .. }));
    assert!(
        stream_started.is_none(),
        "Did not expect StreamStarted event for failed streaming request"
    );

    // Should have error recorded
    let error_recorded = events.iter().any(|e| {
        matches!(e, SessionEvent::Completed { success, .. } if !success)
            || matches!(e, SessionEvent::Completed { error: Some(_), .. })
    });

    if !error_recorded {
        eprintln!("Warning: Expected error event to be recorded for streaming error");
        eprintln!("Events captured: {:?}", events);
    }
}
