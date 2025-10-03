//! Anthropic ingress adapter

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
        ContentPart, FinishReason, FunctionCall, FunctionDefinition, Message, MessageContent,
        NormalizedRequest, NormalizedResponse, NormalizedStreamEvent, Role, Tool, ToolCall,
    },
    provider::Provider,
};
#[cfg(test)]
use lunaroute_core::normalized::{Choice, Usage};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Maximum size for tool arguments (1MB)
const MAX_TOOL_ARGS_SIZE: usize = 1_000_000;

/// Validate tool input schema (must be valid JSON Schema)
fn validate_tool_schema(schema: &serde_json::Value, tool_name: &str) -> IngressResult<()> {
    // Ensure it's a valid JSON Schema object
    if !schema.is_object() {
        return Err(IngressError::InvalidRequest(
            format!("Tool '{}': input_schema must be a valid JSON Schema object", tool_name)
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

/// Anthropic system parameter (can be string or array of blocks)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AnthropicSystem {
    Text(String),
    Blocks(Vec<AnthropicSystemBlock>),
}

/// Anthropic system content block
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicSystemBlock {
    Text { text: String },
}

/// Anthropic messages request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessagesRequest {
    pub model: String,
    pub messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<AnthropicSystem>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<AnthropicTool>>,
}

/// Anthropic message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<AnthropicMessageContent>,
}

/// Anthropic message content (text string or array of content blocks)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AnthropicMessageContent {
    Text(String),
    Blocks(Vec<AnthropicContentBlock>),
}

/// Anthropic content block (for requests)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
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

/// Anthropic content block (for responses)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicContent {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

/// Anthropic usage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Anthropic tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicTool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

/// Anthropic streaming event
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicStreamEvent {
    MessageStart {
        message: AnthropicMessageStart,
    },
    ContentBlockStart {
        index: u32,
        content_block: AnthropicContentBlockStart,
    },
    ContentBlockDelta {
        index: u32,
        delta: AnthropicContentDelta,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        delta: AnthropicMessageDelta,
        usage: AnthropicUsageDelta,
    },
    MessageStop,
}

/// Anthropic message start
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessageStart {
    pub id: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub role: String,
    pub model: String,
    pub usage: AnthropicUsage,
}

/// Anthropic content block start
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicContentBlockStart {
    Text { text: String },
    ToolUse { id: String, name: String },
}

/// Anthropic content delta
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicContentDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
}

/// Anthropic message delta
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessageDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequence: Option<String>,
}

/// Anthropic usage delta
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicUsageDelta {
    pub output_tokens: u32,
}

