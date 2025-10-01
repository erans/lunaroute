//! Anthropic ingress adapter

use crate::types::{IngressError, IngressResult};
use axum::{
    extract::Json,
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use lunaroute_core::normalized::{
    FinishReason, Message, MessageContent, NormalizedRequest, NormalizedResponse, Role,
};
#[cfg(test)]
use lunaroute_core::normalized::{Choice, Usage};
use serde::{Deserialize, Serialize};

/// Anthropic messages request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessagesRequest {
    pub model: String,
    pub messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
}

/// Anthropic message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessage {
    pub role: String,
    pub content: String,
}

/// Anthropic response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub role: String,
    pub content: Vec<AnthropicContent>,
    pub model: String,
    pub stop_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequence: Option<String>,
    pub usage: AnthropicUsage,
}

/// Anthropic content block
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicContent {
    #[serde(rename = "text")]
    Text { text: String },
}

/// Anthropic usage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Validate Anthropic request parameters
fn validate_request(req: &AnthropicMessagesRequest) -> IngressResult<()> {
    // Validate temperature (0.0 to 1.0 for Anthropic)
    if let Some(temp) = req.temperature {
        if temp < 0.0 || temp > 1.0 {
            return Err(IngressError::InvalidRequest(
                format!("temperature must be between 0.0 and 1.0, got {}", temp)
            ));
        }
    }

    // Validate top_p (0.0 to 1.0)
    if let Some(top_p) = req.top_p {
        if top_p < 0.0 || top_p > 1.0 {
            return Err(IngressError::InvalidRequest(
                format!("top_p must be between 0.0 and 1.0, got {}", top_p)
            ));
        }
    }

    // Validate top_k (positive integer)
    if let Some(top_k) = req.top_k {
        if top_k == 0 {
            return Err(IngressError::InvalidRequest(
                "top_k must be greater than 0".to_string()
            ));
        }
    }

    // Validate max_tokens (required and positive for Anthropic)
    if let Some(max_tokens) = req.max_tokens {
        if max_tokens == 0 {
            return Err(IngressError::InvalidRequest(
                "max_tokens must be greater than 0".to_string()
            ));
        }
        if max_tokens > 100000 {
            return Err(IngressError::InvalidRequest(
                format!("max_tokens must be <= 100000, got {}", max_tokens)
            ));
        }
    }

    // Validate model name is not empty and length
    if req.model.is_empty() {
        return Err(IngressError::InvalidRequest(
            "model field cannot be empty".to_string()
        ));
    }
    if req.model.len() > 256 {
        return Err(IngressError::InvalidRequest(
            format!("model name too long: {} chars (max 256)", req.model.len())
        ));
    }

    // Validate messages array is not empty
    if req.messages.is_empty() {
        return Err(IngressError::InvalidRequest(
            "messages array cannot be empty".to_string()
        ));
    }

    // Validate messages array length (max 100,000 per Anthropic spec)
    if req.messages.len() > 100_000 {
        return Err(IngressError::InvalidRequest(
            format!("messages array too large: {} messages (max 100,000)", req.messages.len())
        ));
    }

    Ok(())
}

/// Convert Anthropic request to normalized request
pub fn to_normalized(req: AnthropicMessagesRequest) -> IngressResult<NormalizedRequest> {
    // Validate request parameters first
    validate_request(&req)?;

    let messages: Result<Vec<Message>, IngressError> = req
        .messages
        .into_iter()
        .map(|msg| {
            let role = match msg.role.as_str() {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                _ => {
                    return Err(IngressError::InvalidRequest(format!(
                        "Invalid role: {} (Anthropic only supports 'user' and 'assistant')",
                        msg.role
                    )))
                }
            };

            // Validate message content length
            if msg.content.len() > 1_000_000 {
                return Err(IngressError::InvalidRequest(
                    format!("Message content too large: {} bytes (max 1MB)", msg.content.len())
                ));
            }

            Ok(Message {
                role,
                content: MessageContent::Text(msg.content),
                name: None,
                tool_calls: vec![],
                tool_call_id: None,
            })
        })
        .collect();

    Ok(NormalizedRequest {
        messages: messages?,
        system: req.system,
        model: req.model,
        max_tokens: req.max_tokens,
        temperature: req.temperature,
        top_p: req.top_p,
        top_k: req.top_k,
        stop_sequences: req.stop_sequences.unwrap_or_default(),
        stream: req.stream.unwrap_or(false),
        tools: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    })
}

