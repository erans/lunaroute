//! Integration tests for PostgresSessionStore using testcontainers
//!
//! These tests spin up real PostgreSQL instances using Docker to test
//! the full integration with the database.

use lunaroute_core::{
    Error,
    events::{
        FinalSessionStats, PerformanceMetrics, ResponseStats, SessionEvent, SessionMetadata,
        StreamingStats, TokenStats, TokenTotals, ToolCallStats, ToolUsageSummary,
    },
    session_store::SessionStore,
    tenant::TenantId,
};
use lunaroute_session_postgres::{PostgresSessionStore, PostgresSessionStoreConfig};
use std::{collections::HashMap, time::Duration};
use testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use testcontainers_modules::postgres::Postgres;

/// Helper to create a test PostgreSQL container and store
async fn create_test_store() -> (ContainerAsync<Postgres>, PostgresSessionStore) {
    let container = Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .expect("Failed to start PostgreSQL container");

    let host_port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("Failed to get container port");

    let database_url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres",
        host_port
    );

    // Wait a moment for PostgreSQL to be ready
    tokio::time::sleep(Duration::from_secs(2)).await;

    let store = PostgresSessionStore::new(&database_url)
        .await
        .expect("Failed to create PostgreSQL session store");

    (container, store)
}

/// Helper to create a test store with custom configuration
async fn create_test_store_with_config(
    config: PostgresSessionStoreConfig,
) -> (ContainerAsync<Postgres>, PostgresSessionStore) {
    let container = Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .expect("Failed to start PostgreSQL container");

    let host_port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("Failed to get container port");

    let database_url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres",
        host_port
    );

    // Wait a moment for PostgreSQL to be ready
    tokio::time::sleep(Duration::from_secs(2)).await;

    let store = PostgresSessionStore::with_config(&database_url, config)
        .await
        .expect("Failed to create PostgreSQL session store");

    (container, store)
}

#[tokio::test]
#[ignore] // Requires Docker with proper networking configuration
async fn test_create_store_and_connect() {
    let (_container, store) = create_test_store().await;

    // Verify the store was created successfully
    assert!(store.pool().size() > 0);
}

#[tokio::test]
#[ignore] // Requires Docker with proper networking configuration
async fn test_create_store_with_custom_config() {
    let config = PostgresSessionStoreConfig::default()
        .with_max_connections(10)
        .with_min_connections(2);

    let (_container, _store) = create_test_store_with_config(config).await;

    // If we get here, the store was created successfully with custom config
}

#[tokio::test]
#[ignore] // Requires Docker with proper networking configuration
async fn test_write_started_event() {
    let (_container, store) = create_test_store().await;
    let tenant_id = TenantId::new();

    let event = SessionEvent::Started {
        session_id: "test-session-1".to_string(),
        request_id: "req-1".to_string(),
        timestamp: chrono::Utc::now(),
        model_requested: "gpt-4".to_string(),
        provider: "openai".to_string(),
        listener: "openai".to_string(),
        is_streaming: false,
        metadata: SessionMetadata {
            client_ip: Some("127.0.0.1".to_string()),
            user_agent: Some("test-agent".to_string()),
            api_version: None,
            request_headers: HashMap::new(),
            session_tags: vec![],
        },
    };

    let result = store
        .write_event(Some(tenant_id), serde_json::to_value(&event).unwrap())
        .await;

    assert!(
        result.is_ok(),
        "Failed to write started event: {:?}",
        result
    );
}

