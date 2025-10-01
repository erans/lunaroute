//! Tests for normalized types

use super::*;

#[test]
fn test_normalized_request_text_message() {
    let request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("Hello, world!".to_string()),
            name: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: Some("You are a helpful assistant".to_string()),
        model: "gpt-4".to_string(),
        max_tokens: Some(1000),
        temperature: Some(0.7),
        top_p: Some(0.9),
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_choice: None,
        metadata: HashMap::new(),
    };

    // Test serialization
    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("Hello, world!"));
    assert!(json.contains("gpt-4"));

    // Test deserialization
    let deserialized: NormalizedRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.model, "gpt-4");
    assert_eq!(deserialized.messages.len(), 1);
}

#[test]
fn test_multimodal_message() {
    let message = Message {
        role: Role::User,
        content: MessageContent::Parts(vec![
            ContentPart::Text {
                text: "What's in this image?".to_string(),
            },
            ContentPart::Image {
                source: ImageSource::Url {
                    url: "https://example.com/image.jpg".to_string(),
                },
            },
        ]),
        name: None,
        tool_calls: vec![],
        tool_call_id: None,
    };

    let json = serde_json::to_string(&message).unwrap();
    let deserialized: Message = serde_json::from_str(&json).unwrap();

    match deserialized.content {
        MessageContent::Parts(parts) => {
            assert_eq!(parts.len(), 2);
            matches!(parts[0], ContentPart::Text { .. });
            matches!(parts[1], ContentPart::Image { .. });
        }
        _ => panic!("Expected Parts content"),
    }
}

#[test]
fn test_base64_image() {
    let image_source = ImageSource::Base64 {
        media_type: "image/png".to_string(),
        data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==".to_string(),
    };

    let json = serde_json::to_string(&image_source).unwrap();
    let deserialized: ImageSource = serde_json::from_str(&json).unwrap();

    match deserialized {
        ImageSource::Base64 { media_type, .. } => {
            assert_eq!(media_type, "image/png");
        }
        _ => panic!("Expected Base64 image source"),
    }
}

#[test]
fn test_tool_definition() {
    let tool = Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "get_weather".to_string(),
            description: Some("Get the current weather".to_string()),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "location": {
                        "type": "string",
                        "description": "City name"
                    }
                },
                "required": ["location"]
            }),
        },
    };

    let json = serde_json::to_string(&tool).unwrap();
    let deserialized: Tool = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.function.name, "get_weather");
    assert!(deserialized.function.description.is_some());
}

#[test]
fn test_tool_call() {
    let tool_call = ToolCall {
        id: "call_123".to_string(),
        tool_type: "function".to_string(),
        function: FunctionCall {
            name: "get_weather".to_string(),
            arguments: r#"{"location":"San Francisco"}"#.to_string(),
        },
    };

    let json = serde_json::to_string(&tool_call).unwrap();
    let deserialized: ToolCall = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.id, "call_123");
    assert_eq!(deserialized.function.name, "get_weather");
}

#[test]
fn test_normalized_response() {
    let response = NormalizedResponse {
        id: "resp_123".to_string(),
        model: "gpt-4".to_string(),
        choices: vec![Choice {
            index: 0,
            message: Message {
                role: Role::Assistant,
                content: MessageContent::Text("Hello!".to_string()),
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
    };

    let json = serde_json::to_string(&response).unwrap();
    let deserialized: NormalizedResponse = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.id, "resp_123");
    assert_eq!(deserialized.usage.total_tokens, 30);
    assert_eq!(deserialized.choices.len(), 1);
}

#[test]
fn test_stream_events() {
    let events = vec![
        NormalizedStreamEvent::Start {
            id: "stream_123".to_string(),
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
                content: Some(" world!".to_string()),
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

    for event in events {
        let json = serde_json::to_string(&event).unwrap();
        let _deserialized: NormalizedStreamEvent = serde_json::from_str(&json).unwrap();
    }
}

#[test]
fn test_tool_call_delta() {
    let event = NormalizedStreamEvent::ToolCallDelta {
        index: 0,
        tool_call_index: 0,
        id: Some("call_123".to_string()),
        function: Some(FunctionCallDelta {
            name: Some("get_weather".to_string()),
            arguments: Some(r#"{"location":"#.to_string()),
        }),
    };

    let json = serde_json::to_string(&event).unwrap();
    let deserialized: NormalizedStreamEvent = serde_json::from_str(&json).unwrap();

    match deserialized {
        NormalizedStreamEvent::ToolCallDelta {
            id,
            function,
            ..
        } => {
            assert_eq!(id, Some("call_123".to_string()));
            assert!(function.is_some());
        }
        _ => panic!("Expected ToolCallDelta"),
    }
}

#[test]
fn test_finish_reasons() {
    let reasons = vec![
        FinishReason::Stop,
        FinishReason::Length,
        FinishReason::ToolCalls,
        FinishReason::ContentFilter,
        FinishReason::Error,
    ];

    for reason in reasons {
        let json = serde_json::to_string(&reason).unwrap();
        let _deserialized: FinishReason = serde_json::from_str(&json).unwrap();
    }
}

#[test]
fn test_role_serialization() {
    let roles = vec![Role::System, Role::User, Role::Assistant, Role::Tool];

    for role in roles {
        let json = serde_json::to_string(&role).unwrap();
        let deserialized: Role = serde_json::from_str(&json).unwrap();
        assert_eq!(role, deserialized);
    }

    // Test specific serialization format
    assert_eq!(
        serde_json::to_string(&Role::User).unwrap(),
        r#""user""#
    );
}

#[test]
fn test_tool_choice_variants() {
    let choices = vec![
        ToolChoice::Auto,
        ToolChoice::Required,
        ToolChoice::None,
        ToolChoice::Specific {
            name: "get_weather".to_string(),
        },
    ];

    for choice in choices {
        let json = serde_json::to_string(&choice).unwrap();
        let _deserialized: ToolChoice = serde_json::from_str(&json).unwrap();
    }
}

#[test]
fn test_empty_request() {
    let request = NormalizedRequest {
        messages: vec![],
        system: None,
        model: "test-model".to_string(),
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        stream: false,
        tools: vec![],
        tool_choice: None,
        metadata: HashMap::new(),
    };

    let json = serde_json::to_string(&request).unwrap();
    let deserialized: NormalizedRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.messages.len(), 0);
}

#[test]
fn test_usage_tracking() {
    let usage = Usage {
        prompt_tokens: 100,
        completion_tokens: 50,
        total_tokens: 150,
    };

    assert_eq!(usage.total_tokens, usage.prompt_tokens + usage.completion_tokens);

    let json = serde_json::to_string(&usage).unwrap();
    let deserialized: Usage = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.total_tokens, 150);
}
