//! OpenAI ingress adapter

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

/// OpenAI chat completion request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIChatRequest {
    pub model: String,
    pub messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

/// OpenAI message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// OpenAI chat completion response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIChatResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<OpenAIChoice>,
    pub usage: OpenAIUsage,
}

/// OpenAI choice
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIChoice {
    pub index: u32,
    pub message: OpenAIMessage,
    pub finish_reason: Option<String>,
}

/// OpenAI usage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// OpenAI stream chunk
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIStreamChunk {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<OpenAIStreamChoice>,
}

/// OpenAI stream choice
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIStreamChoice {
    pub index: u32,
    pub delta: OpenAIDelta,
    pub finish_reason: Option<String>,
}

/// OpenAI delta
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// Validate OpenAI request parameters
fn validate_request(req: &OpenAIChatRequest) -> IngressResult<()> {
    // Validate temperature (0.0 to 2.0 for OpenAI)
    if let Some(temp) = req.temperature {
        if temp < 0.0 || temp > 2.0 {
            return Err(IngressError::InvalidRequest(
                format!("temperature must be between 0.0 and 2.0, got {}", temp)
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

    // Validate max_tokens (positive integer)
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

    // Validate presence_penalty (-2.0 to 2.0)
    if let Some(penalty) = req.presence_penalty {
        if penalty < -2.0 || penalty > 2.0 {
            return Err(IngressError::InvalidRequest(
                format!("presence_penalty must be between -2.0 and 2.0, got {}", penalty)
            ));
        }
    }

    // Validate frequency_penalty (-2.0 to 2.0)
    if let Some(penalty) = req.frequency_penalty {
        if penalty < -2.0 || penalty > 2.0 {
            return Err(IngressError::InvalidRequest(
                format!("frequency_penalty must be between -2.0 and 2.0, got {}", penalty)
            ));
        }
    }

    // Validate n (number of completions)
    if let Some(n) = req.n {
        if n == 0 || n > 10 {
            return Err(IngressError::InvalidRequest(
                format!("n must be between 1 and 10, got {}", n)
            ));
        }
    }

    // Validate model name is not empty
    if req.model.is_empty() {
        return Err(IngressError::InvalidRequest(
            "model field cannot be empty".to_string()
        ));
    }

    // Validate messages array is not empty
    if req.messages.is_empty() {
        return Err(IngressError::InvalidRequest(
            "messages array cannot be empty".to_string()
        ));
    }

    Ok(())
}

/// Convert OpenAI request to normalized request
pub fn to_normalized(req: OpenAIChatRequest) -> IngressResult<NormalizedRequest> {
    // Validate request parameters first
    validate_request(&req)?;

    let messages: Result<Vec<Message>, IngressError> = req
        .messages
        .into_iter()
        .map(|msg| {
            let role = match msg.role.as_str() {
                "system" => Role::System,
                "user" => Role::User,
                "assistant" => Role::Assistant,
                "tool" => Role::Tool,
                _ => {
                    return Err(IngressError::InvalidRequest(format!(
                        "Invalid role: {}",
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
                name: msg.name,
                tool_calls: vec![],
                tool_call_id: None,
            })
        })
        .collect();

    Ok(NormalizedRequest {
        messages: messages?,
        system: None,
        model: req.model,
        max_tokens: req.max_tokens,
        temperature: req.temperature,
        top_p: req.top_p,
        top_k: None,
        stop_sequences: req.stop.unwrap_or_default(),
        stream: req.stream.unwrap_or(false),
        tools: vec![],
        tool_choice: None,
        metadata: std::collections::HashMap::new(),
    })
}

/// Convert normalized response to OpenAI response
pub fn from_normalized(resp: NormalizedResponse) -> OpenAIChatResponse {
    let choices: Vec<OpenAIChoice> = resp
        .choices
        .into_iter()
        .map(|choice| {
            let content = match choice.message.content {
                MessageContent::Text(text) => text,
                MessageContent::Parts(_parts) => {
                    // For now, just return empty string for Parts
                    // TODO: Handle multimodal content properly
                    String::new()
                }
            };

            let finish_reason = choice.finish_reason.map(|fr| match fr {
                FinishReason::Stop => "stop".to_string(),
                FinishReason::Length => "length".to_string(),
                FinishReason::ToolCalls => "tool_calls".to_string(),
                FinishReason::ContentFilter => "content_filter".to_string(),
                FinishReason::Error => "error".to_string(),
            });

            OpenAIChoice {
                index: choice.index,
                message: OpenAIMessage {
                    role: "assistant".to_string(),
                    content,
                    name: None,
                },
                finish_reason,
            }
        })
        .collect();

    OpenAIChatResponse {
        id: format!("chatcmpl-{}", resp.id),
        object: "chat.completion".to_string(),
        created: resp.created,
        model: resp.model,
        choices,
        usage: OpenAIUsage {
            prompt_tokens: resp.usage.prompt_tokens,
            completion_tokens: resp.usage.completion_tokens,
            total_tokens: resp.usage.total_tokens,
        },
    }
}

/// Chat completion handler (placeholder - actual routing will be implemented later)
pub async fn chat_completions(
    Json(req): Json<OpenAIChatRequest>,
) -> Result<Response, IngressError> {
    // Convert to normalized format
    let normalized = to_normalized(req.clone())?;

    // For now, return a placeholder response
    // In production, this would route through the gateway
    let response = OpenAIChatResponse {
        id: "chatcmpl-placeholder".to_string(),
        object: "chat.completion".to_string(),
        created: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_else(|_| std::time::Duration::from_secs(0))
            .as_secs() as i64,
        model: normalized.model.clone(),
        choices: vec![OpenAIChoice {
            index: 0,
            message: OpenAIMessage {
                role: "assistant".to_string(),
                content: "This is a placeholder response".to_string(),
                name: None,
            },
            finish_reason: Some("stop".to_string()),
        }],
        usage: OpenAIUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
        },
    };

    Ok(Json(response).into_response())
}

/// Create OpenAI router
pub fn router() -> Router {
    Router::new().route("/v1/chat/completions", post(chat_completions))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_normalized() {
        let req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![
                OpenAIMessage {
                    role: "system".to_string(),
                    content: "You are a helpful assistant".to_string(),
                    name: None,
                },
                OpenAIMessage {
                    role: "user".to_string(),
                    content: "Hello!".to_string(),
                    name: None,
                },
            ],
            temperature: Some(0.7),
            top_p: None,
            max_tokens: Some(100),
            stream: Some(false),
            stop: None,
            n: None,
            presence_penalty: None,
            frequency_penalty: None,
            user: None,
        };

        let normalized = to_normalized(req).unwrap();
        assert_eq!(normalized.model, "gpt-4");
        assert_eq!(normalized.messages.len(), 2);
        assert_eq!(normalized.temperature, Some(0.7));
        assert_eq!(normalized.max_tokens, Some(100));
    }

    #[test]
    fn test_from_normalized() {
        let resp = NormalizedResponse {
            id: "test-123".to_string(),
            model: "gpt-4".to_string(),
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

        let openai = from_normalized(resp);
        assert_eq!(openai.model, "gpt-4");
        assert_eq!(openai.choices[0].message.content, "Hello, world!");
        assert_eq!(openai.usage.total_tokens, 15);
        assert_eq!(openai.choices[0].finish_reason, Some("stop".to_string()));
    }

    #[test]
    fn test_invalid_role() {
        let req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![OpenAIMessage {
                role: "invalid".to_string(),
                content: "test".to_string(),
                name: None,
            }],
            temperature: None,
            top_p: None,
            max_tokens: None,
            stream: None,
            stop: None,
            n: None,
            presence_penalty: None,
            frequency_penalty: None,
            user: None,
        };

        let result = to_normalized(req);
        assert!(result.is_err());
    }

    #[test]
    fn test_tool_role() {
        let req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![
                OpenAIMessage {
                    role: "user".to_string(),
                    content: "What's the weather?".to_string(),
                    name: None,
                },
                OpenAIMessage {
                    role: "assistant".to_string(),
                    content: "".to_string(),
                    name: None,
                },
                OpenAIMessage {
                    role: "tool".to_string(),
                    content: "Sunny, 72°F".to_string(),
                    name: Some("get_weather".to_string()),
                },
            ],
            temperature: None,
            top_p: None,
            max_tokens: None,
            stream: None,
            stop: None,
            n: None,
            presence_penalty: None,
            frequency_penalty: None,
            user: None,
        };

        let normalized = to_normalized(req).unwrap();
        assert_eq!(normalized.messages.len(), 3);
        assert_eq!(normalized.messages[2].role, Role::Tool);
        assert_eq!(normalized.messages[2].name, Some("get_weather".to_string()));
    }

    #[tokio::test]
    async fn test_chat_completions_endpoint() {
        let req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![OpenAIMessage {
                role: "user".to_string(),
                content: "Hello!".to_string(),
                name: None,
            }],
            temperature: None,
            top_p: None,
            max_tokens: None,
            stream: None,
            stop: None,
            n: None,
            presence_penalty: None,
            frequency_penalty: None,
            user: None,
        };

        let response = chat_completions(Json(req)).await;
        assert!(response.is_ok());
    }

    #[test]
    fn test_all_finish_reasons() {
        // Test all finish reason mappings
        let finish_reasons = vec![
            (FinishReason::Stop, "stop"),
            (FinishReason::Length, "length"),
            (FinishReason::ToolCalls, "tool_calls"),
            (FinishReason::ContentFilter, "content_filter"),
            (FinishReason::Error, "error"),
        ];

        for (reason, expected) in finish_reasons {
            let resp = NormalizedResponse {
                id: "test".to_string(),
                model: "gpt-4".to_string(),
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

            let openai = from_normalized(resp);
            assert_eq!(openai.choices[0].finish_reason, Some(expected.to_string()));
        }
    }

    #[test]
    fn test_multiple_choices() {
        let resp = NormalizedResponse {
            id: "test".to_string(),
            model: "gpt-4".to_string(),
            choices: vec![
                Choice {
                    index: 0,
                    message: Message {
                        role: Role::Assistant,
                        content: MessageContent::Text("First choice".to_string()),
                        name: None,
                        tool_calls: vec![],
                        tool_call_id: None,
                    },
                    finish_reason: Some(FinishReason::Stop),
                },
                Choice {
                    index: 1,
                    message: Message {
                        role: Role::Assistant,
                        content: MessageContent::Text("Second choice".to_string()),
                        name: None,
                        tool_calls: vec![],
                        tool_call_id: None,
                    },
                    finish_reason: Some(FinishReason::Stop),
                },
            ],
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 10,
                total_tokens: 20,
            },
            created: 1234567890,
            metadata: std::collections::HashMap::new(),
        };

        let openai = from_normalized(resp);
        assert_eq!(openai.choices.len(), 2);
        assert_eq!(openai.choices[0].index, 0);
        assert_eq!(openai.choices[1].index, 1);
        assert_eq!(openai.choices[0].message.content, "First choice");
        assert_eq!(openai.choices[1].message.content, "Second choice");
    }

    #[test]
    fn test_multimodal_parts_content() {
        use lunaroute_core::normalized::ContentPart;

        let resp = NormalizedResponse {
            id: "test".to_string(),
            model: "gpt-4-vision".to_string(),
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

        let openai = from_normalized(resp);
        // Currently returns empty string for Parts - this is a known limitation
        assert_eq!(openai.choices[0].message.content, "");
    }

    #[test]
    fn test_message_with_all_optional_fields() {
        let req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![OpenAIMessage {
                role: "user".to_string(),
                content: "Hello".to_string(),
                name: Some("Alice".to_string()),
            }],
            temperature: Some(0.8),
            top_p: Some(0.9),
            max_tokens: Some(500),
            stream: Some(true),
            stop: Some(vec!["END".to_string()]),
            n: Some(3),
            presence_penalty: Some(0.5),
            frequency_penalty: Some(0.3),
            user: Some("user_123".to_string()),
        };

        let normalized = to_normalized(req).unwrap();
        assert_eq!(normalized.temperature, Some(0.8));
        assert_eq!(normalized.top_p, Some(0.9));
        assert_eq!(normalized.max_tokens, Some(500));
        assert_eq!(normalized.stream, true);
        assert_eq!(normalized.stop_sequences, vec!["END".to_string()]);
        assert_eq!(normalized.messages[0].name, Some("Alice".to_string()));
    }

    #[test]
    fn test_empty_messages_array() {
        let req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![],
            temperature: None,
            top_p: None,
            max_tokens: None,
            stream: None,
            stop: None,
            n: None,
            presence_penalty: None,
            frequency_penalty: None,
            user: None,
        };

        // Validation should reject empty messages array
        let result = to_normalized(req);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("messages array cannot be empty"));
    }

    #[test]
    fn test_openai_router_creation() {
        let router = router();
        // Just verify it creates without panicking
        // The router is properly configured with /v1/chat/completions endpoint
        drop(router);
    }
}
