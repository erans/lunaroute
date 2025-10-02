//! OpenAI ingress adapter

use crate::types::{IngressError, IngressResult};
use axum::{
    extract::{Json, State},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::post,
    Router,
};
use futures::StreamExt;
use lunaroute_core::{
    normalized::{
        ContentPart, FinishReason, FunctionCall, FunctionDefinition, Message,
        MessageContent, NormalizedRequest, NormalizedResponse, NormalizedStreamEvent, Role, Tool,
        ToolCall, ToolChoice,
    },
    provider::Provider,
};
#[cfg(test)]
use lunaroute_core::normalized::{Choice, Usage};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

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

/// OpenAI streaming chunk (SSE format)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIStreamChunk {
    id: String,
    object: String,
    created: i64,
    model: String,
    choices: Vec<OpenAIStreamChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<OpenAIUsage>,
}

/// OpenAI streaming choice
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIStreamChoice {
    index: u32,
    delta: OpenAIDelta,
    #[serde(skip_serializing_if = "Option::is_none")]
    finish_reason: Option<String>,
}

/// OpenAI delta (streaming content)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCallDelta>>,
}

/// OpenAI tool call delta
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIToolCallDelta {
    index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "type")]
    tool_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function: Option<OpenAIFunctionCallDelta>,
}

/// OpenAI function call delta
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIFunctionCallDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    arguments: Option<String>,
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
        // Use ID as-is (egress already has proper format from provider)
        id: resp.id,
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

/// Convert normalized stream event to OpenAI SSE chunk
fn stream_event_to_openai_chunk(
    event: NormalizedStreamEvent,
    stream_id: &str,
    model: &str,
) -> Option<OpenAIStreamChunk> {
    match event {
        NormalizedStreamEvent::Start { .. } => {
            // OpenAI doesn't send a separate start event, it's implicit in the first delta
            None
        }
        NormalizedStreamEvent::Delta { index, delta } => Some(OpenAIStreamChunk {
            id: stream_id.to_string(),
            object: "chat.completion.chunk".to_string(),
            created: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_else(|_| std::time::Duration::from_secs(0))
                .as_secs() as i64,
            model: model.to_string(),
            choices: vec![OpenAIStreamChoice {
                index,
                delta: OpenAIDelta {
                    role: delta.role.map(|r| match r {
                        Role::System => "system",
                        Role::User => "user",
                        Role::Assistant => "assistant",
                        Role::Tool => "tool",
                    }.to_string()),
                    content: delta.content,
                    tool_calls: None,
                },
                finish_reason: None,
            }],
            usage: None,
        }),
        NormalizedStreamEvent::ToolCallDelta {
            index,
            tool_call_index,
            id,
            function,
        } => Some(OpenAIStreamChunk {
            id: stream_id.to_string(),
            object: "chat.completion.chunk".to_string(),
            created: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_else(|_| std::time::Duration::from_secs(0))
                .as_secs() as i64,
            model: model.to_string(),
            choices: vec![OpenAIStreamChoice {
                index,
                delta: OpenAIDelta {
                    role: None,
                    content: None,
                    tool_calls: Some(vec![OpenAIToolCallDelta {
                        index: tool_call_index,
                        id,
                        tool_type: Some("function".to_string()),
                        function: function.map(|f| OpenAIFunctionCallDelta {
                            name: f.name,
                            arguments: f.arguments,
                        }),
                    }]),
                },
                finish_reason: None,
            }],
            usage: None,
        }),
        NormalizedStreamEvent::Usage { usage } => Some(OpenAIStreamChunk {
            id: stream_id.to_string(),
            object: "chat.completion.chunk".to_string(),
            created: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_else(|_| std::time::Duration::from_secs(0))
                .as_secs() as i64,
            model: model.to_string(),
            choices: vec![],
            usage: Some(OpenAIUsage {
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
                total_tokens: usage.total_tokens,
            }),
        }),
        NormalizedStreamEvent::End { finish_reason } => {
            let finish_reason_str = match finish_reason {
                FinishReason::Stop => "stop",
                FinishReason::Length => "length",
                FinishReason::ToolCalls => "tool_calls",
                FinishReason::ContentFilter => "content_filter",
                FinishReason::Error => "error",
            };
            Some(OpenAIStreamChunk {
                id: stream_id.to_string(),
                object: "chat.completion.chunk".to_string(),
                created: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_else(|_| std::time::Duration::from_secs(0))
                    .as_secs() as i64,
                model: model.to_string(),
                choices: vec![OpenAIStreamChoice {
                    index: 0,
                    delta: OpenAIDelta {
                        role: None,
                        content: None,
                        tool_calls: None,
                    },
                    finish_reason: Some(finish_reason_str.to_string()),
                }],
                usage: None,
            })
        }
        NormalizedStreamEvent::Error { .. } => {
            // Errors are handled separately, don't convert to chunk
            None
        }
    }
}