/// Convert normalized response to Anthropic response
pub fn from_normalized(resp: NormalizedResponse) -> AnthropicResponse {
    let content: Vec<AnthropicContent> = resp
        .choices
        .first()
        .map(|choice| match &choice.message.content {
            MessageContent::Text(text) => vec![AnthropicContent::Text {
                text: text.clone(),
            }],
            MessageContent::Parts(_) => vec![],
        })
        .unwrap_or_default();

    let stop_reason = resp
        .choices
        .first()
        .and_then(|choice| choice.finish_reason.as_ref())
        .map(|fr| match fr {
            FinishReason::Stop => "end_turn".to_string(),
            FinishReason::Length => "max_tokens".to_string(),
            FinishReason::ToolCalls => "tool_use".to_string(),
            // ContentFilter doesn't exist in Anthropic spec - map to end_turn
            FinishReason::ContentFilter => "end_turn".to_string(),
            // Error is not in spec but useful for debugging
            FinishReason::Error => "end_turn".to_string(),
        });

    AnthropicResponse {
        id: format!("msg_{}", resp.id),
        type_: "message".to_string(),
        role: "assistant".to_string(),
        content,
        model: resp.model,
        stop_reason,
        stop_sequence: None, // TODO: Track which stop sequence was hit
        usage: AnthropicUsage {
            input_tokens: resp.usage.prompt_tokens,
            output_tokens: resp.usage.completion_tokens,
        },
    }
}

/// Messages handler (placeholder - actual routing will be implemented later)
pub async fn messages(
    Json(req): Json<AnthropicMessagesRequest>,
) -> Result<Response, IngressError> {
    // Convert to normalized format
    let normalized = to_normalized(req.clone())?;

    // For now, return a placeholder response
    let response = AnthropicResponse {
        id: "msg_placeholder".to_string(),
        type_: "message".to_string(),
        role: "assistant".to_string(),
        content: vec![AnthropicContent::Text {
            text: "This is a placeholder response".to_string(),
        }],
        model: normalized.model.clone(),
        stop_reason: Some("end_turn".to_string()),
        stop_sequence: None,
        usage: AnthropicUsage {
            input_tokens: 10,
            output_tokens: 5,
        },
    };

    Ok(Json(response).into_response())
}