#[tokio::test]
#[ignore] // Requires Docker with proper networking configuration
async fn test_write_and_retrieve_session() {
    let (_container, store) = create_test_store().await;
    let tenant_id = TenantId::new();
    let session_id = "test-session-retrieve";

    // Write a started event
    let started_event = SessionEvent::Started {
        session_id: session_id.to_string(),
        request_id: "req-1".to_string(),
        timestamp: chrono::Utc::now(),
        model_requested: "gpt-4".to_string(),
        provider: "openai".to_string(),
        listener: "openai".to_string(),
        is_streaming: false,
        metadata: SessionMetadata {
            client_ip: Some("192.168.1.1".to_string()),
            user_agent: Some("test-agent".to_string()),
            api_version: None,
            request_headers: HashMap::new(),
            session_tags: vec![],
        },
    };

    store
        .write_event(
            Some(tenant_id),
            serde_json::to_value(&started_event).unwrap(),
        )
        .await
        .expect("Failed to write started event");

    // Retrieve the session
    let session = store
        .get_session(Some(tenant_id), session_id)
        .await
        .expect("Failed to retrieve session");

    assert_eq!(session["session_id"], session_id);
    assert_eq!(session["provider"], "openai");
    assert_eq!(session["model_requested"], "gpt-4");
    assert_eq!(session["client_ip"], "192.168.1.1");
}

#[tokio::test]
#[ignore] // Requires Docker with proper networking configuration
async fn test_complete_session_workflow() {
    let (_container, store) = create_test_store().await;
    let tenant_id = TenantId::new();
    let session_id = "test-complete-workflow";

    // 1. Started event
    let started_event = SessionEvent::Started {
        session_id: session_id.to_string(),
        request_id: "req-1".to_string(),
        timestamp: chrono::Utc::now(),
        model_requested: "gpt-4".to_string(),
        provider: "openai".to_string(),
        listener: "openai".to_string(),
        is_streaming: false,
        metadata: SessionMetadata {
            client_ip: None,
            user_agent: None,
            api_version: None,
            request_headers: HashMap::new(),
            session_tags: vec![],
        },
    };

    store
        .write_event(
            Some(tenant_id),
            serde_json::to_value(&started_event).unwrap(),
        )
        .await
        .unwrap();

    // 2. Response recorded event
    let response_event = SessionEvent::ResponseRecorded {
        session_id: session_id.to_string(),
        request_id: "req-1".to_string(),
        timestamp: chrono::Utc::now(),
        response_text: "I'm doing well, thank you!".to_string(),
        response_json: serde_json::json!({"message": "test"}),
        model_used: "gpt-4-0613".to_string(),
        stats: ResponseStats {
            provider_latency_ms: 150,
            post_processing_ms: 10.0,
            total_proxy_overhead_ms: 50.0,
            tokens: TokenStats {
                input_tokens: 5,
                output_tokens: 10,
                total_tokens: 15,
                thinking_tokens: Some(3),
                reasoning_tokens: None,
                cache_read_tokens: None,
                cache_creation_tokens: None,
                audio_input_tokens: None,
                audio_output_tokens: None,
                thinking_percentage: None,
                tokens_per_second: None,
            },
            tool_calls: vec![],
            response_size_bytes: 80,
            content_blocks: 1,
            has_refusal: false,
            is_streaming: false,
            chunk_count: None,
            streaming_duration_ms: None,
        },
    };

    store
        .write_event(
            Some(tenant_id),
            serde_json::to_value(&response_event).unwrap(),
        )
        .await
        .unwrap();

    // 3. Completed event
    let completed_event = SessionEvent::Completed {
        session_id: session_id.to_string(),
        request_id: "req-1".to_string(),
        timestamp: chrono::Utc::now(),
        success: true,
        error: None,
        finish_reason: Some("stop".to_string()),
        final_stats: Box::new(FinalSessionStats {
            total_duration_ms: 200,
            provider_time_ms: 150,
            proxy_overhead_ms: 50.0,
            total_tokens: TokenTotals {
                total_input: 5,
                total_output: 10,
                total_thinking: 3,
                total_reasoning: 0,
                total_cached: 0,
                total_cache_read: 0,
                total_cache_creation: 0,
                total_audio_input: 0,
                total_audio_output: 0,
                grand_total: 15,
                by_model: HashMap::new(),
            },
            tool_summary: ToolUsageSummary {
                total_tool_calls: 0,
                unique_tool_count: 0,
                by_tool: HashMap::new(),
                total_tool_time_ms: 0,
                tool_error_count: 0,
            },
            performance: PerformanceMetrics {
                avg_provider_latency_ms: 150.0,
                p50_latency_ms: Some(150),
                p95_latency_ms: Some(150),
                p99_latency_ms: Some(150),
                max_latency_ms: 150,
                min_latency_ms: 150,
                avg_pre_processing_ms: 0.0,
                avg_post_processing_ms: 10.0,
                proxy_overhead_percentage: 25.0,
            },
            streaming_stats: None,
            estimated_cost: None,
        }),
    };

    store
        .write_event(
            Some(tenant_id),
            serde_json::to_value(&completed_event).unwrap(),
        )
        .await
        .unwrap();

    // Retrieve and verify the session
    let session = store
        .get_session(Some(tenant_id), session_id)
        .await
        .unwrap();

    assert_eq!(session["session_id"], session_id);
    assert_eq!(session["success"], true);
    assert_eq!(session["finish_reason"], "stop");
    assert_eq!(session["input_tokens"], 5);
    assert_eq!(session["output_tokens"], 10);
    assert!(session["completed_at"].as_str().is_some());
}

