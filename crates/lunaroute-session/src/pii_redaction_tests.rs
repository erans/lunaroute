//! Tests for PII redaction in session recording

use crate::config::PIIConfig;
use crate::pii_redaction::SessionPIIRedactor;
use lunaroute_core::normalized::*;
use std::collections::HashMap;

#[test]
fn test_create_from_config() {
    let config = PIIConfig {
        enabled: true,
        detect_email: true,
        detect_phone: true,
        detect_ssn: false,
        detect_credit_card: false,
        detect_ip_address: false,
        min_confidence: 0.8,
        redaction_mode: "mask".to_string(),
        hmac_secret: None,
        partial_show_chars: 4,
        custom_patterns: vec![],
    };

    let redactor = SessionPIIRedactor::from_config(&config);
    assert!(redactor.is_ok());
}

#[test]
fn test_create_with_invalid_regex() {
    let config = PIIConfig {
        enabled: true,
        detect_email: true,
        detect_phone: false,
        detect_ssn: false,
        detect_credit_card: false,
        detect_ip_address: false,
        min_confidence: 0.7,
        redaction_mode: "mask".to_string(),
        hmac_secret: None,
        partial_show_chars: 4,
        custom_patterns: vec![crate::config::CustomPatternConfig {
            name: "invalid".to_string(),
            pattern: "[invalid(".to_string(), // Invalid regex
            confidence: 0.9,
            redaction_mode: "mask".to_string(),
            placeholder: None,
        }],
    };

    let redactor = SessionPIIRedactor::from_config(&config);
    assert!(redactor.is_err());
}

#[test]
fn test_redact_request_text() {
    let config = PIIConfig::default();
    let redactor = SessionPIIRedactor::from_config(&config).unwrap();

    let mut request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("My email is test@example.com".to_string()),
            name: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
        }],
        model: "gpt-4".to_string(),
        max_tokens: None,
        temperature: None,
        top_k: None,
        top_p: None,
        stream: false,
        tools: Vec::new(),
        tool_choice: None,
        stop_sequences: Vec::new(),
        system: None,
        metadata: HashMap::new(),
    };

    redactor.redact_request(&mut request);

    if let MessageContent::Text(text) = &request.messages[0].content {
        assert!(!text.contains("test@example.com"));
        assert!(text.contains("[EMAIL]"));
    } else {
        panic!("Expected text content");
    }
}

#[test]
fn test_redact_request_multimodal() {
    let config = PIIConfig::default();
    let redactor = SessionPIIRedactor::from_config(&config).unwrap();

    let mut request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Parts(vec![
                ContentPart::Text {
                    text: "My phone is 555-123-4567".to_string(),
                },
                ContentPart::Image {
                    source: ImageSource::Url {
                        url: "https://example.com/image.png".to_string(),
                    },
                },
            ]),
            name: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
        }],
        model: "gpt-4".to_string(),
        max_tokens: None,
        temperature: None,
        top_k: None,
        top_p: None,
        stream: false,
        tools: Vec::new(),
        tool_choice: None,
        stop_sequences: Vec::new(),
        system: None,
        metadata: HashMap::new(),
    };

    redactor.redact_request(&mut request);

    if let MessageContent::Parts(parts) = &request.messages[0].content {
        if let ContentPart::Text { text } = &parts[0] {
            assert!(!text.contains("555-123-4567"));
            assert!(text.contains("[PHONE]"));
        } else {
            panic!("Expected text part");
        }
    } else {
        panic!("Expected multimodal content");
    }
}

