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

/// Convert Anthropic request to normalized request
pub fn to_normalized(req: AnthropicMessagesRequest) -> IngressResult<NormalizedRequest> {
    let messages: Result<Vec<Message>, IngressError> = req
        .messages
        .into_iter()
        .map(|msg| {
            let role = match msg.role.as_str() {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                _ => {
                    return Err(IngressError::InvalidRequest(format!(
                        "Invalid role: {}",
                        msg.role
                    )))
                }
            };

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
            FinishReason::ContentFilter => "content_filter".to_string(),
            FinishReason::Error => "error".to_string(),
        });

    AnthropicResponse {
        id: format!("msg_{}", resp.id),
        type_: "message".to_string(),
        role: "assistant".to_string(),
        content,
        model: resp.model,
        stop_reason,
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
}
