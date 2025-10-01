//! Integration tests for Router streaming functionality
//!
//! These tests verify Router behavior with streaming requests,
//! including fallback, circuit breaker, and error handling.

use futures::stream::{self, StreamExt};
use lunaroute_core::{
    normalized::{
        Choice, Delta, FinishReason, Message, MessageContent, NormalizedRequest, NormalizedResponse,
        NormalizedStreamEvent, Role, Usage,
    },
    provider::Provider,
    Error, Result,
};
use lunaroute_routing::{
    CircuitBreakerConfig, HealthMonitorConfig, RouteTable, Router, RoutingRule, RuleMatcher,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

// Test provider that can stream responses
#[derive(Clone)]
struct StreamingTestProvider {
    id: String,
    should_fail: Arc<AtomicBool>,
    call_count: Arc<AtomicUsize>,
    stream_events: Vec<NormalizedStreamEvent>,
}

impl StreamingTestProvider {
    fn new(id: &str, events: Vec<NormalizedStreamEvent>) -> Self {
        Self {
            id: id.to_string(),
            should_fail: Arc::new(AtomicBool::new(false)),
            call_count: Arc::new(AtomicUsize::new(0)),
            stream_events: events,
        }
    }

    fn set_should_fail(&self, fail: bool) {
        self.should_fail.store(fail, Ordering::SeqCst);
    }

    fn get_call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl Provider for StreamingTestProvider {
    async fn send(&self, request: NormalizedRequest) -> Result<NormalizedResponse> {
        self.call_count.fetch_add(1, Ordering::SeqCst);

        if self.should_fail.load(Ordering::SeqCst) {
            return Err(Error::Provider(format!("Provider {} failed", self.id)));
        }

        Ok(NormalizedResponse {
            id: format!("{}-response", self.id),
            model: request.model,
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: MessageContent::Text(format!("Response from {}", self.id)),
                    name: None,
                    tool_calls: vec![],
                    tool_call_id: None,
                },
                finish_reason: Some(FinishReason::Stop),
            }],
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 20,
                total_tokens: 30,
            },
            created: 1234567890,
            metadata: HashMap::new(),
        })
    }

    async fn stream(
        &self,
        _request: NormalizedRequest,
    ) -> Result<Box<dyn futures::Stream<Item = Result<NormalizedStreamEvent>> + Send + Unpin>> {
        self.call_count.fetch_add(1, Ordering::SeqCst);

        if self.should_fail.load(Ordering::SeqCst) {
            return Err(Error::Provider(format!("Provider {} stream failed", self.id)));
        }

        // Create stream from pre-defined events
        let events = self.stream_events.clone();
        let stream = stream::iter(events.into_iter().map(Ok));
        Ok(Box::new(Box::pin(stream)))
    }

    fn capabilities(&self) -> lunaroute_core::provider::ProviderCapabilities {
        lunaroute_core::provider::ProviderCapabilities {
            supports_streaming: true,
            supports_tools: false,
            supports_vision: false,
        }
    }
}