#[test]
fn test_redact_response() {
    let config = PIIConfig::default();
    let redactor = SessionPIIRedactor::from_config(&config).unwrap();

    let mut response = NormalizedResponse {
        id: "test".to_string(),
        model: "gpt-4".to_string(),
        created: 1234567890,
        choices: vec![Choice {
            index: 0,
            message: Message {
                role: Role::Assistant,
                content: MessageContent::Text("Call me at 555-987-6543".to_string()),
                name: None,
                tool_calls: Vec::new(),
                tool_call_id: None,
            },
            finish_reason: Some(FinishReason::Stop),
        }],
        usage: Usage {
            prompt_tokens: 10,
            completion_tokens: 20,
            total_tokens: 30,
        },
        metadata: HashMap::new(),
    };

    redactor.redact_response(&mut response);

    if let MessageContent::Text(text) = &response.choices[0].message.content {
        assert!(!text.contains("555-987-6543"));
        assert!(text.contains("[PHONE]"));
    } else {
        panic!("Expected text content");
    }
}

#[test]
fn test_redact_stream_event_delta() {
    let config = PIIConfig::default();
    let redactor = SessionPIIRedactor::from_config(&config).unwrap();

    let mut event = NormalizedStreamEvent::Delta {
        index: 0,
        delta: Delta {
            role: Some(Role::Assistant),
            content: Some("My SSN is 123-45-6789".to_string()),
        },
    };

    redactor.redact_stream_event(&mut event);

    if let NormalizedStreamEvent::Delta { delta, .. } = &event {
        if let Some(content) = &delta.content {
            assert!(!content.contains("123-45-6789"));
            assert!(content.contains("[SSN]"));
        } else {
            panic!("Expected delta content");
        }
    } else {
        panic!("Expected delta event");
    }
}

#[test]
fn test_redact_stream_event_tool_call_delta() {
    let config = PIIConfig::default();
    let redactor = SessionPIIRedactor::from_config(&config).unwrap();

    let mut event = NormalizedStreamEvent::ToolCallDelta {
        index: 0,
        tool_call_index: 0,
        id: Some("call_123".to_string()),
        function: Some(FunctionCallDelta {
            name: Some("send_email".to_string()),
            arguments: Some(r#"{"to": "user@example.com"}"#.to_string()),
        }),
    };

    redactor.redact_stream_event(&mut event);

    if let NormalizedStreamEvent::ToolCallDelta { function, .. } = &event {
        if let Some(func) = function {
            if let Some(args) = &func.arguments {
                assert!(!args.contains("user@example.com"));
                assert!(args.contains("[EMAIL]"));
            } else {
                panic!("Expected function arguments");
            }
        } else {
            panic!("Expected function");
        }
    } else {
        panic!("Expected tool call delta event");
    }
}

#[test]
fn test_redact_tool_calls_in_request() {
    let config = PIIConfig::default();
    let redactor = SessionPIIRedactor::from_config(&config).unwrap();

    let mut request = NormalizedRequest {
        messages: vec![Message {
            role: Role::Assistant,
            content: MessageContent::Text("I'll send an email".to_string()),
            name: None,
            tool_calls: vec![ToolCall {
                id: "call_123".to_string(),
                tool_type: "function".to_string(),
                function: FunctionCall {
                    name: "send_email".to_string(),
                    arguments: r#"{"to": "secret@example.com", "body": "Hello"}"#.to_string(),
                },
            }],
            tool_call_id: None,
        }],
        model: "gpt-4".to_string(),
        max_tokens: None,
        temperature: None,
        top_k: None,
        top_p: None,
        stream: false,
        tools: Vec::new(),
        tool_choice: None,
        stop_sequences: Vec::new(),
        system: None,
        metadata: HashMap::new(),
    };

    redactor.redact_request(&mut request);

    let args = &request.messages[0].tool_calls[0].function.arguments;
    assert!(!args.contains("secret@example.com"));
    assert!(args.contains("[EMAIL]"));
}

#[test]
fn test_remove_mode() {
    let config = PIIConfig {
        enabled: true,
        detect_email: true,
        detect_phone: false,
        detect_ssn: false,
        detect_credit_card: false,
        detect_ip_address: false,
        min_confidence: 0.7,
        redaction_mode: "remove".to_string(),
        hmac_secret: None,
        partial_show_chars: 4,
        custom_patterns: vec![],
    };

    let redactor = SessionPIIRedactor::from_config(&config).unwrap();

    let mut request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("Email: test@example.com".to_string()),
            name: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
        }],
        model: "gpt-4".to_string(),
        max_tokens: None,
        temperature: None,
        top_k: None,
        top_p: None,
        stream: false,
        tools: Vec::new(),
        tool_choice: None,
        stop_sequences: Vec::new(),
        system: None,
        metadata: HashMap::new(),
    };

    redactor.redact_request(&mut request);

    if let MessageContent::Text(text) = &request.messages[0].content {
        assert_eq!(text, "Email: "); // Email completely removed
    } else {
        panic!("Expected text content");
    }
}

