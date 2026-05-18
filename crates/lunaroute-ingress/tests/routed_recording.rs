use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use futures::stream;
use lunaroute_core::{
    error::Error as CoreError,
    normalized::{
        Delta, FinishReason, NormalizedRequest, NormalizedResponse, NormalizedStreamEvent, Role,
        Usage,
    },
    provider::{Provider, ProviderCapabilities},
    session_store::SessionStore,
    tenant::TenantId,
};
use lunaroute_session::SessionEvent;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

#[derive(Clone, Default)]
struct InMemorySessionStore {
    events: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl InMemorySessionStore {
    fn new() -> Self {
        Self::default()
    }

    fn get_events(&self) -> Vec<SessionEvent> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| serde_json::from_value(event.clone()).ok())
            .collect()
    }
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn write_event(
        &self,
        _tenant_id: Option<TenantId>,
        event: serde_json::Value,
    ) -> Result<(), CoreError> {
        self.events.lock().unwrap().push(event);
        Ok(())
    }

    async fn search(
        &self,
        _tenant_id: Option<TenantId>,
        _query: serde_json::Value,
    ) -> Result<serde_json::Value, CoreError> {
        Ok(serde_json::json!({"sessions": []}))
    }

    async fn get_session(
        &self,
        _tenant_id: Option<TenantId>,
        _session_id: &str,
    ) -> Result<serde_json::Value, CoreError> {
        Ok(serde_json::json!(null))
    }

    async fn cleanup(
        &self,
        _tenant_id: Option<TenantId>,
        _params: serde_json::Value,
    ) -> Result<serde_json::Value, CoreError> {
        Ok(serde_json::json!({"deleted": 0}))
    }

    async fn get_stats(
        &self,
        _tenant_id: Option<TenantId>,
        _params: serde_json::Value,
    ) -> Result<serde_json::Value, CoreError> {
        Ok(serde_json::json!({"total_sessions": 0}))
    }
}

struct StreamingProvider;

#[async_trait]
impl Provider for StreamingProvider {
    async fn send(
        &self,
        _request: NormalizedRequest,
    ) -> lunaroute_core::Result<NormalizedResponse> {
        Err(lunaroute_core::Error::Provider(
            "streaming test provider does not support send".to_string(),
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
        let events = vec![
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
            NormalizedStreamEvent::Usage {
                usage: Usage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    total_tokens: 15,
                },
            },
            NormalizedStreamEvent::End {
                finish_reason: FinishReason::Stop,
            },
        ];

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

#[tokio::test]
async fn multi_dialect_routed_openai_streaming_records_session_events() {
    let provider = Arc::new(StreamingProvider);
    let store = Arc::new(InMemorySessionStore::new());
    let app = lunaroute_ingress::multi_dialect::router_with_session_store(provider, store.clone());

    let request = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "gpt-4",
                "messages": [{"role": "user", "content": "Hello"}],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(body.contains("Hello"));
    assert!(body.contains("[DONE]"));

    let started = std::time::Instant::now();
    let events = loop {
        let events = store.get_events();
        if events
            .iter()
            .any(|event| matches!(event, SessionEvent::Completed { .. }))
        {
            break events;
        }

        if started.elapsed() > std::time::Duration::from_secs(2) {
            panic!("timed out waiting for Completed event; got {events:?}");
        }

        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    };

    assert!(
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::RequestRecorded { .. })),
        "expected RequestRecorded event, got {events:?}"
    );

    let started_event = events
        .iter()
        .find(|event| matches!(event, SessionEvent::Started { .. }))
        .expect("expected Started event");

    if let SessionEvent::Started {
        model_requested,
        provider,
        listener,
        is_streaming,
        ..
    } = started_event
    {
        assert_eq!(model_requested, "gpt-4");
        assert_eq!(provider, "openai");
        assert_eq!(listener, "openai");
        assert!(*is_streaming);
    }

    let completed_event = events
        .iter()
        .find(|event| matches!(event, SessionEvent::Completed { .. }))
        .expect("expected Completed event");

    if let SessionEvent::Completed {
        success,
        error,
        final_stats,
        ..
    } = completed_event
    {
        assert!(*success);
        assert_eq!(error, &None);
        assert_eq!(final_stats.total_tokens.total_input, 10);
        assert_eq!(final_stats.total_tokens.total_output, 5);
        assert_eq!(final_stats.total_tokens.grand_total, 15);
    }
}