#[tokio::test]
#[ignore] // Requires Docker with proper networking configuration
async fn test_streaming_session_workflow() {
    let (_container, store) = create_test_store().await;
    let tenant_id = TenantId::new();
    let session_id = "test-streaming-workflow";

    // 1. Started event with streaming
    let started_event = SessionEvent::Started {
        session_id: session_id.to_string(),
        request_id: "req-1".to_string(),
        timestamp: chrono::Utc::now(),
        model_requested: "gpt-4".to_string(),
        provider: "openai".to_string(),
        listener: "openai".to_string(),
        is_streaming: true,
        metadata: SessionMetadata {
            client_ip: None,
            user_agent: None,
            api_version: None,
            request_headers: HashMap::new(),
            session_tags: vec![],
        },
    };

    store
        .write_event(
            Some(tenant_id),
            serde_json::to_value(&started_event).unwrap(),
        )
        .await
        .unwrap();

    // 2. Stream started event
    let stream_started = SessionEvent::StreamStarted {
        session_id: session_id.to_string(),
        request_id: "req-1".to_string(),
        timestamp: chrono::Utc::now(),
        time_to_first_token_ms: 50,
    };

    store
        .write_event(
            Some(tenant_id),
            serde_json::to_value(&stream_started).unwrap(),
        )
        .await
        .unwrap();

    // 3. Complete with streaming stats
    let completed_event = SessionEvent::Completed {
        session_id: session_id.to_string(),
        request_id: "req-1".to_string(),
        timestamp: chrono::Utc::now(),
        success: true,
        error: None,
        finish_reason: Some("stop".to_string()),
        final_stats: Box::new(FinalSessionStats {
            total_duration_ms: 600,
            provider_time_ms: 500,
            proxy_overhead_ms: 100.0,
            total_tokens: TokenTotals {
                total_input: 10,
                total_output: 50,
                total_thinking: 0,
                total_reasoning: 0,
                total_cached: 0,
                total_cache_read: 0,
                total_cache_creation: 0,
                total_audio_input: 0,
                total_audio_output: 0,
                grand_total: 60,
                by_model: HashMap::new(),
            },
            tool_summary: ToolUsageSummary {
                total_tool_calls: 0,
                unique_tool_count: 0,
                by_tool: HashMap::new(),
                total_tool_time_ms: 0,
                tool_error_count: 0,
            },
            performance: PerformanceMetrics {
                avg_provider_latency_ms: 500.0,
                p50_latency_ms: Some(500),
                p95_latency_ms: Some(500),
                p99_latency_ms: Some(500),
                max_latency_ms: 500,
                min_latency_ms: 500,
                avg_pre_processing_ms: 0.0,
                avg_post_processing_ms: 0.0,
                proxy_overhead_percentage: 16.67,
            },
            streaming_stats: Some(StreamingStats {
                time_to_first_token_ms: 50,
                total_chunks: 25,
                streaming_duration_ms: 500,
                avg_chunk_latency_ms: 20.0,
                p50_chunk_latency_ms: Some(18),
                p95_chunk_latency_ms: Some(30),
                p99_chunk_latency_ms: Some(35),
                max_chunk_latency_ms: 40,
                min_chunk_latency_ms: 10,
            }),
            estimated_cost: None,
        }),
    };

    store
        .write_event(
            Some(tenant_id),
            serde_json::to_value(&completed_event).unwrap(),
        )
        .await
        .unwrap();

    // Retrieve and verify the session
    let session = store
        .get_session(Some(tenant_id), session_id)
        .await
        .unwrap();

    assert_eq!(session["session_id"], session_id);
    assert_eq!(session["is_streaming"], true);
}