#[test]
fn test_tokenize_mode_with_hmac() {
    let config = PIIConfig {
        enabled: true,
        detect_email: true,
        detect_phone: false,
        detect_ssn: false,
        detect_credit_card: false,
        detect_ip_address: false,
        min_confidence: 0.7,
        redaction_mode: "tokenize".to_string(),
        hmac_secret: Some("test-secret-key".to_string()),
        partial_show_chars: 4,
        custom_patterns: vec![],
    };

    let redactor = SessionPIIRedactor::from_config(&config).unwrap();

    let mut request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("Email: test@example.com".to_string()),
            name: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
        }],
        model: "gpt-4".to_string(),
        max_tokens: None,
        temperature: None,
        top_k: None,
        top_p: None,
        stream: false,
        tools: Vec::new(),
        tool_choice: None,
        stop_sequences: Vec::new(),
        system: None,
        metadata: HashMap::new(),
    };

    redactor.redact_request(&mut request);

    if let MessageContent::Text(text) = &request.messages[0].content {
        assert!(!text.contains("test@example.com"));
        assert!(text.contains("[EM:")); // Tokenized format
        assert!(text.ends_with("]"));
    } else {
        panic!("Expected text content");
    }
}

#[test]
fn test_custom_pattern() {
    let config = PIIConfig {
        enabled: true,
        detect_email: false,
        detect_phone: false,
        detect_ssn: false,
        detect_credit_card: false,
        detect_ip_address: false,
        min_confidence: 0.7,
        redaction_mode: "mask".to_string(),
        hmac_secret: None,
        partial_show_chars: 4,
        custom_patterns: vec![crate::config::CustomPatternConfig {
            name: "api_key".to_string(),
            pattern: r"sk-[a-zA-Z0-9]{32}".to_string(),
            confidence: 0.95,
            redaction_mode: "mask".to_string(),
            placeholder: Some("[API_KEY]".to_string()),
        }],
    };

    let redactor = SessionPIIRedactor::from_config(&config).unwrap();

    let mut request = NormalizedRequest {
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("Key: sk-abcdefghijklmnopqrstuvwxyz123456".to_string()),
            name: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
        }],
        model: "gpt-4".to_string(),
        max_tokens: None,
        temperature: None,
        top_k: None,
        top_p: None,
        stream: false,
        tools: Vec::new(),
        tool_choice: None,
        stop_sequences: Vec::new(),
        system: None,
        metadata: HashMap::new(),
    };

    redactor.redact_request(&mut request);

    if let MessageContent::Text(text) = &request.messages[0].content {
        assert!(!text.contains("sk-abcdefghijklmnopqrstuvwxyz123456"));
        assert!(text.contains("[API_KEY]"));
    } else {
        panic!("Expected text content");
    }
}

// Security tests