fn create_test_request(model: &str, streaming: bool) -> NormalizedRequest {
    NormalizedRequest {
        model: model.to_string(),
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("test".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        max_tokens: Some(100),
        temperature: Some(0.7),
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: streaming,
        tools: vec![],
        tool_choice: None,
        metadata: HashMap::new(),
    }
}

fn create_stream_events(provider_id: &str) -> Vec<NormalizedStreamEvent> {
    vec![
        NormalizedStreamEvent::Start {
            id: format!("{}-stream", provider_id),
            model: "test-model".to_string(),
        },
        NormalizedStreamEvent::Delta {
            index: 0,
            delta: Delta {
                role: Some(Role::Assistant),
                content: Some(format!("Hello from {}", provider_id)),
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
    ]
}

#[tokio::test]
async fn test_streaming_basic_routing() {
    let primary_events = create_stream_events("primary");
    let primary = StreamingTestProvider::new("primary", primary_events);

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("primary".to_string(), Arc::new(primary.clone()));

    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("test-route".to_string()),
        matcher: RuleMatcher::Always,
        primary: "primary".to_string(),
        fallbacks: vec![],
    }];

    let route_table = RouteTable::with_rules(rules);
    let router = Router::with_defaults(route_table, providers);

    let request = create_test_request("test-model", true);
    let mut stream = router.stream(request).await.unwrap();

    // Collect all events
    let mut events = vec![];
    while let Some(event) = stream.next().await {
        events.push(event.unwrap());
    }

    // Verify we got all 4 events (Start, Delta, Usage, End)
    assert_eq!(events.len(), 4);
    assert!(matches!(events[0], NormalizedStreamEvent::Start { .. }));
    assert!(matches!(events[1], NormalizedStreamEvent::Delta { .. }));
    assert!(matches!(events[2], NormalizedStreamEvent::Usage { .. }));
    assert!(matches!(events[3], NormalizedStreamEvent::End { .. }));

    // Verify provider was called
    assert_eq!(primary.get_call_count(), 1);
}

#[tokio::test]
async fn test_streaming_fallback_with_circuit_breaker() {
    // Note: Router currently only supports fallback for streaming when circuit breaker is open,
    // not when stream() returns an error. This test verifies circuit breaker-based fallback.

    let primary_events = create_stream_events("primary");
    let primary = StreamingTestProvider::new("primary", primary_events);
    primary.set_should_fail(true); // Will cause failures to open circuit breaker

    let fallback_events = create_stream_events("fallback");
    let fallback = StreamingTestProvider::new("fallback", fallback_events);

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("primary".to_string(), Arc::new(primary.clone()));
    providers.insert("fallback".to_string(), Arc::new(fallback.clone()));

    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("test-route".to_string()),
        matcher: RuleMatcher::Always,
        primary: "primary".to_string(),
        fallbacks: vec!["fallback".to_string()],
    }];

    let route_table = RouteTable::with_rules(rules);

    // Circuit breaker: 2 failures opens
    let cb_config = CircuitBreakerConfig {
        failure_threshold: 2,
        success_threshold: 1,
        timeout: Duration::from_millis(100),
    };

    let health_config = HealthMonitorConfig::default();
    let router = Router::new(route_table, providers, health_config, cb_config);

    let request = create_test_request("test-model", true);

    // Cause 2 failures with non-streaming to open circuit breaker
    let _ = router.send(request.clone()).await;
    let _ = router.send(request.clone()).await;

    // Now circuit breaker is open, stream should use fallback
    let mut stream = router.stream(request.clone()).await.unwrap();

    // Collect all events
    let mut events = vec![];
    while let Some(event) = stream.next().await {
        events.push(event.unwrap());
    }

    // Should have fallback events (4 events: Start, Delta, Usage, End)
    assert_eq!(events.len(), 4);

    // Verify primary was called twice for send() failures
    assert_eq!(primary.get_call_count(), 2);

    // Verify fallback was used for both send() failures and stream()
    // - 2 send() calls that failed over to fallback
    // - 1 stream() call that used fallback (primary circuit breaker was open)
    assert_eq!(fallback.get_call_count(), 3);

    // Verify content came from fallback
    if let NormalizedStreamEvent::Delta { delta, .. } = &events[1] {
        assert!(delta.content.as_ref().unwrap().contains("fallback"));
    }
}

#[tokio::test]
async fn test_streaming_error_handling() {
    // Test that stream() errors are properly propagated
    let primary_events = create_stream_events("primary");
    let primary = StreamingTestProvider::new("primary", primary_events);
    primary.set_should_fail(true);

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("primary".to_string(), Arc::new(primary.clone()));

    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("test-route".to_string()),
        matcher: RuleMatcher::Always,
        primary: "primary".to_string(),
        fallbacks: vec![],
    }];

    let route_table = RouteTable::with_rules(rules);
    let router = Router::with_defaults(route_table, providers);

    let request = create_test_request("test-model", true);
    let result = router.stream(request).await;

    // Should fail since provider is configured to fail
    assert!(result.is_err());

    // Verify provider was called
    assert_eq!(primary.get_call_count(), 1);
}

