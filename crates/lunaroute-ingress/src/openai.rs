//! OpenAI ingress adapter

use crate::types::{IngressError, IngressResult};
use axum::{
    extract::Json,
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use lunaroute_core::normalized::{
    ContentPart, FinishReason, FunctionCall, FunctionDefinition, Message, MessageContent,
    NormalizedRequest, NormalizedResponse, Role, Tool, ToolCall, ToolChoice,
};
#[cfg(test)]
use lunaroute_core::normalized::{Choice, Usage};
use serde::{Deserialize, Serialize};

/// Maximum size for tool arguments (1MB)
const MAX_TOOL_ARGS_SIZE: usize = 1_000_000;

/// Validate tool parameter schema (must be valid JSON Schema)
fn validate_tool_schema(schema: &serde_json::Value, tool_name: &str) -> IngressResult<()> {
    // Ensure it's a valid JSON Schema object
    if !schema.is_object() {
        return Err(IngressError::InvalidRequest(
            format!("Tool '{}': parameters must be a valid JSON Schema object", tool_name)
        ));
    }

    // Check for required "type" field (common JSON Schema requirement)
    if schema.get("type").is_none() {
        return Err(IngressError::InvalidRequest(
            format!("Tool '{}': schema must have 'type' field", tool_name)
        ));
    }

    Ok(())
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<OpenAITool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<OpenAIToolChoice>,
}

/// OpenAI message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
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

/// OpenAI tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAITool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: OpenAIFunction,
}

/// OpenAI function definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIFunction {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: serde_json::Value,
}

/// OpenAI tool choice
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OpenAIToolChoice {
    String(String), // "none", "auto", "required"
    Object {
        #[serde(rename = "type")]
        tool_type: String,
        function: OpenAIFunctionChoice
    },
}

/// OpenAI function choice
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIFunctionChoice {
    pub name: String,
}

/// OpenAI tool call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: OpenAIFunctionCall,
}

/// OpenAI function call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIFunctionCall {
    pub name: String,
    pub arguments: String,
}