#[test]
fn test_json_structure_preservation() {
    let config = PIIConfig::default();
    let redactor = SessionPIIRedactor::from_config(&config).unwrap();

    let mut request = NormalizedRequest {
        messages: vec![Message {
            role: Role::Assistant,
            content: MessageContent::Text("Sending email".to_string()),
            name: None,
            tool_calls: vec![ToolCall {
                id: "call_123".to_string(),
                tool_type: "function".to_string(),
                function: FunctionCall {
                    name: "send_email".to_string(),
                    arguments: r#"{"to":"user@example.com","subject":"Hello","body":"Test message","cc":["admin@example.com"]}"#.to_string(),
                },
            }],
            tool_call_id: None,
        }],
        model: "gpt-4".to_string(),
        max_tokens: None,
        temperature: None,
        top_k: None,
        top_p: None,
        stream: false,
        tools: Vec::new(),
        tool_choice: None,
        stop_sequences: Vec::new(),
        system: None,
        metadata: HashMap::new(),
    };

    redactor.redact_request(&mut request);

    let args = &request.messages[0].tool_calls[0].function.arguments;

    // Should still be valid JSON
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(args);
    assert!(parsed.is_ok(), "Redacted JSON should still be valid JSON");

    // Check that emails are redacted
    assert!(!args.contains("user@example.com"));
    assert!(!args.contains("admin@example.com"));
    assert!(args.contains("[EMAIL]"));
}

#[test]
fn test_json_with_nested_structures() {
    let config = PIIConfig {
        detect_phone: true,
        ..Default::default()
    };
    let redactor = SessionPIIRedactor::from_config(&config).unwrap();

    let mut request = NormalizedRequest {
        messages: vec![Message {
            role: Role::Assistant,
            content: MessageContent::Text("Creating user".to_string()),
            name: None,
            tool_calls: vec![ToolCall {
                id: "call_456".to_string(),
                tool_type: "function".to_string(),
                function: FunctionCall {
                    name: "create_user".to_string(),
                    arguments: r#"{"user":{"email":"john@example.com","phone":"555-123-1234","metadata":{"backup_email":"john.doe@work.com"}}}"#.to_string(),
                },
            }],
            tool_call_id: None,
        }],
        model: "gpt-4".to_string(),
        max_tokens: None,
        temperature: None,
        top_k: None,
        top_p: None,
        stream: false,
        tools: Vec::new(),
        tool_choice: None,
        stop_sequences: Vec::new(),
        system: None,
        metadata: HashMap::new(),
    };

    redactor.redact_request(&mut request);

    let args = &request.messages[0].tool_calls[0].function.arguments;

    // Should still be valid JSON
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(args);
    assert!(parsed.is_ok(), "Redacted nested JSON should still be valid");

    // Check that all PII is redacted
    assert!(!args.contains("john@example.com"));
    assert!(!args.contains("john.doe@work.com"));
    assert!(!args.contains("555-123-1234"));
}

#[test]
fn test_malformed_json_fallback() {
    let config = PIIConfig {
        detect_phone: true,
        ..Default::default()
    };
    let redactor = SessionPIIRedactor::from_config(&config).unwrap();

    let mut request = NormalizedRequest {
        messages: vec![Message {
            role: Role::Assistant,
            content: MessageContent::Text("Calling function".to_string()),
            name: None,
            tool_calls: vec![ToolCall {
                id: "call_789".to_string(),
                tool_type: "function".to_string(),
                function: FunctionCall {
                    name: "some_func".to_string(),
                    // Invalid JSON - should fall back to string redaction
                    arguments: "email: test@example.com, phone: 555-123-9999".to_string(),
                },
            }],
            tool_call_id: None,
        }],
        model: "gpt-4".to_string(),
        max_tokens: None,
        temperature: None,
        top_k: None,
        top_p: None,
        stream: false,
        tools: Vec::new(),
        tool_choice: None,
        stop_sequences: Vec::new(),
        system: None,
        metadata: HashMap::new(),
    };

    redactor.redact_request(&mut request);

    let args = &request.messages[0].tool_calls[0].function.arguments;

    // Should still redact PII even if not valid JSON
    assert!(!args.contains("test@example.com"));
    assert!(!args.contains("555-123-9999"));
    assert!(args.contains("[EMAIL]"));
    assert!(args.contains("[PHONE]"));
}
