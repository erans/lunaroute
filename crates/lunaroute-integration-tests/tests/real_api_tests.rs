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
    };
    let connector = OpenAIConnector::new(config).unwrap();

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
    };
    let connector = OpenAIConnector::new(config).unwrap();

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
    };
    let connector = OpenAIConnector::new(config).unwrap();

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
    };
    let openai_connector = OpenAIConnector::new(openai_config).unwrap();

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
    };
    let connector = OpenAIConnector::new(config).unwrap();

    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("Count from 1 to 5, each number on a new line.".to_string()),
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
                assert_eq!(finish_reason, lunaroute_core::normalized::FinishReason::Stop);
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
    };
    let connector = AnthropicConnector::new(config).unwrap();

    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("Count from 1 to 5, each number on a new line.".to_string()),
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
                assert_eq!(finish_reason, lunaroute_core::normalized::FinishReason::Stop);
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
    };
    let connector = OpenAIConnector::new(config).unwrap();

    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("What is 2+2?".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: Some("You are a helpful math tutor. Always answer in exactly one word or number.".to_string()),
        model: "gpt-5-mini".to_string(),
        max_tokens: Some(10),
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: true,
        tools: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    let mut stream = connector.stream(request).await.unwrap();

    let mut content = String::new();

    while let Some(event_result) = stream.next().await {
        let event = event_result.unwrap();

        if let lunaroute_core::normalized::NormalizedStreamEvent::Delta { delta, .. } = event
            && let Some(chunk) = delta.content {
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
        system: Some("You are a helpful math tutor. Always answer in exactly one word or number.".to_string()),
        model: "claude-sonnet-4-5".to_string(),
        max_tokens: Some(10),
        temperature: Some(0.0),
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: true,
        tools: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    };

    let mut stream = connector.stream(request).await.unwrap();

    let mut content = String::new();

    while let Some(event_result) = stream.next().await {
        let event = event_result.unwrap();

        if let lunaroute_core::normalized::NormalizedStreamEvent::Delta { delta, .. } = event
            && let Some(chunk) = delta.content {
                content.push_str(&chunk);
            }
    }

    // Response should be very short due to system prompt
    assert!(content.len() < 20);
    assert!(!content.is_empty());
}