/// Validate OpenAI request parameters
fn validate_request(req: &OpenAIChatRequest) -> IngressResult<()> {
    // Validate temperature (0.0 to 2.0 for OpenAI)
    if let Some(temp) = req.temperature
        && !(0.0..=2.0).contains(&temp) {
            return Err(IngressError::InvalidRequest(
                format!("temperature must be between 0.0 and 2.0, got {}", temp)
            ));
        }

    // Validate top_p (0.0 to 1.0)
    if let Some(top_p) = req.top_p
        && !(0.0..=1.0).contains(&top_p) {
            return Err(IngressError::InvalidRequest(
                format!("top_p must be between 0.0 and 1.0, got {}", top_p)
            ));
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
    if let Some(penalty) = req.presence_penalty
        && !(-2.0..=2.0).contains(&penalty) {
            return Err(IngressError::InvalidRequest(
                format!("presence_penalty must be between -2.0 and 2.0, got {}", penalty)
            ));
        }

    // Validate frequency_penalty (-2.0 to 2.0)
    if let Some(penalty) = req.frequency_penalty
        && !(-2.0..=2.0).contains(&penalty) {
            return Err(IngressError::InvalidRequest(
                format!("frequency_penalty must be between -2.0 and 2.0, got {}", penalty)
            ));
        }

    // Validate n (number of completions)
    if let Some(n) = req.n
        && !(1..=10).contains(&n) {
            return Err(IngressError::InvalidRequest(
                format!("n must be between 1 and 10, got {}", n)
            ));
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

            // Validate message content length if present
            if let Some(ref content) = msg.content
                && content.len() > 1_000_000 {
                    return Err(IngressError::InvalidRequest(
                        format!("Message content too large: {} bytes (max 1MB)", content.len())
                    ));
                }

            // Convert tool_calls
            let tool_calls = if let Some(calls) = msg.tool_calls {
                calls.into_iter().map(|call| {
                    // Validate tool arguments size
                    if call.function.arguments.len() > MAX_TOOL_ARGS_SIZE {
                        return Err(IngressError::InvalidRequest(
                            format!("Tool arguments too large for '{}': {} bytes (max 1MB)",
                                    call.function.name, call.function.arguments.len())
                        ));
                    }

                    Ok(ToolCall {
                        id: call.id,
                        tool_type: call.tool_type,
                        function: FunctionCall {
                            name: call.function.name,
                            arguments: call.function.arguments,
                        },
                    })
                }).collect::<Result<Vec<_>, _>>()?
            } else {
                vec![]
            };

            Ok(Message {
                role,
                content: MessageContent::Text(msg.content.unwrap_or_default()),
                name: msg.name,
                tool_calls,
                tool_call_id: msg.tool_call_id,
            })
        })
        .collect();

    // Convert tools with validation
    let tools = if let Some(tools) = req.tools {
        let result: Result<Vec<Tool>, IngressError> = tools.into_iter().map(|tool| {
            // Validate tool parameter schema
            validate_tool_schema(&tool.function.parameters, &tool.function.name)?;

            Ok(Tool {
                tool_type: tool.tool_type,
                function: FunctionDefinition {
                    name: tool.function.name,
                    description: tool.function.description,
                    parameters: tool.function.parameters,
                },
            })
        }).collect();
        result?
    } else {
        vec![]
    };

    // Convert tool_choice
    let tool_choice = req.tool_choice.and_then(|choice| {
        match choice {
            OpenAIToolChoice::String(s) => {
                match s.as_str() {
                    "none" => Some(ToolChoice::None),
                    "auto" => Some(ToolChoice::Auto),
                    "required" => Some(ToolChoice::Required),
                    _ => None,
                }
            },
            OpenAIToolChoice::Object { function, .. } => {
                Some(ToolChoice::Specific { name: function.name })
            }
        }
    });

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
        tools,
        tool_choice,
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
                MessageContent::Text(text) => {
                    if text.is_empty() && !choice.message.tool_calls.is_empty() {
                        None
                    } else {
                        Some(text)
                    }
                },
                MessageContent::Parts(parts) => {
                    // Extract text parts from multimodal content
                    let text_parts: Vec<String> = parts.iter()
                        .filter_map(|part| match part {
                            ContentPart::Text { text } => Some(text.clone()),
                            ContentPart::Image { .. } => {
                                tracing::warn!("Image content in response not supported for OpenAI format, skipping");
                                None
                            }
                        })
                        .collect();

                    if text_parts.is_empty() {
                        if choice.message.tool_calls.is_empty() {
                            tracing::warn!("No text content in multimodal response");
                        }
                        None
                    } else {
                        Some(text_parts.join("\n"))
                    }
                }
            };

            let tool_calls = if !choice.message.tool_calls.is_empty() {
                Some(choice.message.tool_calls.into_iter().map(|call| {
                    OpenAIToolCall {
                        id: call.id,
                        tool_type: call.tool_type,
                        function: OpenAIFunctionCall {
                            name: call.function.name,
                            arguments: call.function.arguments,
                        },
                    }
                }).collect())
            } else {
                None
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
                    name: choice.message.name,
                    tool_calls,
                    tool_call_id: choice.message.tool_call_id,
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
                content: Some("This is a placeholder response".to_string()),
                name: None,
                tool_calls: None,
                tool_call_id: None,
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
                    content: Some("You are a helpful assistant".to_string()),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                OpenAIMessage {
                    role: "user".to_string(),
                    content: Some("Hello!".to_string()),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
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
            tools: None,
            tool_choice: None,
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
        assert_eq!(openai.choices[0].message.content, Some("Hello, world!".to_string()));
        assert_eq!(openai.usage.total_tokens, 15);
        assert_eq!(openai.choices[0].finish_reason, Some("stop".to_string()));
    }

    #[test]
    fn test_invalid_role() {
        let req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![OpenAIMessage {
                role: "invalid".to_string(),
                content: Some("test".to_string()),
                name: None,
                tool_calls: None,
                tool_call_id: None,
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
            tools: None,
            tool_choice: None,
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
                    content: Some("What's the weather?".to_string()),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                OpenAIMessage {
                    role: "assistant".to_string(),
                    content: Some("".to_string()),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                OpenAIMessage {
                    role: "tool".to_string(),
                    content: Some("Sunny, 72°F".to_string()),
                    name: Some("get_weather".to_string()),
                    tool_calls: None,
                    tool_call_id: None,
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
            tools: None,
            tool_choice: None,
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
                content: Some("Hello!".to_string()),
                name: None,
                tool_calls: None,
                tool_call_id: None,
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
            tools: None,
            tool_choice: None,
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
        assert_eq!(openai.choices[0].message.content, Some("First choice".to_string()));
        assert_eq!(openai.choices[1].message.content, Some("Second choice".to_string()));
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
        // Extracts text from multimodal Parts
        assert_eq!(openai.choices[0].message.content, Some("I see an image".to_string()));
    }

    #[test]
    fn test_message_with_all_optional_fields() {
        let req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![OpenAIMessage {
                role: "user".to_string(),
                content: Some("Hello".to_string()),
                name: Some("Alice".to_string()),
                tool_calls: None,
                tool_call_id: None,
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
            tools: None,
            tool_choice: None,
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
            tools: None,
            tool_choice: None,
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