#[tokio::test]
#[ignore] // Requires Docker with proper networking configuration
async fn test_tool_call_recording() {
    let (_container, store) = create_test_store().await;
    let tenant_id = TenantId::new();
    let session_id = "test-tool-calls";

    // Started event
    let started_event = SessionEvent::Started {
        session_id: session_id.to_string(),
        request_id: "req-1".to_string(),
        timestamp: chrono::Utc::now(),
        model_requested: "gpt-4".to_string(),
        provider: "openai".to_string(),
        listener: "openai".to_string(),
        is_streaming: false,
        metadata: SessionMetadata {
            client_ip: None,
            user_agent: None,
            api_version: None,
            request_headers: HashMap::new(),
            session_tags: vec![],
        },
    };

    store
        .write_event(
            Some(tenant_id),
            serde_json::to_value(&started_event).unwrap(),
        )
        .await
        .unwrap();

    // Tool call recorded event
    let tool_event = SessionEvent::ToolCallRecorded {
        session_id: session_id.to_string(),
        request_id: "req-1".to_string(),
        timestamp: chrono::Utc::now(),
        tool_name: "get_weather".to_string(),
        tool_call_id: "call_123".to_string(),
        execution_time_ms: Some(150),
        input_size_bytes: 50,
        output_size_bytes: Some(200),
        success: Some(true),
        tool_arguments: Some(r#"{"location": "San Francisco"}"#.to_string()),
    };

    store
        .write_event(Some(tenant_id), serde_json::to_value(&tool_event).unwrap())
        .await
        .unwrap();

    // Response with tool calls in stats
    let response_event = SessionEvent::ResponseRecorded {
        session_id: session_id.to_string(),
        request_id: "req-1".to_string(),
        timestamp: chrono::Utc::now(),
        response_text: "The weather is sunny".to_string(),
        response_json: serde_json::json!({"message": "sunny"}),
        model_used: "gpt-4-0613".to_string(),
        stats: ResponseStats {
            provider_latency_ms: 300,
            post_processing_ms: 10.0,
            total_proxy_overhead_ms: 200.0,
            tokens: TokenStats {
                input_tokens: 20,
                output_tokens: 15,
                total_tokens: 35,
                thinking_tokens: None,
                reasoning_tokens: None,
                cache_read_tokens: None,
                cache_creation_tokens: None,
                audio_input_tokens: None,
                audio_output_tokens: None,
                thinking_percentage: None,
                tokens_per_second: None,
            },
            tool_calls: vec![ToolCallStats {
                tool_name: "get_weather".to_string(),
                tool_call_id: Some("call_123".to_string()),
                execution_time_ms: Some(150),
                input_size_bytes: 50,
                output_size_bytes: Some(200),
                success: Some(true),
            }],
            response_size_bytes: 150,
            content_blocks: 1,
            has_refusal: false,
            is_streaming: false,
            chunk_count: None,
            streaming_duration_ms: None,
        },
    };

    store
        .write_event(
            Some(tenant_id),
            serde_json::to_value(&response_event).unwrap(),
        )
        .await
        .unwrap();

    // Verify session was created
    let session = store
        .get_session(Some(tenant_id), session_id)
        .await
        .unwrap();

    assert_eq!(session["session_id"], session_id);
}

#[tokio::test]
#[ignore] // Requires Docker with proper networking configuration
async fn test_tenant_isolation() {
    let (_container, store) = create_test_store().await;
    let tenant_a = TenantId::new();
    let tenant_b = TenantId::new();
    let session_id = "shared-session-id";

    // Write session for tenant A
    let event_a = SessionEvent::Started {
        session_id: session_id.to_string(),
        request_id: "req-a".to_string(),
        timestamp: chrono::Utc::now(),
        model_requested: "gpt-4".to_string(),
        provider: "openai".to_string(),
        listener: "openai".to_string(),
        is_streaming: false,
        metadata: SessionMetadata {
            client_ip: Some("10.0.0.1".to_string()),
            user_agent: None,
            api_version: None,
            request_headers: HashMap::new(),
            session_tags: vec![],
        },
    };

    store
        .write_event(Some(tenant_a), serde_json::to_value(&event_a).unwrap())
        .await
        .unwrap();

    // Write session for tenant B with same session_id
    let event_b = SessionEvent::Started {
        session_id: session_id.to_string(),
        request_id: "req-b".to_string(),
        timestamp: chrono::Utc::now(),
        model_requested: "claude-3".to_string(),
        provider: "anthropic".to_string(),
        listener: "anthropic".to_string(),
        is_streaming: false,
        metadata: SessionMetadata {
            client_ip: Some("10.0.0.2".to_string()),
            user_agent: None,
            api_version: None,
            request_headers: HashMap::new(),
            session_tags: vec![],
        },
    };

    store
        .write_event(Some(tenant_b), serde_json::to_value(&event_b).unwrap())
        .await
        .unwrap();

    // Retrieve sessions for each tenant
    let session_a = store.get_session(Some(tenant_a), session_id).await.unwrap();

    let session_b = store.get_session(Some(tenant_b), session_id).await.unwrap();

    // Verify tenant isolation - different data for each tenant
    assert_eq!(session_a["provider"], "openai");
    assert_eq!(session_a["client_ip"], "10.0.0.1");

    assert_eq!(session_b["provider"], "anthropic");
    assert_eq!(session_b["client_ip"], "10.0.0.2");
}

#[tokio::test]
#[ignore] // Requires Docker with proper networking configuration
async fn test_requires_tenant_id() {
    let (_container, store) = create_test_store().await;

    let event = SessionEvent::Started {
        session_id: "test".to_string(),
        request_id: "req-1".to_string(),
        timestamp: chrono::Utc::now(),
        model_requested: "gpt-4".to_string(),
        provider: "openai".to_string(),
        listener: "openai".to_string(),
        is_streaming: false,
        metadata: SessionMetadata {
            client_ip: None,
            user_agent: None,
            api_version: None,
            request_headers: HashMap::new(),
            session_tags: vec![],
        },
    };

    // write_event without tenant_id should fail
    let result = store
        .write_event(None, serde_json::to_value(&event).unwrap())
        .await;
    assert!(matches!(result, Err(Error::TenantRequired(_))));

    // get_session without tenant_id should fail
    let result = store.get_session(None, "session-id").await;
    assert!(matches!(result, Err(Error::TenantRequired(_))));

    // search without tenant_id should fail
    let result = store.search(None, serde_json::json!({})).await;
    assert!(matches!(result, Err(Error::TenantRequired(_))));

    // list_sessions without tenant_id should fail
    let result = store.list_sessions(None, 10, 0).await;
    assert!(matches!(result, Err(Error::TenantRequired(_))));
}

#[tokio::test]
#[ignore] // Requires Docker with proper networking configuration
async fn test_search_sessions() {
    let (_container, store) = create_test_store().await;
    let tenant_id = TenantId::new();

    // Create multiple sessions
    for i in 0..3 {
        let event = SessionEvent::Started {
            session_id: format!("session-{}", i),
            request_id: format!("req-{}", i),
            timestamp: chrono::Utc::now(),
            model_requested: "gpt-4".to_string(),
            provider: "openai".to_string(),
            listener: "openai".to_string(),
            is_streaming: false,
            metadata: SessionMetadata {
                client_ip: None,
                user_agent: None,
                api_version: None,
                request_headers: HashMap::new(),
                session_tags: vec![],
            },
        };

        store
            .write_event(Some(tenant_id), serde_json::to_value(&event).unwrap())
            .await
            .unwrap();
    }

    // Search sessions
    let results = store
        .search(Some(tenant_id), serde_json::json!({}))
        .await
        .unwrap();

    // Verify we got results
    let items = results["items"].as_array().unwrap();
    assert!(items.len() >= 3, "Expected at least 3 sessions");
}

#[tokio::test]
#[ignore] // Requires Docker with proper networking configuration
async fn test_list_sessions_pagination() {
    let (_container, store) = create_test_store().await;
    let tenant_id = TenantId::new();

    // Create 5 sessions
    for i in 0..5 {
        let event = SessionEvent::Started {
            session_id: format!("page-session-{}", i),
            request_id: format!("req-{}", i),
            timestamp: chrono::Utc::now(),
            model_requested: "gpt-4".to_string(),
            provider: "openai".to_string(),
            listener: "openai".to_string(),
            is_streaming: false,
            metadata: SessionMetadata {
                client_ip: None,
                user_agent: None,
                api_version: None,
                request_headers: HashMap::new(),
                session_tags: vec![],
            },
        };

        store
            .write_event(Some(tenant_id), serde_json::to_value(&event).unwrap())
            .await
            .unwrap();
    }

    // Test pagination - first page
    let page1 = store.list_sessions(Some(tenant_id), 2, 0).await.unwrap();
    assert_eq!(page1.len(), 2);

    // Test pagination - second page
    let page2 = store.list_sessions(Some(tenant_id), 2, 2).await.unwrap();
    assert_eq!(page2.len(), 2);

    // Test pagination - third page
    let page3 = store.list_sessions(Some(tenant_id), 2, 4).await.unwrap();
    assert_eq!(page3.len(), 1);
}

#[tokio::test]
#[ignore] // Requires Docker with proper networking configuration
async fn test_session_not_found() {
    let (_container, store) = create_test_store().await;
    let tenant_id = TenantId::new();

    let result = store
        .get_session(Some(tenant_id), "non-existent-session")
        .await;

    assert!(matches!(result, Err(Error::SessionNotFound(_))));
}

#[tokio::test]
#[ignore] // Requires Docker with proper networking configuration
async fn test_flush() {
    let (_container, store) = create_test_store().await;

    // Flush should succeed (even though it's a no-op for PostgreSQL)
    let result = store.flush().await;
    assert!(result.is_ok());
}

#[tokio::test]
#[ignore] // Requires Docker with proper networking configuration
async fn test_concurrent_writes() {
    let (_container, store) = create_test_store().await;
    let tenant_id = TenantId::new();

    // Create multiple concurrent write tasks
    let mut handles = vec![];

    for i in 0..10 {
        let store = store.clone();

        let handle = tokio::spawn(async move {
            let event = SessionEvent::Started {
                session_id: format!("concurrent-session-{}", i),
                request_id: format!("req-{}", i),
                timestamp: chrono::Utc::now(),
                model_requested: "gpt-4".to_string(),
                provider: "openai".to_string(),
                listener: "openai".to_string(),
                is_streaming: false,
                metadata: SessionMetadata {
                    client_ip: None,
                    user_agent: None,
                    api_version: None,
                    request_headers: HashMap::new(),
                    session_tags: vec![],
                },
            };

            store
                .write_event(Some(tenant_id), serde_json::to_value(&event).unwrap())
                .await
        });

        handles.push(handle);
    }

    // Wait for all writes to complete
    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_ok(), "Concurrent write failed");
    }

    // Verify all sessions were written
    let sessions = store.list_sessions(Some(tenant_id), 20, 0).await.unwrap();
    assert!(
        sessions.len() >= 10,
        "Expected at least 10 sessions from concurrent writes"
    );
}