/// Create Anthropic router
pub fn router() -> Router {
    Router::new().route("/v1/messages", post(messages))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_normalized() {
        let req = AnthropicMessagesRequest {
            model: "claude-3-opus".to_string(),
            messages: vec![
                AnthropicMessage {
                    role: "user".to_string(),
                    content: "Hello!".to_string(),
                },
            ],
            system: Some("You are a helpful assistant".to_string()),
            max_tokens: Some(1024),
            temperature: Some(0.7),
            top_p: None,
            top_k: None,
            stream: Some(false),
            stop_sequences: None,
        };

        let normalized = to_normalized(req).unwrap();
        assert_eq!(normalized.model, "claude-3-opus");
        assert_eq!(normalized.messages.len(), 1);
        assert_eq!(normalized.system, Some("You are a helpful assistant".to_string()));
        assert_eq!(normalized.max_tokens, Some(1024));
    }

    #[test]
    fn test_from_normalized() {
        let resp = NormalizedResponse {
            id: "test-123".to_string(),
            model: "claude-3-opus".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: MessageContent::Text("Hello, world!".to_string()),
                    name: None,
                    tool_calls: vec![],
                    tool_call_id: None,
                },
                finish_reason: Some(FinishReason::Stop),
            }],
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
            created: 1234567890,
            metadata: std::collections::HashMap::new(),
        };

        let anthropic = from_normalized(resp);
        assert_eq!(anthropic.model, "claude-3-opus");
        assert_eq!(anthropic.role, "assistant");
        assert_eq!(anthropic.stop_reason, Some("end_turn".to_string()));
        assert_eq!(anthropic.usage.input_tokens, 10);
        assert_eq!(anthropic.usage.output_tokens, 5);
    }

    #[test]
    fn test_invalid_role() {
        let req = AnthropicMessagesRequest {
            model: "claude-3-opus".to_string(),
            messages: vec![AnthropicMessage {
                role: "invalid".to_string(),
                content: "test".to_string(),
            }],
            system: None,
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stream: None,
            stop_sequences: None,
        };

        let result = to_normalized(req);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_messages_endpoint() {
        let req = AnthropicMessagesRequest {
            model: "claude-3-opus".to_string(),
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: "Hello!".to_string(),
            }],
            system: None,
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stream: None,
            stop_sequences: None,
        };

        let response = messages(Json(req)).await;
        assert!(response.is_ok());
    }

    #[test]
    fn test_all_anthropic_finish_reasons() {
        // Test all finish reason mappings for Anthropic
        let finish_reasons = vec![
            (FinishReason::Stop, "end_turn"),
            (FinishReason::Length, "max_tokens"),
            (FinishReason::ToolCalls, "tool_use"),
            // ContentFilter and Error both map to end_turn for Anthropic
            (FinishReason::ContentFilter, "end_turn"),
            (FinishReason::Error, "end_turn"),
        ];

        for (reason, expected) in finish_reasons {
            let resp = NormalizedResponse {
                id: "test".to_string(),
                model: "claude-3-opus".to_string(),
                choices: vec![Choice {
                    index: 0,
                    message: Message {
                        role: Role::Assistant,
                        content: MessageContent::Text("test".to_string()),
                        name: None,
                        tool_calls: vec![],
                        tool_call_id: None,
                    },
                    finish_reason: Some(reason),
                }],
                usage: Usage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    total_tokens: 15,
                },
                created: 1234567890,
                metadata: std::collections::HashMap::new(),
            };

            let anthropic = from_normalized(resp);
            assert_eq!(anthropic.stop_reason, Some(expected.to_string()));
        }
    }

    #[test]
    fn test_anthropic_empty_messages() {
        let req = AnthropicMessagesRequest {
            model: "claude-3-opus".to_string(),
            messages: vec![],
            system: None,
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stream: None,
            stop_sequences: None,
        };

        // Validation should reject empty messages array
        let result = to_normalized(req);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("messages array cannot be empty"));
    }

    #[test]
    fn test_anthropic_with_system_prompt() {
        let req = AnthropicMessagesRequest {
            model: "claude-3-opus".to_string(),
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: "Hello!".to_string(),
            }],
            system: Some("You are a helpful assistant".to_string()),
            max_tokens: Some(1024),
            temperature: Some(0.7),
            top_p: Some(0.9),
            top_k: Some(40),
            stream: Some(true),
            stop_sequences: Some(vec!["STOP".to_string()]),
        };

        let normalized = to_normalized(req).unwrap();
        assert_eq!(normalized.system, Some("You are a helpful assistant".to_string()));
        assert_eq!(normalized.max_tokens, Some(1024));
        assert_eq!(normalized.temperature, Some(0.7));
        assert_eq!(normalized.top_p, Some(0.9));
        assert_eq!(normalized.top_k, Some(40));
        assert_eq!(normalized.stream, true);
        assert_eq!(normalized.stop_sequences, vec!["STOP".to_string()]);
    }

    #[test]
    fn test_anthropic_stop_sequence_field() {
        let resp = NormalizedResponse {
            id: "test".to_string(),
            model: "claude-3-opus".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: MessageContent::Text("test".to_string()),
                    name: None,
                    tool_calls: vec![],
                    tool_call_id: None,
                },
                finish_reason: Some(FinishReason::Stop),
            }],
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
            created: 1234567890,
            metadata: std::collections::HashMap::new(),
        };

        let anthropic = from_normalized(resp);
        // Currently returns None - will be populated when tracking is implemented
        assert_eq!(anthropic.stop_sequence, None);
    }

    #[test]
    fn test_anthropic_multimodal_parts() {
        use lunaroute_core::normalized::ContentPart;

        let resp = NormalizedResponse {
            id: "test".to_string(),
            model: "claude-3-opus".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: MessageContent::Parts(vec![
                        ContentPart::Text {
                            text: "I see an image".to_string(),
                        },
                    ]),
                    name: None,
                    tool_calls: vec![],
                    tool_call_id: None,
                },
                finish_reason: Some(FinishReason::Stop),
            }],
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
            created: 1234567890,
            metadata: std::collections::HashMap::new(),
        };

        let anthropic = from_normalized(resp);
        // Currently returns empty vec for Parts - this is a known limitation
        assert_eq!(anthropic.content.len(), 0);
    }

    #[test]
    fn test_anthropic_router_creation() {
        let router = router();
        // Just verify it creates without panicking
        // The router is properly configured with /v1/messages endpoint
        drop(router);
    }
}