#[tokio::test]
async fn test_streaming_circuit_breaker_blocks() {
    let primary_events = create_stream_events("primary");
    let primary = StreamingTestProvider::new("primary", primary_events);
    primary.set_should_fail(true);

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("primary".to_string(), Arc::new(primary.clone()));

    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("test-route".to_string()),
        matcher: RuleMatcher::Always,
        primary: "primary".to_string(),
        fallbacks: vec![],
    }];

    let route_table = RouteTable::with_rules(rules);

    // Circuit breaker: 2 failures opens
    let cb_config = CircuitBreakerConfig {
        failure_threshold: 2,
        success_threshold: 1,
        timeout: Duration::from_millis(100),
    };

    let health_config = HealthMonitorConfig::default();
    let router = Router::new(route_table, providers, health_config, cb_config);

    let request = create_test_request("test-model", false);

    // Cause 2 send() failures to open circuit breaker
    let _ = router.send(request.clone()).await;
    let _ = router.send(request.clone()).await;

    // Now circuit breaker should be open for streaming too
    let streaming_request = create_test_request("test-model", true);
    let result = router.stream(streaming_request).await;
    assert!(result.is_err());

    // Provider's stream should not have been called due to open circuit breaker
    // Only send() was called twice
    assert_eq!(primary.get_call_count(), 2);
}

#[tokio::test]
async fn test_streaming_multiple_chunks() {
    // Create a stream with many deltas
    let events = vec![
        NormalizedStreamEvent::Start {
            id: "test-stream".to_string(),
            model: "test-model".to_string(),
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
                prompt_tokens: 5,
                completion_tokens: 3,
                total_tokens: 8,
            },
        },
        NormalizedStreamEvent::End {
            finish_reason: FinishReason::Stop,
        },
    ];

    let provider = StreamingTestProvider::new("test", events);

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("test".to_string(), Arc::new(provider));

    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("test-route".to_string()),
        matcher: RuleMatcher::Always,
        primary: "test".to_string(),
        fallbacks: vec![],
    }];

    let route_table = RouteTable::with_rules(rules);
    let router = Router::with_defaults(route_table, providers);

    let request = create_test_request("test-model", true);
    let mut stream = router.stream(request).await.unwrap();

    // Collect all events
    let mut events = vec![];
    while let Some(event) = stream.next().await {
        events.push(event.unwrap());
    }

    // Should have 6 events (start + 3 deltas + usage + end)
    assert_eq!(events.len(), 6);

    // Collect all content from Delta events
    let content: String = events
        .iter()
        .filter_map(|e| match e {
            NormalizedStreamEvent::Delta { delta, .. } => delta.content.clone(),
            _ => None,
        })
        .collect();

    assert_eq!(content, "Hello world!");
}

#[tokio::test]
async fn test_streaming_with_usage_tracking() {
    let events = vec![
        NormalizedStreamEvent::Start {
            id: "test-stream".to_string(),
            model: "test-model".to_string(),
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
                prompt_tokens: 100,
                completion_tokens: 50,
                total_tokens: 150,
            },
        },
        NormalizedStreamEvent::End {
            finish_reason: FinishReason::Stop,
        },
    ];

    let provider = StreamingTestProvider::new("test", events);

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("test".to_string(), Arc::new(provider));

    let rules = vec![RoutingRule {
        priority: 10,
        name: Some("test-route".to_string()),
        matcher: RuleMatcher::Always,
        primary: "test".to_string(),
        fallbacks: vec![],
    }];

    let route_table = RouteTable::with_rules(rules);
    let router = Router::with_defaults(route_table, providers);

    let request = create_test_request("test-model", true);
    let mut stream = router.stream(request).await.unwrap();

    // Collect all events
    let mut events = vec![];
    while let Some(event) = stream.next().await {
        events.push(event.unwrap());
    }

    // Find the usage event
    let usage_event = events
        .iter()
        .find(|e| matches!(e, NormalizedStreamEvent::Usage { .. }))
        .unwrap();

    if let NormalizedStreamEvent::Usage { usage } = usage_event {
        assert_eq!(usage.prompt_tokens, 100);
        assert_eq!(usage.completion_tokens, 50);
        assert_eq!(usage.total_tokens, 150);
    }
}