/// Validate Anthropic request parameters
fn validate_request(req: &AnthropicMessagesRequest) -> IngressResult<()> {
    // Validate temperature (0.0 to 1.0 for Anthropic)
    if let Some(temp) = req.temperature
        && !(0.0..=1.0).contains(&temp) {
            return Err(IngressError::InvalidRequest(
                format!("temperature must be between 0.0 and 1.0, got {}", temp)
            ));
        }

    // Validate top_p (0.0 to 1.0)
    if let Some(top_p) = req.top_p
        && !(0.0..=1.0).contains(&top_p) {
            return Err(IngressError::InvalidRequest(
                format!("top_p must be between 0.0 and 1.0, got {}", top_p)
            ));
        }

    // Validate top_k (positive integer)
    if let Some(top_k) = req.top_k
        && top_k == 0 {
            return Err(IngressError::InvalidRequest(
                "top_k must be greater than 0".to_string()
            ));
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

            let content = msg.content.unwrap_or(AnthropicMessageContent::Text(String::new()));

            // Parse content and extract tool calls
            let (text_content, tool_calls, tool_call_id) = match content {
                AnthropicMessageContent::Text(text) => {
                    // Validate message content length
                    if text.len() > 1_000_000 {
                        return Err(IngressError::InvalidRequest(
                            format!("Message content too large: {} bytes (max 1MB)", text.len())
                        ));
                    }
                    (text, vec![], None)
                },
                AnthropicMessageContent::Blocks(blocks) => {
                    let mut text_parts = Vec::new();
                    let mut tool_calls_vec = Vec::new();
                    let mut tool_result_id = None;

                    for block in blocks {
                        match block {
                            AnthropicContentBlock::Text { text } => {
                                text_parts.push(text);
                            },
                            AnthropicContentBlock::ToolUse { id, name, input } => {
                                let arguments = serde_json::to_string(&input)
                                    .map_err(|e| {
                                        tracing::warn!("Failed to serialize tool input: {}", e);
                                        IngressError::InvalidRequest(format!(
                                            "Invalid tool input for '{}': {}", name, e
                                        ))
                                    })?;

                                // Validate tool arguments size
                                if arguments.len() > MAX_TOOL_ARGS_SIZE {
                                    return Err(IngressError::InvalidRequest(
                                        format!("Tool arguments too large for '{}': {} bytes (max 1MB)",
                                                name, arguments.len())
                                    ));
                                }

                                tool_calls_vec.push(ToolCall {
                                    id,
                                    tool_type: "function".to_string(),
                                    function: FunctionCall {
                                        name,
                                        arguments,
                                    },
                                });
                            },
                            AnthropicContentBlock::ToolResult { tool_use_id, content } => {
                                tool_result_id = Some(tool_use_id);
                                text_parts.push(content);
                            },
                        }
                    }

                    (text_parts.join("\n"), tool_calls_vec, tool_result_id)
                }
            };

            Ok(Message {
                role,
                content: MessageContent::Text(text_content),
                name: None,
                tool_calls,
                tool_call_id,
            })
        })
        .collect();

    // Convert tools with validation
    let tools = if let Some(tools) = req.tools {
        let result: Result<Vec<Tool>, IngressError> = tools.into_iter().map(|tool| {
            // Validate tool input schema
            validate_tool_schema(&tool.input_schema, &tool.name)?;

            Ok(Tool {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: tool.name,
                    description: tool.description,
                    parameters: tool.input_schema,
                },
            })
        }).collect();
        result?
    } else {
        vec![]
    };

    // Convert system parameter to string (supports both string and array formats)
    let system = req.system.map(|sys| match sys {
        AnthropicSystem::Text(text) => text,
        AnthropicSystem::Blocks(blocks) => {
            // Concatenate all text blocks
            blocks
                .into_iter()
                .map(|block| match block {
                    AnthropicSystemBlock::Text { text } => text,
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
    });

    Ok(NormalizedRequest {
        messages: messages?,
        system,
        model: req.model,
        max_tokens: req.max_tokens,
        temperature: req.temperature,
        top_p: req.top_p,
        top_k: req.top_k,
        stop_sequences: req.stop_sequences.unwrap_or_default(),
        stream: req.stream.unwrap_or(false),
        tools,
        tool_choice: None, // Anthropic doesn't have tool_choice in same way
        metadata: std::collections::HashMap::new(),
    })
}

/// Convert normalized response to Anthropic response
pub fn from_normalized(resp: NormalizedResponse) -> AnthropicResponse {
    let content: Vec<AnthropicContent> = resp
        .choices
        .first()
        .map(|choice| {
            let mut content_blocks = Vec::new();

            // Add text content if present
            match &choice.message.content {
                MessageContent::Text(text) => {
                    if !text.is_empty() {
                        content_blocks.push(AnthropicContent::Text {
                            text: text.clone(),
                        });
                    }
                },
                MessageContent::Parts(parts) => {
                    // Extract text parts from multimodal content
                    // Anthropic responses with images would need specific handling,
                    // but for now we extract text parts
                    for part in parts {
                        match part {
                            ContentPart::Text { text } => {
                                if !text.is_empty() {
                                    content_blocks.push(AnthropicContent::Text {
                                        text: text.clone(),
                                    });
                                }
                            },
                            ContentPart::Image { .. } => {
                                tracing::warn!("Image content in response not supported for Anthropic format, skipping");
                            }
                        }
                    }
                },
            }

            // Add tool_use blocks
            for tool_call in &choice.message.tool_calls {
                // Parse arguments back to JSON
                let input = match serde_json::from_str(&tool_call.function.arguments) {
                    Ok(value) => value,
                    Err(e) => {
                        tracing::error!(
                            "Failed to parse tool call arguments for '{}': {}. Using empty object.",
                            tool_call.function.name, e
                        );
                        serde_json::Value::Object(serde_json::Map::new())
                    }
                };

                content_blocks.push(AnthropicContent::ToolUse {
                    id: tool_call.id.clone(),
                    name: tool_call.function.name.clone(),
                    input,
                });
            }

            content_blocks
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
        // Use ID as-is (egress already has proper format from provider)
        id: resp.id,
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

/// Convert normalized stream event to Anthropic SSE events
/// Returns a vector because some normalized events map to multiple Anthropic events
///
/// State Management:
/// - `content_block_started` tracks whether we've sent a ContentBlockStart event
/// - This ensures proper event sequencing: Start → Delta → Stop
/// - State is maintained per-stream via scan() combinator (sequential, no concurrency)
fn stream_event_to_anthropic_events(
    event: NormalizedStreamEvent,
    stream_id: &str,
    model: &str,
    content_block_started: &mut bool,
) -> Vec<AnthropicStreamEvent> {
    use tracing::debug;
    match event {
        NormalizedStreamEvent::Start { .. } => {
            // Reset state for new stream
            *content_block_started = false;
            debug!("Anthropic stream started: stream_id={}", stream_id);

            // Anthropic starts with message_start event
            vec![AnthropicStreamEvent::MessageStart {
                message: AnthropicMessageStart {
                    id: stream_id.to_string(),
                    type_: "message".to_string(),
                    role: "assistant".to_string(),
                    model: model.to_string(),
                    usage: AnthropicUsage {
                        input_tokens: 0,
                        output_tokens: 0,
                    },
                },
            }]
        }
        NormalizedStreamEvent::Delta { index, delta } => {
            let mut events = Vec::new();

            // For first delta, send content_block_start
            if !*content_block_started {
                events.push(AnthropicStreamEvent::ContentBlockStart {
                    index,
                    content_block: AnthropicContentBlockStart::Text {
                        text: String::new(),
                    },
                });
                *content_block_started = true;
            }

            // Send the text delta
            if let Some(content) = delta.content {
                events.push(AnthropicStreamEvent::ContentBlockDelta {
                    index,
                    delta: AnthropicContentDelta::TextDelta { text: content },
                });
            }

            events
        }
        NormalizedStreamEvent::ToolCallDelta {
            index: _,
            tool_call_index,
            id,
            function,
        } => {
            let mut events = Vec::new();

            // Start tool use block if we have id and name
            if let (Some(tool_id), Some(func)) = (id, function.as_ref().and_then(|f| f.name.clone())) {
                events.push(AnthropicStreamEvent::ContentBlockStart {
                    index: tool_call_index,
                    content_block: AnthropicContentBlockStart::ToolUse {
                        id: tool_id,
                        name: func,
                    },
                });
            }

            // Send partial JSON if available
            if let Some(func) = function.and_then(|f| f.arguments) {
                events.push(AnthropicStreamEvent::ContentBlockDelta {
                    index: tool_call_index,
                    delta: AnthropicContentDelta::InputJsonDelta { partial_json: func },
                });
            }

            events
        }
        NormalizedStreamEvent::Usage { .. } => {
            // Usage is sent with message_delta at the end
            vec![]
        }
        NormalizedStreamEvent::End { finish_reason } => {
            let mut events = Vec::new();

            // Close content block if it was started
            if *content_block_started {
                events.push(AnthropicStreamEvent::ContentBlockStop { index: 0 });
                *content_block_started = false;
                debug!("Anthropic content block closed");
            } else {
                debug!("Anthropic stream ended without content block");
            }

            // Send message_delta with stop_reason
            let stop_reason = match finish_reason {
                FinishReason::Stop => "end_turn",
                FinishReason::Length => "max_tokens",
                FinishReason::ToolCalls => "tool_use",
                FinishReason::ContentFilter => "end_turn",
                FinishReason::Error => "end_turn",
            };

            events.push(AnthropicStreamEvent::MessageDelta {
                delta: AnthropicMessageDelta {
                    stop_reason: Some(stop_reason.to_string()),
                    stop_sequence: None,
                },
                usage: AnthropicUsageDelta { output_tokens: 0 },
            });

            // Send message_stop
            events.push(AnthropicStreamEvent::MessageStop);

            events
        }
        NormalizedStreamEvent::Error { .. } => {
            // Errors are handled separately
            vec![]
        }
    }
}

/// Messages handler
pub async fn messages(
    State(provider): State<Arc<dyn Provider>>,
    Json(req): Json<AnthropicMessagesRequest>,
) -> Result<Response, IngressError> {
    let start_time = std::time::Instant::now();
    let is_streaming = req.stream.unwrap_or(false);
    let model = req.model.clone();

    // Convert to normalized format (includes validation)
    let normalized = to_normalized(req)?;

    if is_streaming {
        // Log streaming request for observability
        tracing::debug!(
            "Anthropic streaming request: model={}, messages={}",
            model,
            normalized.messages.len()
        );

        // Validate that provider supports streaming
        if !provider.capabilities().supports_streaming {
            return Err(IngressError::UnsupportedFeature(
                "Provider does not support streaming".to_string(),
            ));
        }

        // Call provider.stream()
        let stream = provider
            .stream(normalized)
            .await
            .map_err(|e| IngressError::ProviderError(e.to_string()))?;

        // Generate stream ID (Anthropic uses msg_* prefix) and wrap in Arc for efficient sharing
        let stream_id = Arc::new(format!("msg_{}", uuid::Uuid::new_v4().simple()));
        let model = Arc::new(model);

        // Convert normalized events to Anthropic SSE format
        // Use scan to maintain state across stream events
        let sse_stream = stream
            .scan(false, move |content_block_started, result| {
                let stream_id = Arc::clone(&stream_id);
                let model = Arc::clone(&model);

                let events = match result {
                    Ok(event) => {
                        let anthropic_events = stream_event_to_anthropic_events(
                            event,
                            stream_id.as_str(),
                            model.as_str(),
                            content_block_started,
                        );

                        anthropic_events
                            .into_iter()
                            .map(|evt| {
                                match Event::default().json_data(evt) {
                                    Ok(event) => Ok::<_, IngressError>(event),
                                    Err(e) => Err(IngressError::Internal(format!(
                                        "Failed to create SSE event: {}",
                                        e
                                    ))),
                                }
                            })
                            .collect::<Vec<_>>()
                    }
                    Err(e) => {
                        // Send error event
                        let error_event = serde_json::json!({
                            "type": "error",
                            "error": {
                                "type": "api_error",
                                "message": e.to_string()
                            }
                        });
                        match Event::default().json_data(error_event) {
                            Ok(event) => vec![Ok(event)],
                            Err(_) => vec![Err(IngressError::Internal(
                                "Failed to create error SSE event".to_string()
                            ))],
                        }
                    }
                };

                futures::future::ready(Some(events))
            })
            .flat_map(futures::stream::iter);

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

        // Call provider
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

        // Convert back to Anthropic format
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

/// State for passthrough handler (connector + optional stats tracker + metrics + session recorder)
pub struct PassthroughState {
    pub connector: Arc<lunaroute_egress::anthropic::AnthropicConnector>,
    pub stats_tracker: Option<Arc<dyn crate::types::SessionStatsTracker>>,
    pub metrics: Option<Arc<lunaroute_observability::Metrics>>,
    pub session_recorder: Option<Arc<lunaroute_session::MultiWriterRecorder>>,
}

/// Passthrough handler for Anthropic→Anthropic routing (no normalization)
/// Takes raw JSON, sends directly to Anthropic, returns raw JSON
/// Preserves 100% API fidelity while still extracting metrics
pub async fn messages_passthrough(
    State(state): State<Arc<PassthroughState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<serde_json::Value>,
) -> Result<Response, IngressError> {
    let start_time = std::time::Instant::now();
    tracing::debug!("Anthropic passthrough mode: skipping normalization");

    // Extract session ID from metadata
    let session_id = req
        .get("metadata")
        .and_then(|m| m.get("user_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Pass through ALL headers from the client (except hop-by-hop headers)
    // This allows client to provide auth headers if no API key is configured
    let mut passthrough_headers = std::collections::HashMap::new();

    // Headers that should NOT be forwarded (hop-by-hop headers per RFC 7230)
    let skip_headers = [
        "connection", "keep-alive", "proxy-authenticate", "proxy-authorization",
        "te", "trailers", "transfer-encoding", "upgrade", "host", "content-length"
    ];

    for (name, value) in headers.iter() {
        let name_str = name.as_str().to_lowercase();

        // Skip hop-by-hop headers
        if skip_headers.contains(&name_str.as_str()) {
            continue;
        }

        // Forward all other headers including authorization
        if let Ok(value_str) = value.to_str() {
            passthrough_headers.insert(name.as_str().to_string(), value_str.to_string());
        }
    }

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
            provider: "anthropic".to_string(),
            listener: "anthropic".to_string(),
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
            .stream_passthrough(req, passthrough_headers)
            .await
            .map_err(|e| IngressError::ProviderError(e.to_string()))?;

        // Track streaming metrics using shared module
        use futures::StreamExt;
        use crate::streaming_metrics::StreamingMetricsTracker;

        let tracker = StreamingMetricsTracker::new(before_provider);
        let tracker_ref = tracker.clone();
        let tracker_for_finalize = tracker.clone();
        let metrics_clone = state.metrics.clone();
        let model_clone = model.clone();

        let byte_stream = stream_response.bytes_stream();
        let sse_stream = eventsource_stream::EventStream::new(byte_stream);

        let tracked_stream = sse_stream.map(move |event_result| {
            match event_result {
                Ok(event) => {
                    let now = std::time::Instant::now();

                    // Track TTFT on first chunk
                    tracker_ref.record_ttft(now);

                    // Track chunk latency with memory bounds
                    let _ = tracker_ref.record_chunk_latency(now, "anthropic", &model_clone, &metrics_clone);

                    // Increment chunk count
                    tracker_ref.increment_chunk_count();

                    // Parse event data to extract text and metadata
                    if let Ok(anthropic_event) = serde_json::from_str::<serde_json::Value>(&event.data) {
                        // Extract text deltas (with size limit to prevent OOM)
                        if let Some("content_block_delta") = anthropic_event.get("type").and_then(|t| t.as_str())
                            && let Some("text_delta") = anthropic_event.get("delta").and_then(|d| d.get("type")).and_then(|t| t.as_str())
                            && let Some(text) = anthropic_event.get("delta").and_then(|d| d.get("text")).and_then(|t| t.as_str())
                        {
                            tracker_ref.accumulate_text(text, "anthropic", &model_clone, &metrics_clone);
                        }

                        // Extract model from message_start
                        if let Some("message_start") = anthropic_event.get("type").and_then(|t| t.as_str())
                            && let Some(model_str) = anthropic_event.get("message").and_then(|m| m.get("model")).and_then(|m| m.as_str())
                        {
                            tracker_ref.set_model(model_str.to_string());
                        }

                        // Extract finish reason from message_delta
                        if let Some("message_delta") = anthropic_event.get("type").and_then(|t| t.as_str())
                            && let Some(reason) = anthropic_event.get("delta").and_then(|d| d.get("stop_reason")).and_then(|r| r.as_str())
                        {
                            tracker_ref.set_finish_reason(reason.to_string());
                        }
                    }

                    // Forward the event
                    match serde_json::from_str::<serde_json::Value>(&event.data) {
                        Ok(json) => Event::default().json_data(json)
                            .map_err(|e| IngressError::Internal(format!("Failed to create SSE event: {}", e))),
                        Err(e) => Err(IngressError::Internal(format!("Failed to parse SSE event data: {}", e)))
                    }
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

        let completion_stream = tracked_stream.chain(futures::stream::once(async move {
            // Record StreamStarted and Completed events after stream ends
            if let (Some(recorder), Some(session_id), Some(request_id)) =
                (recorder_clone.as_ref(), session_id_clone.as_ref(), request_id_clone.as_ref())
            {
                use lunaroute_session::{SessionEvent, events::{FinalSessionStats, TokenTotals, ToolUsageSummary, PerformanceMetrics}};

                // Finalize streaming metrics using tracker
                let finalized = tracker_for_finalize.finalize(start_clone, before_provider_clone);

                // Record Prometheus metrics
                finalized.record_to_prometheus(&state.metrics, "anthropic", &model);

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

    // Send directly to Anthropic API (non-streaming)
    let response_result = state.connector
        .send_passthrough(req, passthrough_headers)
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
    let (input_tokens, output_tokens, thinking_tokens) = if let Some(usage) = response.get("usage") {
        let input = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let output = usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);

        // Extract thinking tokens - check multiple possible field names
        // The actual field name depends on API version and features used
        let thinking = usage.get("thinking_tokens")
            .and_then(|v| v.as_u64())
            .or_else(|| usage.get("cache_read_input_tokens").and_then(|v| v.as_u64()))
            .or_else(|| usage.get("extended_thinking_tokens").and_then(|v| v.as_u64()))
            .unwrap_or(0);

        if input > 0 || output > 0 {
            if thinking > 0 {
                tracing::debug!(
                    "Passthrough metrics: input_tokens={}, output_tokens={}, thinking_tokens={}, total={}",
                    input,
                    output,
                    thinking,
                    input + output + thinking
                );
            } else {
                tracing::debug!(
                    "Passthrough metrics: input_tokens={}, output_tokens={}, total={}",
                    input,
                    output,
                    input + output
                );
            }
        }
        (input, output, thinking)
    } else {
        (0, 0, 0)
    };

    // Extract tool calls from response content
    let mut tool_calls = std::collections::HashMap::new();
    if let Some(content) = response.get("content").and_then(|c| c.as_array()) {
        for block in content {
            if let Some("tool_use") = block.get("type").and_then(|t| t.as_str())
                && let Some(tool_name) = block.get("name").and_then(|n| n.as_str())
            {
                *tool_calls.entry(tool_name.to_string()).or_insert(0) += 1;
            }
        }
    }

    if !tool_calls.is_empty() {
        let total_tools: u64 = tool_calls.values().sum();
        tracing::debug!("Tool calls: {} total across {} tools", total_tools, tool_calls.len());
    }

    // Record session response if enabled (using async events)
    if let (Some(recorder), Some(session_id), Some(request_id)) =
        (&state.session_recorder, &recording_session_id, &recording_request_id)
        && let Ok(anthropic_resp) = serde_json::from_value::<AnthropicResponse>(response.clone()) {
            use lunaroute_session::{SessionEvent, events::{ResponseStats, TokenStats, FinalSessionStats, TokenTotals, ToolUsageSummary, PerformanceMetrics}};

            // Extract response text
            let response_text = anthropic_resp.content.iter().filter_map(|c| {
                if let AnthropicContent::Text { text } = c {
                    Some(text.clone())
                } else {
                    None
                }
            }).collect::<Vec<_>>().join("\n");

            // Record response event
            recorder.record_event(SessionEvent::ResponseRecorded {
                session_id: session_id.clone(),
                request_id: request_id.clone(),
                timestamp: chrono::Utc::now(),
                response_text: response_text.clone(),
                response_json: response.clone(),
                model_used: anthropic_resp.model.clone(),
                stats: ResponseStats {
                    provider_latency_ms: provider_time.as_millis() as u64,
                    post_processing_ms: 0.0,
                    total_proxy_overhead_ms: 0.0,
                    tokens: TokenStats {
                        input_tokens: input_tokens as u32,
                        output_tokens: output_tokens as u32,
                        thinking_tokens: if thinking_tokens > 0 { Some(thinking_tokens as u32) } else { None },
                        cache_read_tokens: None,
                        cache_write_tokens: None,
                        total_tokens: (input_tokens + output_tokens) as u32,
                        thinking_percentage: None,
                        tokens_per_second: None,
                    },
                    tool_calls: vec![],
                    response_size_bytes: response_text.len(),
                    content_blocks: anthropic_resp.content.len(),
                    has_refusal: false,
                    is_streaming: false,
                    chunk_count: None,
                    streaming_duration_ms: None,
                },
            });

            // Record completion event
            recorder.record_event(SessionEvent::Completed {
                session_id: session_id.clone(),
                request_id: request_id.clone(),
                timestamp: chrono::Utc::now(),
                success: true,
                error: None,
                finish_reason: anthropic_resp.stop_reason.clone(),
                final_stats: Box::new(FinalSessionStats {
                    total_duration_ms: start_time.elapsed().as_millis() as u64,
                    provider_time_ms: provider_time.as_millis() as u64,
                    proxy_overhead_ms: 0.0,
                    total_tokens: TokenTotals {
                        total_input: input_tokens,
                        total_output: output_tokens,
                        total_thinking: thinking_tokens,
                        total_cached: 0,
                        grand_total: input_tokens + output_tokens,
                        by_model: Default::default(),
                    },
                    tool_summary: ToolUsageSummary {
                        total_tool_calls: 0,
                        unique_tool_count: 0,
                        by_tool: Default::default(),
                        total_tool_time_ms: 0,
                        tool_error_count: 0,
                    },
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

    // Record metrics if available
    if let Some(metrics) = state.metrics.as_ref() {
        let total_time = start_time.elapsed().as_secs_f64();

        if response_result.is_ok() {
            metrics.record_request_success("anthropic", &model, "anthropic", total_time);
            metrics.record_tokens("anthropic", &model, input_tokens as u32, output_tokens as u32);
        } else {
            metrics.record_request_failure(
                "anthropic",
                &model,
                "anthropic",
                "provider_error",
                total_time,
            );
        }

        // Record tool calls
        for (tool_name, count) in &tool_calls {
            for _ in 0..*count {
                metrics.record_tool_call("anthropic", &model, tool_name);
            }
        }

        // Record processing times
        metrics.record_post_processing(post_provider_overhead.as_secs_f64());
        metrics.record_proxy_overhead((pre_provider_overhead + post_provider_overhead).as_secs_f64());
    }

    // Record stats if tracker is available and we have a session ID
    if let (Some(tracker), Some(sid)) = (state.stats_tracker.as_ref(), session_id) {
        tracker.record_request(
            sid,
            crate::types::SessionRequestStats {
                input_tokens,
                output_tokens,
                thinking_tokens,
                tool_calls,
                pre_proxy_time: pre_provider_overhead,
                post_proxy_time: post_provider_overhead,
            },
        );
    }

    response_result
}

/// Create Anthropic router with provider state
pub fn router(provider: Arc<dyn Provider>) -> Router {
    Router::new()
        .route("/v1/messages", post(messages))
        .with_state(provider)
}

/// Create Anthropic passthrough router (for Anthropic→Anthropic direct routing)
pub fn passthrough_router(
    connector: Arc<lunaroute_egress::anthropic::AnthropicConnector>,
    stats_tracker: Option<Arc<dyn crate::types::SessionStatsTracker>>,
    metrics: Option<Arc<lunaroute_observability::Metrics>>,
    session_recorder: Option<Arc<lunaroute_session::MultiWriterRecorder>>,
) -> Router {
    let state = Arc::new(PassthroughState {
        connector,
        stats_tracker,
        metrics,
        session_recorder,
    });

    Router::new()
        .route("/v1/messages", post(messages_passthrough))
        .with_state(state)
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
                model: "claude-3-opus".to_string(),
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
        let req = AnthropicMessagesRequest {
            model: "claude-3-opus".to_string(),
            messages: vec![
                AnthropicMessage {
                    role: "user".to_string(),
                    content: Some(AnthropicMessageContent::Text("Hello!".to_string())),
                },
            ],
            system: Some(AnthropicSystem::Text("You are a helpful assistant".to_string())),
            max_tokens: Some(1024),
            temperature: Some(0.7),
            top_p: None,
            top_k: None,
            stream: Some(false),
            stop_sequences: None,
            tools: None,
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
                content: Some(AnthropicMessageContent::Text("test".to_string())),
            }],
            system: None,
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stream: None,
            stop_sequences: None,
            tools: None,
        };

        let result = to_normalized(req);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_messages_endpoint() {
        let provider = Arc::new(MockProvider);
        let req = AnthropicMessagesRequest {
            model: "claude-3-opus".to_string(),
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: Some(AnthropicMessageContent::Text("Hello!".to_string())),
            }],
            system: None,
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stream: None,
            stop_sequences: None,
            tools: None,
        };

        let response = messages(State(provider), Json(req)).await;
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
            tools: None,
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
                content: Some(AnthropicMessageContent::Text("Hello!".to_string())),
            }],
            system: Some(AnthropicSystem::Text("You are a helpful assistant".to_string())),
            max_tokens: Some(1024),
            temperature: Some(0.7),
            top_p: Some(0.9),
            top_k: Some(40),
            stream: Some(true),
            stop_sequences: Some(vec!["STOP".to_string()]),
            tools: None,
        };

        let normalized = to_normalized(req).unwrap();
        assert_eq!(normalized.system, Some("You are a helpful assistant".to_string()));
        assert_eq!(normalized.max_tokens, Some(1024));
        assert_eq!(normalized.temperature, Some(0.7));
        assert_eq!(normalized.top_p, Some(0.9));
        assert_eq!(normalized.top_k, Some(40));
        assert!(normalized.stream);
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
        // Now properly extracts text from multimodal Parts
        assert_eq!(anthropic.content.len(), 1);
        if let AnthropicContent::Text { text } = &anthropic.content[0] {
            assert_eq!(text, "I see an image");
        } else {
            panic!("Expected Text content block");
        }
    }

    #[test]
    fn test_anthropic_router_creation() {
        let provider = Arc::new(MockProvider);
        let router = router(provider);
        // Just verify it creates without panicking
        // The router is properly configured with /v1/messages endpoint
        drop(router);
    }

    #[test]
    fn test_anthropic_system_array_format() {
        // Test the array format for system parameter (used by Claude Code)
        let req = AnthropicMessagesRequest {
            model: "claude-3-opus".to_string(),
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: Some(AnthropicMessageContent::Text("Hello!".to_string())),
            }],
            system: Some(AnthropicSystem::Blocks(vec![
                AnthropicSystemBlock::Text {
                    text: "You are a helpful assistant.".to_string(),
                },
                AnthropicSystemBlock::Text {
                    text: "You are concise.".to_string(),
                },
            ])),
            max_tokens: Some(1024),
            temperature: None,
            top_p: None,
            top_k: None,
            stream: Some(false),
            stop_sequences: None,
            tools: None,
        };

        let normalized = to_normalized(req).unwrap();
        // Multiple blocks should be joined with newlines
        assert_eq!(
            normalized.system,
            Some("You are a helpful assistant.\nYou are concise.".to_string())
        );
    }

    #[test]
    fn test_anthropic_system_deserialize_string() {
        // Test that string format still works
        let json = r#"{"model":"claude-3-opus","messages":[],"system":"You are helpful"}"#;
        let req: AnthropicMessagesRequest = serde_json::from_str(json).unwrap();
        match req.system {
            Some(AnthropicSystem::Text(text)) => assert_eq!(text, "You are helpful"),
            _ => panic!("Expected Text variant"),
        }
    }

    #[test]
    fn test_anthropic_system_deserialize_array() {
        // Test that array format works (Claude Code format)
        let json = r#"{"model":"claude-3-opus","messages":[],"system":[{"type":"text","text":"You are helpful"}]}"#;
        let req: AnthropicMessagesRequest = serde_json::from_str(json).unwrap();
        match req.system {
            Some(AnthropicSystem::Blocks(blocks)) => {
                assert_eq!(blocks.len(), 1);
                match &blocks[0] {
                    AnthropicSystemBlock::Text { text } => assert_eq!(text, "You are helpful"),
                }
            }
            _ => panic!("Expected Blocks variant"),
        }
    }
}
