//! Real API integration tests
//!
//! These tests make actual calls to OpenAI and Anthropic APIs.
//! They require API keys in a .env file and are ignored by default.
//!
//! Run with: cargo test --package lunaroute_integration_tests -- --ignored
//! Run specific test: cargo test --package lunaroute_integration_tests test_openai_real_api -- --ignored

use dotenv::dotenv;
use lunaroute_core::{
    normalized::{Message, MessageContent, NormalizedRequest, Role},
    provider::Provider,
};
use lunaroute_egress::{
    anthropic::{AnthropicConfig, AnthropicConnector},
    openai::{OpenAIConfig, OpenAIConnector},
};
use std::env;

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lunaroute=debug,reqwest=warn")
        .try_init();
}

fn get_openai_key() -> String {
    dotenv().ok();
    env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set in .env file")
}

fn get_anthropic_key() -> String {
    dotenv().ok();
    env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY must be set in .env file")
}

#[tokio::test]
#[ignore] // Run with --ignored flag
async fn test_openai_real_api_simple_completion() {
    init_tracing();

    let config = OpenAIConfig {
        api_key: get_openai_key(),
        base_url: "https://api.openai.com/v1".to_string(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let connector = OpenAIConnector::new(config).await.unwrap();

    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("Say 'Hello, LunaRoute!' and nothing else.".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        model: "gpt-5-mini".to_string(), // GPT-5 mini (cost-effective)
        max_tokens: Some(50),
        temperature: None, // GPT-5 doesn't support temperature parameter
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    let response = connector.send(request).await.unwrap();

    // Verify response structure
    assert!(response.model.contains("gpt-5")); // OpenAI returns full version with date
    assert_eq!(response.choices.len(), 1);
    assert_eq!(response.choices[0].message.role, Role::Assistant);

    // Verify content is text (GPT-5 reasoning models may respond differently)
    if let MessageContent::Text(text) = &response.choices[0].message.content {
        assert!(!text.is_empty(), "Response should not be empty");
    } else {
        panic!("Expected text content");
    }

    // Verify usage
    assert!(response.usage.total_tokens > 0);
    assert!(response.usage.prompt_tokens > 0);
    assert!(response.usage.completion_tokens > 0);
}

#[tokio::test]
#[ignore]
async fn test_anthropic_real_api_simple_completion() {
    init_tracing();

    let config = AnthropicConfig {
        api_key: get_anthropic_key(),
        base_url: "https://api.anthropic.com".to_string(),
        api_version: "2023-06-01".to_string(),
        client_config: Default::default(),
        switch_notification_message: None,
    };
    let connector = AnthropicConnector::new(config).unwrap();

    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("Say 'Hello, LunaRoute!' and nothing else.".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        model: "claude-sonnet-4-5".to_string(), // Claude Sonnet 4.5
        max_tokens: Some(50),
        temperature: Some(0.0),
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    let response = connector.send(request).await.unwrap();

    // Verify response structure
    assert!(response.model.contains("claude")); // Claude may return full version
    assert_eq!(response.choices.len(), 1);
    assert_eq!(response.choices[0].message.role, Role::Assistant);

    // Verify content is text
    if let MessageContent::Text(text) = &response.choices[0].message.content {
        assert!(!text.is_empty(), "Response should not be empty");
    } else {
        panic!("Expected text content");
    }

    // Verify usage
    assert!(response.usage.total_tokens > 0);
}

#[tokio::test]
#[ignore]
async fn test_openai_with_system_message() {
    init_tracing();

    let config = OpenAIConfig {
        api_key: get_openai_key(),
        base_url: "https://api.openai.com/v1".to_string(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let connector = OpenAIConnector::new(config).await.unwrap();

    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("What color is the sky?".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: Some("You are a helpful assistant. Always answer in exactly one word.".to_string()),
        model: "gpt-5-mini".to_string(),
        max_tokens: Some(10),
        temperature: None, // GPT-5 doesn't support temperature parameter
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    let response = connector.send(request).await.unwrap();

    assert!(!response.choices.is_empty());
    if let MessageContent::Text(text) = &response.choices[0].message.content {
        // Should be a very short answer due to system prompt
        assert!(text.len() < 20);
    }
}

#[tokio::test]
#[ignore]
async fn test_anthropic_with_system_message() {
    init_tracing();

    let config = AnthropicConfig {
        api_key: get_anthropic_key(),
        base_url: "https://api.anthropic.com".to_string(),
        api_version: "2023-06-01".to_string(),
        client_config: Default::default(),
        switch_notification_message: None,
    };
    let connector = AnthropicConnector::new(config).unwrap();

    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("What color is the sky?".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: Some("You are a helpful assistant. Always answer in exactly one word.".to_string()),
        model: "claude-sonnet-4-5".to_string(),
        max_tokens: Some(10),
        temperature: Some(0.0),
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    let response = connector.send(request).await.unwrap();

    assert!(!response.choices.is_empty());
    if let MessageContent::Text(text) = &response.choices[0].message.content {
        // Should be a very short answer due to system prompt
        assert!(text.len() < 20);
    }
}

#[tokio::test]
#[ignore]
async fn test_openai_error_handling_invalid_model() {
    init_tracing();

    let config = OpenAIConfig {
        api_key: get_openai_key(),
        base_url: "https://api.openai.com/v1".to_string(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let connector = OpenAIConnector::new(config).await.unwrap();

    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("Test".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        model: "invalid-model-name-that-does-not-exist".to_string(),
        max_tokens: Some(10),
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    // Should get an error from OpenAI
    let result = connector.send(request).await;
    assert!(result.is_err());
}

#[tokio::test]
#[ignore]
async fn test_both_providers_sequential() {
    init_tracing();

    // Test OpenAI
    let openai_config = OpenAIConfig {
        api_key: get_openai_key(),
        base_url: "https://api.openai.com/v1".to_string(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let openai_connector = OpenAIConnector::new(openai_config).await.unwrap();

    let openai_request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("Say 'OpenAI works!'".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        model: "gpt-5-mini".to_string(),
        max_tokens: Some(20),
        temperature: None, // GPT-5 doesn't support temperature parameter
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    let openai_response = openai_connector.send(openai_request).await.unwrap();
    assert!(!openai_response.choices.is_empty());

    // Test Anthropic
    let anthropic_config = AnthropicConfig {
        api_key: get_anthropic_key(),
        base_url: "https://api.anthropic.com".to_string(),
        api_version: "2023-06-01".to_string(),
        client_config: Default::default(),
        switch_notification_message: None,
    };
    let anthropic_connector = AnthropicConnector::new(anthropic_config).unwrap();

    let anthropic_request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("Say 'Anthropic works!'".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        model: "claude-sonnet-4-5".to_string(),
        max_tokens: Some(20),
        temperature: Some(0.0),
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    let anthropic_response = anthropic_connector.send(anthropic_request).await.unwrap();
    assert!(!anthropic_response.choices.is_empty());
}

#[tokio::test]
#[ignore]
async fn test_openai_streaming_basic() {
    use futures::StreamExt;

    init_tracing();

    let config = OpenAIConfig {
        api_key: get_openai_key(),
        base_url: "https://api.openai.com/v1".to_string(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let connector = OpenAIConnector::new(config).await.unwrap();

    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text(
                "Count from 1 to 5, each number on a new line.".to_string(),
            ),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        model: "gpt-5-mini".to_string(),
        max_tokens: Some(50),
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: true,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    let mut stream = connector.stream(request).await.unwrap();

    let mut event_count = 0;
    let mut content_chunks = vec![];

    while let Some(event_result) = stream.next().await {
        let event = event_result.unwrap();
        event_count += 1;

        match event {
            lunaroute_core::normalized::NormalizedStreamEvent::Start { id, model } => {
                assert!(!id.is_empty());
                assert!(model.contains("gpt"));
            }
            lunaroute_core::normalized::NormalizedStreamEvent::Delta { delta, .. } => {
                if let Some(content) = delta.content {
                    content_chunks.push(content);
                }
            }
            lunaroute_core::normalized::NormalizedStreamEvent::Usage { usage } => {
                assert!(usage.total_tokens > 0);
            }
            lunaroute_core::normalized::NormalizedStreamEvent::End { finish_reason } => {
                assert_eq!(
                    finish_reason,
                    lunaroute_core::normalized::FinishReason::Stop
                );
            }
            _ => {}
        }
    }

    // Should have received multiple events
    assert!(event_count > 3);

    // Should have received content
    let full_content: String = content_chunks.join("");
    assert!(!full_content.is_empty());
}

#[tokio::test]
#[ignore]
async fn test_anthropic_streaming_basic() {
    use futures::StreamExt;

    init_tracing();

    let config = AnthropicConfig {
        api_key: get_anthropic_key(),
        base_url: "https://api.anthropic.com".to_string(),
        api_version: "2023-06-01".to_string(),
        client_config: Default::default(),
        switch_notification_message: None,
    };
    let connector = AnthropicConnector::new(config).unwrap();

    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text(
                "Count from 1 to 5, each number on a new line.".to_string(),
            ),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        model: "claude-sonnet-4-5".to_string(),
        max_tokens: Some(50),
        temperature: Some(0.0),
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: true,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    let mut stream = connector.stream(request).await.unwrap();

    let mut event_count = 0;
    let mut content_chunks = vec![];

    while let Some(event_result) = stream.next().await {
        let event = event_result.unwrap();
        event_count += 1;

        match event {
            lunaroute_core::normalized::NormalizedStreamEvent::Start { id, model } => {
                assert!(!id.is_empty());
                assert!(model.contains("claude"));
            }
            lunaroute_core::normalized::NormalizedStreamEvent::Delta { delta, .. } => {
                if let Some(content) = delta.content {
                    content_chunks.push(content);
                }
            }
            lunaroute_core::normalized::NormalizedStreamEvent::Usage { usage } => {
                assert!(usage.total_tokens > 0);
            }
            lunaroute_core::normalized::NormalizedStreamEvent::End { finish_reason } => {
                assert_eq!(
                    finish_reason,
                    lunaroute_core::normalized::FinishReason::Stop
                );
            }
            _ => {}
        }
    }

    // Should have received multiple events
    assert!(event_count > 3);

    // Should have received content
    let full_content: String = content_chunks.join("");
    assert!(!full_content.is_empty());
}

#[tokio::test]
#[ignore]
async fn test_openai_streaming_with_system_prompt() {
    use futures::StreamExt;

    init_tracing();

    let config = OpenAIConfig {
        api_key: get_openai_key(),
        base_url: "https://api.openai.com/v1".to_string(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let connector = OpenAIConnector::new(config).await.unwrap();

    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("What is 2+2?".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: Some(
            "You are a helpful math tutor. Always answer in exactly one word or number."
                .to_string(),
        ),
        model: "gpt-5-mini".to_string(),
        max_tokens: Some(10),
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: true,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    let mut stream = connector.stream(request).await.unwrap();

    let mut content = String::new();

    while let Some(event_result) = stream.next().await {
        let event = event_result.unwrap();

        if let lunaroute_core::normalized::NormalizedStreamEvent::Delta { delta, .. } = event
            && let Some(chunk) = delta.content
        {
            content.push_str(&chunk);
        }
    }

    // Response should be very short due to system prompt
    assert!(content.len() < 20);
    assert!(!content.is_empty());
}

#[tokio::test]
#[ignore]
async fn test_anthropic_streaming_with_system_prompt() {
    use futures::StreamExt;

    init_tracing();

    let config = AnthropicConfig {
        api_key: get_anthropic_key(),
        base_url: "https://api.anthropic.com".to_string(),
        api_version: "2023-06-01".to_string(),
        client_config: Default::default(),
        switch_notification_message: None,
    };
    let connector = AnthropicConnector::new(config).unwrap();

    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("What is 2+2?".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: Some(
            "You are a helpful math tutor. Always answer in exactly one word or number."
                .to_string(),
        ),
        model: "claude-sonnet-4-5".to_string(),
        max_tokens: Some(10),
        temperature: Some(0.0),
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: true,
        tools: vec![],
        tool_results: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    let mut stream = connector.stream(request).await.unwrap();

    let mut content = String::new();

    while let Some(event_result) = stream.next().await {
        let event = event_result.unwrap();

        if let lunaroute_core::normalized::NormalizedStreamEvent::Delta { delta, .. } = event
            && let Some(chunk) = delta.content
        {
            content.push_str(&chunk);
        }
    }

    // Response should be very short due to system prompt
    assert!(content.len() < 20);
    assert!(!content.is_empty());
}

#[tokio::test]
#[ignore] // Run with --ignored flag (requires OPENAI_API_KEY in .env)
async fn test_anthropic_request_routed_to_openai_real_api() {
    //! End-to-end test: Anthropic client → LunaRoute server → OpenAI API
    //!
    //! Flow:
    //! 1. Client sends Anthropic-formatted HTTP request to /v1/messages
    //! 2. Anthropic ingress layer translates to normalized format
    //! 3. OpenAI egress connector translates to OpenAI format
    //! 4. Calls real OpenAI API
    //! 5. Response translated back: OpenAI → Normalized → Anthropic
    //! 6. Client receives Anthropic-formatted response

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::json;
    use std::sync::Arc;
    use tower::ServiceExt;

    init_tracing();

    // Create OpenAI egress connector with real API key
    let openai_config = OpenAIConfig {
        api_key: get_openai_key(),
        base_url: "https://api.openai.com/v1".to_string(),
        organization: None,
        client_config: Default::default(),
        custom_headers: None,
        request_body_config: None,
        response_body_config: None,
        codex_auth: None,
        switch_notification_message: None,
    };
    let openai_connector = OpenAIConnector::new(openai_config).await.unwrap();

    // Create Anthropic ingress router with OpenAI as backend
    let app = lunaroute_ingress::anthropic::router(Arc::new(openai_connector));

    // Send Anthropic-formatted request
    // Note: GPT-5 mini doesn't support temperature parameter
    let anthropic_request = json!({
        "model": "gpt-5-mini",
        "max_tokens": 200,  // Increased from 50 to see actual response
        "messages": [{
            "role": "user",
            "content": "Say 'Translation works!' and nothing else."
        }]
    });

    println!("Sending Anthropic request to local server (routes to OpenAI API)...");

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/messages")
                .method("POST")
                .header("content-type", "application/json")
                .header("anthropic-version", "2023-06-01")
                .header("x-api-key", "not-used") // Server uses OpenAI key from connector
                .body(Body::from(serde_json::to_vec(&anthropic_request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Verify response status
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "Expected 200 OK response"
    );

    // Parse Anthropic response
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let anthropic_response: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    println!(
        "Received Anthropic response: {}",
        serde_json::to_string_pretty(&anthropic_response).unwrap()
    );

    // Verify Anthropic response structure
    assert_eq!(
        anthropic_response["type"], "message",
        "Response should have type=message"
    );
    assert_eq!(
        anthropic_response["role"], "assistant",
        "Response should have role=assistant"
    );

    // Verify content is an array (Anthropic format)
    assert!(
        anthropic_response["content"].is_array(),
        "Content should be array in Anthropic format"
    );

    // Extract and display content
    let content_blocks = anthropic_response["content"].as_array().unwrap();
    let stop_reason = anthropic_response["stop_reason"].as_str().unwrap();

    if !content_blocks.is_empty() && content_blocks[0]["type"] == "text" {
        let text = content_blocks[0]["text"].as_str().unwrap();
        println!("   📝 Response text: '{}'", text);
    } else {
        println!("   ⚠️  No text content (stop_reason: {})", stop_reason);
    }

    // Verify usage is present (Anthropic format)
    assert!(
        anthropic_response["usage"].is_object(),
        "Should have usage object"
    );
    let usage = &anthropic_response["usage"];
    assert!(
        usage["input_tokens"].as_u64().unwrap() > 0,
        "Should have input tokens"
    );
    assert!(
        usage["output_tokens"].as_u64().unwrap() > 0,
        "Should have output tokens"
    );

    // Verify model field
    assert!(
        anthropic_response["model"]
            .as_str()
            .unwrap()
            .contains("gpt"),
        "Model should contain 'gpt' (from OpenAI)"
    );

    println!("✅ Success: Anthropic request → OpenAI API → Anthropic response");
    println!("   Translation complete! The full flow works:");
    println!("   1. Sent Anthropic-formatted request to /v1/messages");
    println!("   2. Translated to OpenAI format");
    println!("   3. Called real OpenAI API (gpt-5-mini)");
    println!("   4. Received OpenAI response");
    println!("   5. Translated back to Anthropic format");
    println!(
        "   6. Response has correct Anthropic structure (type, role, content array, usage, etc.)"
    );
}

#[tokio::test]
#[ignore] // Run with --ignored flag (requires ANTHROPIC_API_KEY in .env)
async fn test_openai_request_routed_to_anthropic_real_api() {
    //! End-to-end test: OpenAI client → LunaRoute server → Anthropic API
    //!
    //! Flow:
    //! 1. Client sends OpenAI-formatted HTTP request to /v1/chat/completions
    //! 2. OpenAI ingress layer translates to normalized format
    //! 3. Anthropic egress connector translates to Anthropic format
    //! 4. Calls real Anthropic API
    //! 5. Response translated back: Anthropic → Normalized → OpenAI
    //! 6. Client receives OpenAI-formatted response

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::json;
    use std::sync::Arc;
    use tower::ServiceExt;

    init_tracing();

    // Create Anthropic egress connector with real API key
    let anthropic_config = AnthropicConfig {
        api_key: get_anthropic_key(),
        base_url: "https://api.anthropic.com".to_string(),
        api_version: "2023-06-01".to_string(),
        client_config: Default::default(),
        switch_notification_message: None,
    };
    let anthropic_connector = AnthropicConnector::new(anthropic_config).unwrap();

    // Create OpenAI ingress router with Anthropic as backend
    let app = lunaroute_ingress::openai::router(Arc::new(anthropic_connector));

    // Send OpenAI-formatted request
    let openai_request = json!({
        "model": "claude-sonnet-4-5",
        "max_tokens": 200,
        "messages": [{
            "role": "user",
            "content": "Say 'Reverse translation works!' and nothing else."
        }]
    });

    println!("Sending OpenAI request to local server (routes to Anthropic API)...");

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/chat/completions")
                .method("POST")
                .header("content-type", "application/json")
                .header("authorization", "Bearer not-used") // Server uses Anthropic key from connector
                .body(Body::from(serde_json::to_vec(&openai_request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Verify response status
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "Expected 200 OK response"
    );

    // Parse OpenAI response
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let openai_response: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    println!(
        "Received OpenAI response: {}",
        serde_json::to_string_pretty(&openai_response).unwrap()
    );

    // Verify OpenAI response structure
    assert_eq!(
        openai_response["object"], "chat.completion",
        "Response should have object=chat.completion"
    );

    // Verify choices array (OpenAI format)
    assert!(
        openai_response["choices"].is_array(),
        "Should have choices array in OpenAI format"
    );
    let choices = openai_response["choices"].as_array().unwrap();
    assert!(!choices.is_empty(), "Choices array should not be empty");

    // Extract and display content
    let first_choice = &choices[0];
    assert_eq!(first_choice["index"], 0, "First choice should have index 0");

    let message = &first_choice["message"];
    assert_eq!(
        message["role"], "assistant",
        "Message should have role=assistant"
    );

    let content = message["content"].as_str().unwrap();
    println!("   📝 Response text: '{}'", content);

    let finish_reason = first_choice["finish_reason"].as_str().unwrap();
    println!("   🏁 Finish reason: {}", finish_reason);

    // Verify usage is present (OpenAI format)
    assert!(
        openai_response["usage"].is_object(),
        "Should have usage object"
    );
    let usage = &openai_response["usage"];
    assert!(
        usage["prompt_tokens"].as_u64().unwrap() > 0,
        "Should have prompt tokens"
    );
    assert!(
        usage["completion_tokens"].as_u64().unwrap() > 0,
        "Should have completion tokens"
    );
    assert!(
        usage["total_tokens"].as_u64().unwrap() > 0,
        "Should have total tokens"
    );

    // Verify model field
    assert!(
        openai_response["model"]
            .as_str()
            .unwrap()
            .contains("claude"),
        "Model should contain 'claude' (from Anthropic)"
    );

    println!("✅ Success: OpenAI request → Anthropic API → OpenAI response");
    println!("   Translation complete! The full flow works:");
    println!("   1. Sent OpenAI-formatted request to /v1/chat/completions");
    println!("   2. Translated to Anthropic format");
    println!("   3. Called real Anthropic API (claude-sonnet-4-5)");
    println!("   4. Received Anthropic response");
    println!("   5. Translated back to OpenAI format");
    println!("   6. Response has correct OpenAI structure (object, choices, message, usage, etc.)");
}