/// Chat completion handler
pub async fn chat_completions(
    State(provider): State<Arc<dyn Provider>>,
    Json(req): Json<OpenAIChatRequest>,
) -> Result<Response, IngressError> {
    let start_time = std::time::Instant::now();
    let is_streaming = req.stream.unwrap_or(false);
    let model = req.model.clone();

    // Convert to normalized format (includes validation)
    let normalized = to_normalized(req)?;

    if is_streaming {
        // Log streaming request for observability
        tracing::debug!(
            "OpenAI streaming request: model={}, messages={}",
            model,
            normalized.messages.len()
        );

        // Validate that provider supports streaming
        if !provider.capabilities().supports_streaming {
            return Err(IngressError::UnsupportedFeature(
                "Provider does not support streaming".to_string(),
            ));
        }

        // Handle streaming response
        let stream = provider
            .stream(normalized)
            .await
            .map_err(|e| IngressError::ProviderError(e.to_string()))?;

        // Generate a stream ID and wrap in Arc for efficient sharing across stream events
        let stream_id = Arc::new(format!("chatcmpl-{}", Uuid::new_v4().simple()));
        let model = Arc::new(model);

        // Convert normalized stream to OpenAI SSE format
        let sse_stream = stream.filter_map(move |result| {
            let stream_id = Arc::clone(&stream_id);
            let model = Arc::clone(&model);
            async move {
                match result {
                    Ok(event) => {
                        // Convert normalized event to OpenAI chunk
                        if let Some(chunk) = stream_event_to_openai_chunk(event, stream_id.as_str(), model.as_str()) {
                            // Serialize to JSON
                            match serde_json::to_string(&chunk) {
                                Ok(json) => Some(Ok(Event::default().data(json))),
                                Err(e) => Some(Err(IngressError::Internal(format!(
                                    "Failed to serialize SSE chunk: {}",
                                    e
                                )))),
                            }
                        } else {
                            None
                        }
                    }
                    Err(e) => {
                        // Send error event with proper JSON serialization to prevent injection
                        let error_json = serde_json::json!({
                            "error": {
                                "message": e.to_string()
                            }
                        });
                        match serde_json::to_string(&error_json) {
                            Ok(error_msg) => Some(Ok(Event::default().data(error_msg))),
                            Err(_) => Some(Ok(Event::default().data(r#"{"error":{"message":"Failed to serialize error"}}"#))),
                        }
                    }
                }
            }
        });

        // Add [DONE] message at the end
        let sse_stream = sse_stream.chain(futures::stream::once(async {
            Ok::<_, IngressError>(Event::default().data("[DONE]"))
        }));

        Ok(Sse::new(sse_stream)
            .keep_alive(KeepAlive::default())
            .into_response())
    } else {
        let before_provider = std::time::Instant::now();
        let pre_provider_overhead = before_provider.duration_since(start_time);
        tracing::debug!(
            "Proxy overhead before provider call (normalization): {:.2}ms",
            pre_provider_overhead.as_secs_f64() * 1000.0
        );

        // Handle non-streaming response
        let normalized_response = provider
            .send(normalized)
            .await
            .map_err(|e| IngressError::ProviderError(e.to_string()))?;

        let after_provider = std::time::Instant::now();
        let provider_time = after_provider.duration_since(before_provider);
        tracing::debug!(
            "Provider response time: {:.2}ms",
            provider_time.as_secs_f64() * 1000.0
        );

        // Convert back to OpenAI format
        let response = from_normalized(normalized_response);

        let response_result = Ok(Json(response).into_response());

        let total_time = std::time::Instant::now().duration_since(start_time);
        let post_provider_overhead = total_time - provider_time - pre_provider_overhead;
        tracing::debug!(
            "Proxy overhead after provider response (denormalization): {:.2}ms",
            post_provider_overhead.as_secs_f64() * 1000.0
        );
        tracing::debug!(
            "Total proxy overhead: {:.2}ms (pre: {:.2}ms + post: {:.2}ms), provider: {:.2}ms, total: {:.2}ms",
            (pre_provider_overhead + post_provider_overhead).as_secs_f64() * 1000.0,
            pre_provider_overhead.as_secs_f64() * 1000.0,
            post_provider_overhead.as_secs_f64() * 1000.0,
            provider_time.as_secs_f64() * 1000.0,
            total_time.as_secs_f64() * 1000.0
        );

        response_result
    }
}

/// Create OpenAI router with provider state
pub fn router(provider: Arc<dyn Provider>) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .with_state(provider)
}

/// State for OpenAI passthrough handler (connector + optional stats tracker + metrics + session recorder)
pub struct OpenAIPassthroughState {
    pub connector: Arc<lunaroute_egress::openai::OpenAIConnector>,
    pub stats_tracker: Option<Arc<dyn crate::types::SessionStatsTracker>>,
    pub metrics: Option<Arc<lunaroute_observability::Metrics>>,
    pub session_recorder: Option<Arc<lunaroute_session::MultiWriterRecorder>>,
}

/// Create OpenAI passthrough router (for OpenAI→OpenAI direct routing)
pub fn passthrough_router(
    connector: Arc<lunaroute_egress::openai::OpenAIConnector>,
    stats_tracker: Option<Arc<dyn crate::types::SessionStatsTracker>>,
    metrics: Option<Arc<lunaroute_observability::Metrics>>,
    session_recorder: Option<Arc<lunaroute_session::MultiWriterRecorder>>,
) -> Router {
    let state = Arc::new(OpenAIPassthroughState {
        connector,
        stats_tracker,
        metrics,
        session_recorder,
    });

    Router::new()
        .route("/v1/chat/completions", post(chat_completions_passthrough))
        .with_state(state)
}

/// Passthrough handler for OpenAI→OpenAI routing (no normalization)
/// Takes raw JSON, sends directly to OpenAI, returns raw JSON
/// Preserves 100% API fidelity while still extracting metrics
pub async fn chat_completions_passthrough(
    State(state): State<Arc<OpenAIPassthroughState>>,
    Json(req): Json<serde_json::Value>,
) -> Result<Response, IngressError> {
    let start_time = std::time::Instant::now();
    tracing::debug!("OpenAI passthrough mode: skipping normalization");

    // Extract session ID from metadata (reserved for future use)
    let _session_id = req
        .get("metadata")
        .and_then(|m| m.get("user_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let is_streaming = req.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);

    let before_provider = std::time::Instant::now();
    let pre_provider_overhead = before_provider.duration_since(start_time);
    tracing::debug!(
        "Proxy overhead before provider call: {:.2}ms",
        pre_provider_overhead.as_secs_f64() * 1000.0
    );

    // Extract model before req is moved (for metrics and session recording)
    let model = req.get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    // Generate session ID and request ID for recording
    let recording_session_id = if state.session_recorder.is_some() {
        Some(uuid::Uuid::new_v4().to_string())
    } else {
        None
    };

    let recording_request_id = if state.session_recorder.is_some() {
        Some(uuid::Uuid::new_v4().to_string())
    } else {
        None
    };

    // Start session recording if enabled (using async events)
    if let (Some(recorder), Some(session_id), Some(request_id)) =
        (&state.session_recorder, &recording_session_id, &recording_request_id)
    {
        use lunaroute_session::{SessionEvent, events::SessionMetadata as V2Metadata};

        recorder.record_event(SessionEvent::Started {
            session_id: session_id.clone(),
            request_id: request_id.clone(),
            timestamp: chrono::Utc::now(),
            model_requested: model.clone(),
            provider: "openai".to_string(),
            listener: "openai".to_string(),
            is_streaming,
            metadata: V2Metadata {
                client_ip: None,
                user_agent: None,
                api_version: None,
                request_headers: Default::default(),
                session_tags: vec![],
            },
        });
    }

    if is_streaming {
        // Handle streaming passthrough
        let stream_response = state.connector
            .stream_passthrough(req)
            .await
            .map_err(|e| IngressError::ProviderError(e.to_string()))?;

        // Track streaming metrics using shared module
        use futures::StreamExt;
        use crate::streaming_metrics::StreamingMetricsTracker;

        let tracker = StreamingMetricsTracker::new(before_provider);
        let tracker_clone = std::sync::Arc::new(tracker);
        let tracker_ref = tracker_clone.clone();
        let metrics_clone = state.metrics.clone();
        let model_name_clone = model.clone();

        let byte_stream = stream_response.bytes_stream();
        let sse_stream = eventsource_stream::EventStream::new(byte_stream);

        let tracked_stream = sse_stream.map(move |event_result| {
            match event_result {
                Ok(event) => {
                    let now = std::time::Instant::now();

                    // Track TTFT on first chunk
                    tracker_ref.record_ttft(now);

                    // Track chunk latency
                    let _ = tracker_ref.record_chunk_latency(now, "openai", &model_name_clone, &metrics_clone);

                    // Increment chunk count
                    tracker_ref.increment_chunk_count();

                    // Parse event data once to avoid double parsing
                    let parsed_data = match serde_json::from_str::<serde_json::Value>(&event.data) {
                        Ok(json) => json,
                        Err(e) => {
                            // Log but don't fail the stream for individual parse errors
                            tracing::warn!("Failed to parse SSE event data: {}", e);
                            // Forward raw data if JSON parsing fails
                            return Ok(Event::default().data(event.data));
                        }
                    };

                    // Extract text deltas from choices
                    if let Some(choices) = parsed_data.get("choices").and_then(|c| c.as_array())
                        && let Some(first_choice) = choices.first()
                        && let Some(delta) = first_choice.get("delta")
                        && let Some(content) = delta.get("content").and_then(|c| c.as_str())
                    {
                        tracker_ref.accumulate_text(content, "openai", &model_name_clone, &metrics_clone);
                    }

                    // Extract model from chunk
                    if let Some(model_str) = parsed_data.get("model").and_then(|m| m.as_str()) {
                        tracker_ref.set_model(model_str.to_string());
                    }

                    // Extract finish reason from choices
                    if let Some(choices) = parsed_data.get("choices").and_then(|c| c.as_array())
                        && let Some(first_choice) = choices.first()
                        && let Some(reason) = first_choice.get("finish_reason").and_then(|r| r.as_str())
                    {
                        tracker_ref.set_finish_reason(reason.to_string());
                    }

                    // Forward the parsed event
                    Event::default().json_data(parsed_data)
                        .map_err(|e| IngressError::Internal(format!("Failed to create SSE event: {}", e)))
                }
                Err(e) => {
                    Err(IngressError::Internal(format!("SSE stream error: {}", e)))
                }
            }
        });

        // Add completion handler to record session stats when stream ends
        let recorder_clone = state.session_recorder.clone();
        let session_id_clone = recording_session_id.clone();
        let request_id_clone = recording_request_id.clone();
        let start_clone = start_time;
        let before_provider_clone = before_provider;
        let metrics_for_finalize = state.metrics.clone();
        let model_for_finalize = model.clone();

        let completion_stream = tracked_stream.chain(futures::stream::once(async move {
            // Finalize metrics using shared module
            let finalized = tracker_clone.finalize(start_clone, before_provider_clone);

            // Record to Prometheus
            finalized.record_to_prometheus(&metrics_for_finalize, "openai", &model_for_finalize);

            // Record StreamStarted and Completed events after stream ends
            if let (Some(recorder), Some(session_id), Some(request_id)) =
                (recorder_clone.as_ref(), session_id_clone.as_ref(), request_id_clone.as_ref())
            {
                use lunaroute_session::{SessionEvent, events::{FinalSessionStats, TokenTotals, ToolUsageSummary, PerformanceMetrics}};

                // Record StreamStarted event
                if finalized.ttft_ms > 0 {
                    recorder.record_event(SessionEvent::StreamStarted {
                        session_id: session_id.clone(),
                        request_id: request_id.clone(),
                        timestamp: chrono::Utc::now(),
                        time_to_first_token_ms: finalized.ttft_ms,
                    });
                }

                // Record Completed event with streaming_stats
                recorder.record_event(SessionEvent::Completed {
                    session_id: session_id.clone(),
                    request_id: request_id.clone(),
                    timestamp: chrono::Utc::now(),
                    success: true,
                    error: None,
                    finish_reason: finalized.finish_reason.clone(),
                    final_stats: Box::new(FinalSessionStats {
                        total_duration_ms: finalized.total_duration_ms,
                        provider_time_ms: finalized.total_duration_ms, // Entire duration is provider time in streaming
                        proxy_overhead_ms: 0.0, // Minimal overhead in passthrough mode
                        total_tokens: TokenTotals::default(), // Tokens not available in passthrough streaming
                        tool_summary: ToolUsageSummary::default(),
                        performance: PerformanceMetrics::default(),
                        streaming_stats: Some(finalized.to_streaming_stats()),
                        estimated_cost: None,
                    }),
                });
            }

            // Return empty event to complete the stream
            Err(IngressError::Internal("Stream completed".to_string()))
        }));

        // Filter out the completion marker
        let final_stream = completion_stream.filter_map(|result| {
            futures::future::ready(match result {
                Ok(event) => Some(Ok(event)),
                Err(e) if e.to_string().contains("Stream completed") => None,
                Err(e) => Some(Err(e)),
            })
        });

        // Create SSE response
        let sse_response = Sse::new(final_stream).keep_alive(KeepAlive::default());

        return Ok(sse_response.into_response());
    }

    // Send directly to OpenAI API (non-streaming)
    let response_result = state.connector
        .send_passthrough(req)
        .await;

    let response = match response_result {
        Ok(resp) => resp,
        Err(e) => {
            // Record error in session if recording is enabled
            if let (Some(recorder), Some(session_id), Some(request_id)) =
                (&state.session_recorder, &recording_session_id, &recording_request_id)
            {
                use lunaroute_session::{SessionEvent, events::FinalSessionStats};

                recorder.record_event(SessionEvent::Completed {
                    session_id: session_id.clone(),
                    request_id: request_id.clone(),
                    timestamp: chrono::Utc::now(),
                    success: false,
                    error: Some(e.to_string()),
                    finish_reason: None,
                    final_stats: Box::new(FinalSessionStats {
                        total_duration_ms: start_time.elapsed().as_millis() as u64,
                        provider_time_ms: 0,
                        proxy_overhead_ms: 0.0,
                        total_tokens: Default::default(),
                        tool_summary: Default::default(),
                        performance: Default::default(),
                        streaming_stats: None,
                        estimated_cost: None,
                    }),
                });
            }
            return Err(IngressError::ProviderError(e.to_string()));
        }
    };

    let after_provider = std::time::Instant::now();
    let provider_time = after_provider.duration_since(before_provider);
    tracing::debug!(
        "Provider response time: {:.2}ms",
        provider_time.as_secs_f64() * 1000.0
    );

    // Extract metrics for observability (optional: log tokens, model, etc.)
    let (input_tokens, output_tokens) = if let Some(usage) = response.get("usage") {
        let input = usage.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let output = usage.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0);

        if input > 0 || output > 0 {
            tracing::debug!(
                "Passthrough metrics: prompt_tokens={}, completion_tokens={}, total={}",
                input,
                output,
                input + output
            );
        }
        (input, output)
    } else {
        (0, 0)
    };

    // Record session response if enabled (using async events)
    if let (Some(recorder), Some(session_id), Some(request_id)) =
        (&state.session_recorder, &recording_session_id, &recording_request_id)
        && let Ok(openai_resp) = serde_json::from_value::<OpenAIChatResponse>(response.clone()) {
            use lunaroute_session::{SessionEvent, events::{ResponseStats, TokenStats, FinalSessionStats, TokenTotals, ToolUsageSummary, PerformanceMetrics}};

            // Extract response text
            let response_text = openai_resp.choices.iter()
                .filter_map(|c| c.message.content.clone())
                .collect::<Vec<_>>()
                .join("\n");

            // Record response event
            recorder.record_event(SessionEvent::ResponseRecorded {
                session_id: session_id.clone(),
                request_id: request_id.clone(),
                timestamp: chrono::Utc::now(),
                response_text: response_text.clone(),
                response_json: response.clone(),
                model_used: openai_resp.model.clone(),
                stats: ResponseStats {
                    provider_latency_ms: provider_time.as_millis() as u64,
                    post_processing_ms: 0.0,
                    total_proxy_overhead_ms: 0.0,
                    tokens: TokenStats {
                        input_tokens: input_tokens as u32,
                        output_tokens: output_tokens as u32,
                        thinking_tokens: None,
                        cache_read_tokens: None,
                        cache_write_tokens: None,
                        total_tokens: (input_tokens + output_tokens) as u32,
                        thinking_percentage: None,
                        tokens_per_second: None,
                    },
                    tool_calls: vec![],
                    response_size_bytes: response_text.len(),
                    content_blocks: openai_resp.choices.len(),
                    has_refusal: false,
                    is_streaming: false,
                    chunk_count: None,
                    streaming_duration_ms: None,
                },
            });

            // Record completion event
            let finish_reason = openai_resp.choices.first()
                .and_then(|c| c.finish_reason.clone());

            recorder.record_event(SessionEvent::Completed {
                session_id: session_id.clone(),
                request_id: request_id.clone(),
                timestamp: chrono::Utc::now(),
                success: true,
                error: None,
                finish_reason,
                final_stats: Box::new(FinalSessionStats {
                    total_duration_ms: start_time.elapsed().as_millis() as u64,
                    provider_time_ms: provider_time.as_millis() as u64,
                    proxy_overhead_ms: 0.0,
                    total_tokens: TokenTotals {
                        total_input: input_tokens,
                        total_output: output_tokens,
                        total_thinking: 0,
                        total_cached: 0,
                        grand_total: input_tokens + output_tokens,
                        by_model: Default::default(),
                    },
                    tool_summary: ToolUsageSummary::default(),
                    performance: PerformanceMetrics {
                        avg_provider_latency_ms: provider_time.as_secs_f64() * 1000.0,
                        p50_latency_ms: Some(provider_time.as_millis() as u64),
                        p95_latency_ms: Some(provider_time.as_millis() as u64),
                        p99_latency_ms: Some(provider_time.as_millis() as u64),
                        max_latency_ms: provider_time.as_millis() as u64,
                        min_latency_ms: provider_time.as_millis() as u64,
                        avg_pre_processing_ms: 0.0,
                        avg_post_processing_ms: 0.0,
                        proxy_overhead_percentage: 0.0,
                    },
                    streaming_stats: None,
                    estimated_cost: None,
                }),
            });
        }

    let response_result = Ok(Json(response).into_response());

    let total_time = std::time::Instant::now().duration_since(start_time);
    let post_provider_overhead = total_time - provider_time - pre_provider_overhead;
    tracing::debug!(
        "Proxy overhead after provider response: {:.2}ms",
        post_provider_overhead.as_secs_f64() * 1000.0
    );
    tracing::debug!(
        "Total proxy overhead: {:.2}ms (pre: {:.2}ms + post: {:.2}ms), provider: {:.2}ms, total: {:.2}ms",
        (pre_provider_overhead + post_provider_overhead).as_secs_f64() * 1000.0,
        pre_provider_overhead.as_secs_f64() * 1000.0,
        post_provider_overhead.as_secs_f64() * 1000.0,
        provider_time.as_secs_f64() * 1000.0,
        total_time.as_secs_f64() * 1000.0
    );

    response_result
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use lunaroute_core::provider::ProviderCapabilities;
    use futures::stream;

    // Mock provider for testing
    struct MockProvider;

    #[async_trait]
    impl Provider for MockProvider {
        async fn send(&self, _request: NormalizedRequest) -> lunaroute_core::Result<NormalizedResponse> {
            Ok(NormalizedResponse {
                id: "test-123".to_string(),
                model: "gpt-4".to_string(),
                choices: vec![Choice {
                    index: 0,
                    message: Message {
                        role: Role::Assistant,
                        content: MessageContent::Text("Hello from mock".to_string()),
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
            })
        }

        async fn stream(
            &self,
            _request: NormalizedRequest,
        ) -> lunaroute_core::Result<Box<dyn futures::Stream<Item = lunaroute_core::Result<lunaroute_core::normalized::NormalizedStreamEvent>> + Send + Unpin>> {
            Ok(Box::new(stream::empty()))
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                supports_streaming: false,
                supports_tools: false,
                supports_vision: false,
            }
        }
    }

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
        let provider = Arc::new(MockProvider);
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

        let response = chat_completions(State(provider), Json(req)).await;
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
        assert!(normalized.stream);
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
        let provider = Arc::new(MockProvider);
        let router = router(provider);
        // Just verify it creates without panicking
        // The router is properly configured with /v1/chat/completions endpoint
        drop(router);
    }

    // Tool schema validation tests
    #[test]
    fn test_tool_schema_invalid_not_object() {
        let req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![OpenAIMessage {
                role: "user".to_string(),
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
            tools: Some(vec![OpenAITool {
                tool_type: "function".to_string(),
                function: OpenAIFunction {
                    name: "test_func".to_string(),
                    description: None,
                    parameters: serde_json::json!("not an object"),
                },
            }]),
            tool_choice: None,
        };

        let result = to_normalized(req);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("parameters must be a valid JSON Schema object"));
    }

    #[test]
    fn test_tool_schema_missing_type_field() {
        let req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![OpenAIMessage {
                role: "user".to_string(),
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
            tools: Some(vec![OpenAITool {
                tool_type: "function".to_string(),
                function: OpenAIFunction {
                    name: "test_func".to_string(),
                    description: None,
                    parameters: serde_json::json!({"properties": {}}),
                },
            }]),
            tool_choice: None,
        };

        let result = to_normalized(req);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("schema must have 'type' field"));
    }

    #[test]
    fn test_tool_schema_valid_object() {
        let req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![OpenAIMessage {
                role: "user".to_string(),
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
            tools: Some(vec![OpenAITool {
                tool_type: "function".to_string(),
                function: OpenAIFunction {
                    name: "test_func".to_string(),
                    description: Some("Test function".to_string()),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "arg1": {"type": "string"}
                        }
                    }),
                },
            }]),
            tool_choice: None,
        };

        let normalized = to_normalized(req).unwrap();
        assert_eq!(normalized.tools.len(), 1);
        assert_eq!(normalized.tools[0].function.name, "test_func");
        assert_eq!(normalized.tools[0].function.description, Some("Test function".to_string()));
    }

    #[test]
    fn test_tool_schema_empty_object() {
        let req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![OpenAIMessage {
                role: "user".to_string(),
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
            tools: Some(vec![OpenAITool {
                tool_type: "function".to_string(),
                function: OpenAIFunction {
                    name: "test_func".to_string(),
                    description: None,
                    parameters: serde_json::json!({"type": "object"}),
                },
            }]),
            tool_choice: None,
        };

        let normalized = to_normalized(req).unwrap();
        assert_eq!(normalized.tools.len(), 1);
    }

    // Tool argument size validation tests
    #[test]
    fn test_tool_args_at_size_limit() {
        // Create arguments string at exactly 1MB
        let large_args = "x".repeat(1_000_000);

        let req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![
                OpenAIMessage {
                    role: "user".to_string(),
                    content: Some("test".to_string()),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                OpenAIMessage {
                    role: "assistant".to_string(),
                    content: None,
                    name: None,
                    tool_calls: Some(vec![OpenAIToolCall {
                        id: "call_1".to_string(),
                        tool_type: "function".to_string(),
                        function: OpenAIFunctionCall {
                            name: "test_func".to_string(),
                            arguments: large_args,
                        },
                    }]),
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

        // Should succeed at exactly the limit
        let normalized = to_normalized(req).unwrap();
        assert_eq!(normalized.messages.len(), 2);
    }

    #[test]
    fn test_tool_args_exceeds_size_limit() {
        // Create arguments string exceeding 1MB
        let large_args = "x".repeat(1_000_001);

        let req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![
                OpenAIMessage {
                    role: "user".to_string(),
                    content: Some("test".to_string()),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                OpenAIMessage {
                    role: "assistant".to_string(),
                    content: None,
                    name: None,
                    tool_calls: Some(vec![OpenAIToolCall {
                        id: "call_1".to_string(),
                        tool_type: "function".to_string(),
                        function: OpenAIFunctionCall {
                            name: "test_func".to_string(),
                            arguments: large_args,
                        },
                    }]),
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

        let result = to_normalized(req);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Tool arguments too large"));
        assert!(err_msg.contains("max 1MB"));
    }

    #[test]
    fn test_tool_args_empty_json() {
        let req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![
                OpenAIMessage {
                    role: "user".to_string(),
                    content: Some("test".to_string()),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                OpenAIMessage {
                    role: "assistant".to_string(),
                    content: None,
                    name: None,
                    tool_calls: Some(vec![OpenAIToolCall {
                        id: "call_1".to_string(),
                        tool_type: "function".to_string(),
                        function: OpenAIFunctionCall {
                            name: "test_func".to_string(),
                            arguments: "{}".to_string(),
                        },
                    }]),
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
        assert_eq!(normalized.messages.len(), 2);
        assert_eq!(normalized.messages[1].tool_calls.len(), 1);
        assert_eq!(normalized.messages[1].tool_calls[0].function.arguments, "{}");
    }

    // Multimodal content extraction tests
    #[test]
    fn test_multimodal_multiple_text_parts() {
        use lunaroute_core::normalized::ContentPart;

        let resp = NormalizedResponse {
            id: "test".to_string(),
            model: "gpt-4".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: MessageContent::Parts(vec![
                        ContentPart::Text {
                            text: "First part".to_string(),
                        },
                        ContentPart::Text {
                            text: "Second part".to_string(),
                        },
                        ContentPart::Text {
                            text: "Third part".to_string(),
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
        // Multiple text parts should be joined with newlines
        assert_eq!(openai.choices[0].message.content, Some("First part\nSecond part\nThird part".to_string()));
    }

    #[test]
    fn test_multimodal_mixed_text_and_images() {
        use lunaroute_core::normalized::{ContentPart, ImageSource};

        let resp = NormalizedResponse {
            id: "test".to_string(),
            model: "gpt-4-vision".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: MessageContent::Parts(vec![
                        ContentPart::Text {
                            text: "I see".to_string(),
                        },
                        ContentPart::Image {
                            source: ImageSource::Url {
                                url: "https://example.com/image.jpg".to_string(),
                            },
                        },
                        ContentPart::Text {
                            text: "a cat".to_string(),
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
        // Image parts should be filtered out, only text extracted
        assert_eq!(openai.choices[0].message.content, Some("I see\na cat".to_string()));
    }

    #[test]
    fn test_multimodal_empty_parts() {
        let resp = NormalizedResponse {
            id: "test".to_string(),
            model: "gpt-4".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: MessageContent::Parts(vec![]),
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
        // Empty parts should result in None content
        assert_eq!(openai.choices[0].message.content, None);
    }

    #[test]
    fn test_multimodal_only_images() {
        use lunaroute_core::normalized::{ContentPart, ImageSource};

        let resp = NormalizedResponse {
            id: "test".to_string(),
            model: "gpt-4-vision".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: MessageContent::Parts(vec![
                        ContentPart::Image {
                            source: ImageSource::Url {
                                url: "https://example.com/image1.jpg".to_string(),
                            },
                        },
                        ContentPart::Image {
                            source: ImageSource::Base64 {
                                media_type: "image/jpeg".to_string(),
                                data: "base64data".to_string(),
                            },
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
        // Only images should result in None content
        assert_eq!(openai.choices[0].message.content, None);
    }

    // Round-trip conversion tests
    #[test]
    fn test_roundtrip_basic_request() {
        let original_req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![
                OpenAIMessage {
                    role: "system".to_string(),
                    content: Some("You are helpful".to_string()),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                OpenAIMessage {
                    role: "user".to_string(),
                    content: Some("Hello world".to_string()),
                    name: Some("user1".to_string()),
                    tool_calls: None,
                    tool_call_id: None,
                },
            ],
            temperature: Some(0.8),
            top_p: Some(0.9),
            max_tokens: Some(150),
            stream: Some(false),
            stop: Some(vec!["STOP".to_string()]),
            n: None,
            presence_penalty: Some(0.5),
            frequency_penalty: Some(-0.5),
            user: None,
            tools: None,
            tool_choice: None,
        };

        let normalized = to_normalized(original_req.clone()).unwrap();

        // Verify key fields are preserved in normalized format
        assert_eq!(normalized.model, "gpt-4");
        assert_eq!(normalized.messages.len(), 2);
        assert_eq!(normalized.temperature, Some(0.8));
        assert_eq!(normalized.max_tokens, Some(150));
        assert_eq!(normalized.stop_sequences, vec!["STOP".to_string()]);
    }

    #[test]
    fn test_roundtrip_request_with_tools() {
        let original_req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![OpenAIMessage {
                role: "user".to_string(),
                content: Some("What's the weather?".to_string()),
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
            tools: Some(vec![OpenAITool {
                tool_type: "function".to_string(),
                function: OpenAIFunction {
                    name: "get_weather".to_string(),
                    description: Some("Get weather info".to_string()),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "location": {"type": "string"}
                        }
                    }),
                },
            }]),
            tool_choice: Some(OpenAIToolChoice::String("auto".to_string())),
        };

        let normalized = to_normalized(original_req.clone()).unwrap();

        // Verify tools are preserved
        assert_eq!(normalized.tools.len(), 1);
        assert_eq!(normalized.tools[0].function.name, "get_weather");
        assert_eq!(normalized.tools[0].function.description, Some("Get weather info".to_string()));
        assert_eq!(normalized.tool_choice, Some(ToolChoice::Auto));
    }

    #[test]
    fn test_roundtrip_response() {
        let normalized_resp = NormalizedResponse {
            id: "resp-123".to_string(),
            model: "gpt-4".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: MessageContent::Text("Hello back!".to_string()),
                    name: None,
                    tool_calls: vec![],
                    tool_call_id: None,
                },
                finish_reason: Some(FinishReason::Stop),
            }],
            usage: Usage {
                prompt_tokens: 20,
                completion_tokens: 10,
                total_tokens: 30,
            },
            created: 1234567890,
            metadata: std::collections::HashMap::new(),
        };

        let openai_resp = from_normalized(normalized_resp.clone());

        // Verify response fields are preserved (ID is used as-is)
        assert_eq!(openai_resp.id, "resp-123");
        assert_eq!(openai_resp.model, "gpt-4");
        assert_eq!(openai_resp.choices[0].message.content, Some("Hello back!".to_string()));
        assert_eq!(openai_resp.usage.total_tokens, 30);
        assert_eq!(openai_resp.choices[0].finish_reason, Some("stop".to_string()));
    }

    #[test]
    fn test_roundtrip_message_with_tool_calls() {
        let original_req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![
                OpenAIMessage {
                    role: "user".to_string(),
                    content: Some("Get weather".to_string()),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                OpenAIMessage {
                    role: "assistant".to_string(),
                    content: None,
                    name: None,
                    tool_calls: Some(vec![OpenAIToolCall {
                        id: "call_123".to_string(),
                        tool_type: "function".to_string(),
                        function: OpenAIFunctionCall {
                            name: "get_weather".to_string(),
                            arguments: r#"{"location":"NYC"}"#.to_string(),
                        },
                    }]),
                    tool_call_id: None,
                },
                OpenAIMessage {
                    role: "tool".to_string(),
                    content: Some("72°F".to_string()),
                    name: Some("get_weather".to_string()),
                    tool_calls: None,
                    tool_call_id: Some("call_123".to_string()),
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

        let normalized = to_normalized(original_req.clone()).unwrap();

        // Verify tool calls are preserved
        assert_eq!(normalized.messages.len(), 3);
        assert_eq!(normalized.messages[1].tool_calls.len(), 1);
        assert_eq!(normalized.messages[1].tool_calls[0].id, "call_123");
        assert_eq!(normalized.messages[1].tool_calls[0].function.name, "get_weather");
        assert_eq!(normalized.messages[2].role, Role::Tool);
        assert_eq!(normalized.messages[2].tool_call_id, Some("call_123".to_string()));
    }

    // Error path tests
    #[test]
    fn test_error_invalid_temperature_range() {
        let req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![OpenAIMessage {
                role: "user".to_string(),
                content: Some("test".to_string()),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            temperature: Some(2.5), // Invalid: > 2.0
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
        assert!(result.unwrap_err().to_string().contains("temperature must be between 0.0 and 2.0"));
    }

    #[test]
    fn test_error_message_content_too_large() {
        // Create content larger than 1MB
        let large_content = "x".repeat(1_000_001);

        let req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![OpenAIMessage {
                role: "user".to_string(),
                content: Some(large_content),
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
        assert!(result.unwrap_err().to_string().contains("Message content too large"));
    }

    #[test]
    fn test_error_empty_model_name() {
        let req = OpenAIChatRequest {
            model: "".to_string(), // Invalid: empty
            messages: vec![OpenAIMessage {
                role: "user".to_string(),
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
        assert!(result.unwrap_err().to_string().contains("model field cannot be empty"));
    }

    #[test]
    fn test_error_invalid_max_tokens() {
        let req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![OpenAIMessage {
                role: "user".to_string(),
                content: Some("test".to_string()),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            temperature: None,
            top_p: None,
            max_tokens: Some(200000), // Invalid: > 100000
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
        assert!(result.unwrap_err().to_string().contains("max_tokens must be <= 100000"));
    }

    // Edge case tests
    #[test]
    fn test_edge_empty_tools_array() {
        let req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![OpenAIMessage {
                role: "user".to_string(),
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
            tools: Some(vec![]), // Empty tools array
            tool_choice: None,
        };

        let normalized = to_normalized(req).unwrap();
        assert_eq!(normalized.tools.len(), 0);
    }

    #[test]
    fn test_edge_message_only_tool_calls_no_content() {
        let req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![
                OpenAIMessage {
                    role: "user".to_string(),
                    content: Some("test".to_string()),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                OpenAIMessage {
                    role: "assistant".to_string(),
                    content: None, // No content
                    name: None,
                    tool_calls: Some(vec![OpenAIToolCall {
                        id: "call_1".to_string(),
                        tool_type: "function".to_string(),
                        function: OpenAIFunctionCall {
                            name: "test_func".to_string(),
                            arguments: "{}".to_string(),
                        },
                    }]),
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
        assert_eq!(normalized.messages.len(), 2);
        // Content should be empty string when None
        if let MessageContent::Text(text) = &normalized.messages[1].content {
            assert_eq!(text, "");
        } else {
            panic!("Expected Text content");
        }
        assert_eq!(normalized.messages[1].tool_calls.len(), 1);
    }

    #[test]
    fn test_edge_unicode_in_content_and_names() {
        let req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![OpenAIMessage {
                role: "user".to_string(),
                content: Some("Hello 世界 🌍 مرحبا".to_string()), // Unicode content
                name: Some("用户_1".to_string()), // Unicode name
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

        let normalized = to_normalized(req).unwrap();
        if let MessageContent::Text(text) = &normalized.messages[0].content {
            assert!(text.contains("世界"));
            assert!(text.contains("🌍"));
            assert!(text.contains("مرحبا"));
        } else {
            panic!("Expected Text content");
        }
        assert_eq!(normalized.messages[0].name, Some("用户_1".to_string()));
    }

    #[test]
    fn test_edge_multiple_tool_calls_in_message() {
        let req = OpenAIChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![
                OpenAIMessage {
                    role: "user".to_string(),
                    content: Some("test".to_string()),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                OpenAIMessage {
                    role: "assistant".to_string(),
                    content: None,
                    name: None,
                    tool_calls: Some(vec![
                        OpenAIToolCall {
                            id: "call_1".to_string(),
                            tool_type: "function".to_string(),
                            function: OpenAIFunctionCall {
                                name: "func1".to_string(),
                                arguments: r#"{"arg":"value1"}"#.to_string(),
                            },
                        },
                        OpenAIToolCall {
                            id: "call_2".to_string(),
                            tool_type: "function".to_string(),
                            function: OpenAIFunctionCall {
                                name: "func2".to_string(),
                                arguments: r#"{"arg":"value2"}"#.to_string(),
                            },
                        },
                        OpenAIToolCall {
                            id: "call_3".to_string(),
                            tool_type: "function".to_string(),
                            function: OpenAIFunctionCall {
                                name: "func3".to_string(),
                                arguments: r#"{"arg":"value3"}"#.to_string(),
                            },
                        },
                    ]),
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
        assert_eq!(normalized.messages.len(), 2);
        assert_eq!(normalized.messages[1].tool_calls.len(), 3);
        assert_eq!(normalized.messages[1].tool_calls[0].function.name, "func1");
        assert_eq!(normalized.messages[1].tool_calls[1].function.name, "func2");
        assert_eq!(normalized.messages[1].tool_calls[2].function.name, "func3");
    }
}
