//! Integration tests for OpenAI Responses API WebSocket ingress.
//!
//! Verifies:
//! 1. A `response.create` frame drives the HTTP pipeline and streams events back.
//! 2. Session events (Started, RequestRecorded, Completed) are recorded.
//! 3. Multiple `response.create` frames on one connection run sequentially.
//! 4. An unsupported event type returns a structured error frame.

mod common;

use common::InMemorySessionStore;
use futures::{SinkExt, StreamExt};
use lunaroute_egress::openai::{OpenAIConfig, OpenAIConnector};
use lunaroute_session::SessionEvent;
use serde_json::json;
use std::sync::Arc;
use tokio_tungstenite::tungstenite::Message;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Bind the passthrough router to a random localhost port and return the port.
/// The task runs `axum::serve` on a `tokio::net::TcpListener`.
async fn spawn_passthrough(
    connector: Arc<OpenAIConnector>,
    store: Arc<InMemorySessionStore>,
) -> u16 {
    let app = lunaroute_ingress::openai::passthrough_router(
        connector,
        None,
        None,
        Some(store),
        15,
        true,
        None,
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    // Give the server a beat to start accepting.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    port
}

/// Build a minimal Responses API SSE body: created → output_text.delta → completed.
fn upstream_sse_body() -> String {
    [
        r#"{"type":"response.created","response":{"id":"resp_1","model":"gpt-5"}}"#,
        r#"{"type":"response.output_text.delta","delta":"hi"}"#,
        r#"{"type":"response.completed","response":{"id":"resp_1","usage":{"input_tokens":5,"output_tokens":1,"total_tokens":6}}}"#,
    ]
    .iter()
    .map(|e| format!("data: {e}\n\n"))
    .collect::<String>()
}

/// Build and connect a WebSocket client to the given URL, returning the stream/sink.
async fn ws_connect(
    url: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let (ws, _resp) = tokio_tungstenite::connect_async(url).await.unwrap();
    ws
}

#[tokio::test]
async fn ws_response_create_streams_events_and_records_session() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_string(upstream_sse_body()))
        .expect(1)
        .mount(&mock_server)
        .await;

    let store = Arc::new(InMemorySessionStore::new());

    let config = OpenAIConfig {
        api_key: "test-api-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let connector = Arc::new(OpenAIConnector::new(config).await.unwrap());

    let port = spawn_passthrough(connector, store.clone()).await;
    let url = format!("ws://127.0.0.1:{port}/v1/responses");
    let mut ws = ws_connect(&url).await;

    // Send a response.create frame.
    let create = json!({
        "type": "response.create",
        "response": {
            "model": "gpt-5",
            "input": "hello"
        }
    });
    ws.send(Message::Text(create.to_string().into()))
        .await
        .unwrap();

    // Collect frames until `response.completed` arrives.
    let mut seen_types: Vec<String> = Vec::new();
    while let Some(frame) = ws.next().await {
        let msg = frame.unwrap();
        let text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Close(_) => break,
            _ => continue,
        };
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let ty = value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        seen_types.push(ty.clone());
        if ty == "response.completed" {
            break;
        }
    }

    assert_eq!(
        seen_types,
        vec![
            "response.created",
            "response.output_text.delta",
            "response.completed"
        ],
        "unexpected event order; got {seen_types:?}"
    );

    // Give async session writers a moment to flush.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let events = store.get_events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SessionEvent::Started { .. })),
        "expected Started event; got {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SessionEvent::RequestRecorded { .. })),
        "expected RequestRecorded event; got {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SessionEvent::Completed { .. })),
        "expected Completed event; got {events:?}"
    );

    ws.close(None).await.ok();
}

#[tokio::test]
async fn ws_runs_two_response_creates_sequentially_on_one_connection() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_string(upstream_sse_body()))
        .expect(2)
        .mount(&mock_server)
        .await;

    let store = Arc::new(InMemorySessionStore::new());

    let config = OpenAIConfig {
        api_key: "test-api-key".to_string(),
        base_url: mock_server.uri(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let connector = Arc::new(OpenAIConnector::new(config).await.unwrap());

    let port = spawn_passthrough(connector, store.clone()).await;
    let url = format!("ws://127.0.0.1:{port}/v1/responses");
    let mut ws = ws_connect(&url).await;

    for _ in 0..2 {
        let create = json!({
            "type": "response.create",
            "response": { "model": "gpt-5", "input": "hi" }
        });
        ws.send(Message::Text(create.to_string().into()))
            .await
            .unwrap();

        // Drain frames until `response.completed`.
        loop {
            let msg = ws.next().await.unwrap().unwrap();
            let text = match msg {
                Message::Text(t) => t.to_string(),
                Message::Close(_) => {
                    panic!("server closed connection unexpectedly mid-test");
                }
                _ => continue,
            };
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            if value.get("type").and_then(|v| v.as_str()) == Some("response.completed") {
                break;
            }
        }
    }

    ws.close(None).await.ok();

    // Upstream must have received exactly 2 POSTs; wiremock's `.expect(2)` asserts
    // this implicitly on drop when the mock_server is dropped at end of scope.
}

#[tokio::test]
async fn ws_sends_error_frame_for_unsupported_event_type() {
    let mock_server = MockServer::start().await;

    // Mount a mock but we don't expect it to be called (response.cancel is rejected
    // before hitting the upstream).
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_string(upstream_sse_body()))
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
        switch_notification_message: None,
    };
    let connector = Arc::new(OpenAIConnector::new(config).await.unwrap());
    let store = Arc::new(InMemorySessionStore::new());
    let port = spawn_passthrough(connector, store).await;

    let url = format!("ws://127.0.0.1:{port}/v1/responses");
    let mut ws = ws_connect(&url).await;

    // Send an unsupported event type.
    ws.send(Message::Text(r#"{"type":"response.cancel"}"#.into()))
        .await
        .unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    let text = match msg {
        Message::Text(t) => t.to_string(),
        other => panic!("expected text frame, got {other:?}"),
    };
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        value.get("type").and_then(|v| v.as_str()),
        Some("error"),
        "expected type=error; got {value}"
    );
    assert_eq!(
        value
            .get("error")
            .and_then(|e| e.get("code"))
            .and_then(|v| v.as_str()),
        Some("unsupported_event_type"),
        "expected code=unsupported_event_type; got {value}"
    );

    ws.close(None).await.ok();
}
