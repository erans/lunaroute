//! Anthropic egress connector

use crate::{
    client::{create_client, with_retry, HttpClientConfig},
    EgressError, Result,
};
use async_trait::async_trait;
use futures::Stream;
use lunaroute_core::{
    normalized::{
        ContentPart, Delta, FinishReason, FunctionCall, FunctionCallDelta, Message,
        MessageContent, NormalizedRequest, NormalizedResponse, NormalizedStreamEvent, Role,
        ToolCall, Usage,
    },
    provider::{Provider, ProviderCapabilities},
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use tracing::{debug, instrument};

/// Anthropic connector configuration
#[derive(Debug, Clone)]
pub struct AnthropicConfig {
    /// API key for authentication
    pub api_key: String,

    /// Base URL for Anthropic API (default: https://api.anthropic.com)
    pub base_url: String,

    /// Anthropic API version (default: 2023-06-01)
    pub api_version: String,

    /// HTTP client configuration
    pub client_config: HttpClientConfig,
}

impl AnthropicConfig {
    /// Create a new Anthropic configuration
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: "https://api.anthropic.com".to_string(),
            api_version: "2023-06-01".to_string(),
            client_config: HttpClientConfig::default(),
        }
    }

    /// Set the base URL (for custom endpoints)
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Set the API version
    pub fn with_api_version(mut self, api_version: impl Into<String>) -> Self {
        self.api_version = api_version.into();
        self
    }
}

/// Anthropic connector
pub struct AnthropicConnector {
    config: AnthropicConfig,
    client: Client,
}

impl AnthropicConnector {
    /// Create a new Anthropic connector
    pub fn new(config: AnthropicConfig) -> Result<Self> {
        let client = create_client(&config.client_config)?;
        Ok(Self { config, client })
    }

    /// Send a raw JSON request directly to Anthropic (passthrough mode)
    /// This skips normalization for Anthropic→Anthropic routing, preserving 100% API fidelity.
    /// Still parses the response to extract metrics (tokens, model, etc.)
    #[instrument(skip(self, request_json, headers))]
    pub async fn send_passthrough(
        &self,
        request_json: serde_json::Value,
        headers: std::collections::HashMap<String, String>,
    ) -> Result<(serde_json::Value, std::collections::HashMap<String, String>)> {
        debug!("Sending passthrough request to Anthropic (no normalization)");

        // Log request headers and body at debug level
        debug!("┌─────────────────────────────────────────────────────────");
        debug!("│ Anthropic Passthrough Request Headers");
        debug!("├─────────────────────────────────────────────────────────");
        debug!("│ x-api-key: <api_key>");
        for (name, value) in &headers {
            debug!("│ {}: {}", name, value);
        }
        debug!("│ Content-Type: application/json");
        debug!("├─────────────────────────────────────────────────────────");
        debug!("│ Anthropic Passthrough Request Body");
        debug!("├─────────────────────────────────────────────────────────");
        debug!("│ {}", serde_json::to_string_pretty(&request_json).unwrap_or_else(|_| "Failed to serialize".to_string()));
        debug!("└─────────────────────────────────────────────────────────");

        let max_retries = self.config.client_config.max_retries;
        let result = with_retry(max_retries, || {
            let request_json = request_json.clone();
            let headers = headers.clone();
            let config_api_key = self.config.api_key.clone();
            async move {
                let mut request_builder = self.client
                    .post(format!("{}/v1/messages", self.config.base_url))
                    .header("Content-Type", "application/json");

                // Add all passthrough headers first
                for (name, value) in &headers {
                    request_builder = request_builder.header(name, value);
                }

                // If we have a configured API key, override any client-provided auth
                // If not, rely on client's x-api-key or authorization header
                if !config_api_key.is_empty() {
                    request_builder = request_builder.header("x-api-key", &config_api_key);
                }

                let response = request_builder
                    .json(&request_json)
                    .send()
                    .await?;

                // Log response headers at debug level
                debug!("┌─────────────────────────────────────────────────────────");
                debug!("│ Anthropic Passthrough Response Headers");
                debug!("├─────────────────────────────────────────────────────────");
                debug!("│ Status: {}", response.status());
                for (name, value) in response.headers() {
                    if let Ok(val_str) = value.to_str() {
                        debug!("│ {}: {}", name, val_str);
                    }
                }
                debug!("└─────────────────────────────────────────────────────────");

                self.handle_anthropic_passthrough_response(response).await
            }
        })
        .await?;

        Ok(result)
    }

    /// Handle passthrough response (for send_passthrough)
    /// Returns (json_body, headers_map)
    async fn handle_anthropic_passthrough_response(
        &self,
        response: reqwest::Response,
    ) -> Result<(serde_json::Value, std::collections::HashMap<String, String>)> {
        let status = response.status();

        if !status.is_success() {
            let status_code = status.as_u16();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());

            return Err(if status_code == 429 {
                EgressError::RateLimitExceeded {
                    retry_after_secs: None,
                }
            } else {
                EgressError::ProviderError {
                    status_code,
                    message: body,
                }
            });
        }

        // Capture response headers before consuming the response
        let mut headers_map = std::collections::HashMap::new();
        for (name, value) in response.headers() {
            if let Ok(value_str) = value.to_str() {
                headers_map.insert(name.to_string(), value_str.to_string());
            }
        }

        let json_body = response
            .json::<serde_json::Value>()
            .await
            .map_err(|e| {
                EgressError::ParseError(format!("Failed to parse Anthropic passthrough response: {}", e))
            })?;

        Ok((json_body, headers_map))
    }

    /// Stream raw Anthropic request (passthrough mode - no normalization)
    /// Returns raw response for direct SSE forwarding
    #[instrument(skip(self, request_json, headers))]
    pub async fn stream_passthrough(
        &self,
        request_json: serde_json::Value,
        headers: std::collections::HashMap<String, String>,
    ) -> Result<reqwest::Response> {
        debug!("Sending passthrough streaming request to Anthropic (no normalization)");

        // Log request headers and body at debug level
        debug!("┌─────────────────────────────────────────────────────────");
        debug!("│ Anthropic Streaming Passthrough Request Headers");
        debug!("├─────────────────────────────────────────────────────────");
        debug!("│ x-api-key: <api_key>");
        for (name, value) in &headers {
            debug!("│ {}: {}", name, value);
        }
        debug!("│ Content-Type: application/json");
        debug!("├─────────────────────────────────────────────────────────");
        debug!("│ Anthropic Streaming Passthrough Request Body");
        debug!("├─────────────────────────────────────────────────────────");
        debug!("│ {}", serde_json::to_string_pretty(&request_json).unwrap_or_else(|_| "Failed to serialize".to_string()));
        debug!("└─────────────────────────────────────────────────────────");

        let mut request_builder = self
            .client
            .post(format!("{}/v1/messages", self.config.base_url))
            .header("Content-Type", "application/json");

        // Add all passthrough headers first
        for (name, value) in &headers {
            request_builder = request_builder.header(name, value);
        }

        // If we have a configured API key, override any client-provided auth
        // If not, rely on client's x-api-key or authorization header
        if !self.config.api_key.is_empty() {
            request_builder = request_builder.header("x-api-key", &self.config.api_key);
        }

        let response = request_builder
            .json(&request_json)
            .send()
            .await
            .map_err(EgressError::from)?;

        // Log response headers at debug level
        debug!("┌─────────────────────────────────────────────────────────");
        debug!("│ Anthropic Streaming Passthrough Response Headers");
        debug!("├─────────────────────────────────────────────────────────");
        debug!("│ Status: {}", response.status());
        for (name, value) in response.headers() {
            if let Ok(val_str) = value.to_str() {
                debug!("│ {}: {}", name, val_str);
            }
        }
        debug!("└─────────────────────────────────────────────────────────");

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());
            return Err(EgressError::ProviderError {
                status_code: status,
                message: body,
            });
        }

        Ok(response)
    }
}

#[async_trait]
impl Provider for AnthropicConnector {
    #[instrument(skip(self, request), fields(model = %request.model))]
    async fn send(
        &self,
        request: NormalizedRequest,
    ) -> lunaroute_core::Result<NormalizedResponse> {
        debug!("Sending non-streaming request to Anthropic");

        let anthropic_req = to_anthropic_request(request)?;

        // Log request headers at debug level
        debug!("┌─────────────────────────────────────────────────────────");
        debug!("│ Anthropic Request Headers");
        debug!("├─────────────────────────────────────────────────────────");
        debug!("│ x-api-key: <api_key>");
        debug!("│ anthropic-version: {}", self.config.api_version);
        debug!("│ Content-Type: application/json");
        debug!("└─────────────────────────────────────────────────────────");

        let max_retries = self.config.client_config.max_retries;
        let result = with_retry(max_retries, || {
            let anthropic_req = anthropic_req.clone();
            async move {
                let response = self.client
                    .post(format!("{}/v1/messages", self.config.base_url))
                    .header("x-api-key", &self.config.api_key)
                    .header("anthropic-version", &self.config.api_version)
                    .header("Content-Type", "application/json")
                    .json(&anthropic_req)
                    .send()
                    .await?;

                // Log response headers at debug level
                debug!("┌─────────────────────────────────────────────────────────");
                debug!("│ Anthropic Response Headers");
                debug!("├─────────────────────────────────────────────────────────");
                debug!("│ Status: {}", response.status());
                for (name, value) in response.headers() {
                    if let Ok(val_str) = value.to_str() {
                        debug!("│ {}: {}", name, val_str);
                    }
                }
                debug!("└─────────────────────────────────────────────────────────");

                response.handle_anthropic_response().await
            }
        })
        .await?;

        let normalized = from_anthropic_response(result)?;
        Ok(normalized)
    }

    async fn stream(
        &self,
        request: NormalizedRequest,
    ) -> lunaroute_core::Result<
        Box<dyn Stream<Item = lunaroute_core::Result<NormalizedStreamEvent>> + Send + Unpin>,
    > {
        debug!("Sending streaming request to Anthropic");

        let mut anthropic_req = to_anthropic_request(request)?;
        anthropic_req.stream = Some(true);

        // Log request headers at debug level
        debug!("┌─────────────────────────────────────────────────────────");
        debug!("│ Anthropic Streaming Request Headers");
        debug!("├─────────────────────────────────────────────────────────");
        debug!("│ x-api-key: <api_key>");
        debug!("│ anthropic-version: {}", self.config.api_version);
        debug!("│ Content-Type: application/json");
        debug!("└─────────────────────────────────────────────────────────");

        let response = self
            .client
            .post(format!("{}/v1/messages", self.config.base_url))
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", &self.config.api_version)
            .header("Content-Type", "application/json")
            .json(&anthropic_req)
            .send()
            .await
            .map_err(EgressError::from)?;

        // Log response headers at debug level
        debug!("┌─────────────────────────────────────────────────────────");
        debug!("│ Anthropic Streaming Response Headers");
        debug!("├─────────────────────────────────────────────────────────");
        debug!("│ Status: {}", response.status());
        for (name, value) in response.headers() {
            if let Ok(val_str) = value.to_str() {
                debug!("│ {}: {}", name, val_str);
            }
        }
        debug!("└─────────────────────────────────────────────────────────");

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());
            return Err(EgressError::ProviderError {
                status_code: status,
                message: body,
            }
            .into());
        }

        let stream = create_anthropic_stream(response);
        Ok(Box::new(stream))
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_vision: true,
        }
    }
}

// Anthropic API types

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AnthropicRequest {
    model: String,
    messages: Vec<AnthropicMessage>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AnthropicMessage {
    role: String,
    content: AnthropicContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum AnthropicContent {
    Text(String),
    Blocks(Vec<AnthropicContentBlock>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContentBlock {
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
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AnthropicTool {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AnthropicResponse {
    id: String,
    #[serde(rename = "type")]
    type_: String,
    role: String,
    content: Vec<AnthropicContentBlock>,
    model: String,
    stop_reason: Option<String>,
    usage: AnthropicUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}

// Conversion functions

fn to_anthropic_request(req: NormalizedRequest) -> Result<AnthropicRequest> {
    // Extract system message if present
    let system = req
        .messages
        .iter()
        .find(|m| m.role == Role::System)
        .and_then(|m| match &m.content {
            MessageContent::Text(text) => Some(text.clone()),
            MessageContent::Parts(parts) => {
                // Concatenate all text parts
                let texts: Vec<String> = parts
                    .iter()
                    .filter_map(|p| match p {
                        ContentPart::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .collect();
                if texts.is_empty() {
                    None
                } else {
                    Some(texts.join("\n"))
                }
            }
        });

    // Convert non-system messages
    let messages: Vec<AnthropicMessage> = req
        .messages
        .iter()
        .filter(|m| m.role != Role::System)
        .map(|m| {
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "user", // Tool results go in user messages
                Role::System => "user", // Should not reach here due to filter
            }
            .to_string();

            let content = match &m.content {
                MessageContent::Text(text) => AnthropicContent::Text(text.clone()),
                MessageContent::Parts(parts) => {
                    let mut blocks = Vec::new();

                    for part in parts {
                        match part {
                            ContentPart::Text { text } => {
                                blocks.push(AnthropicContentBlock::Text { text: text.clone() });
                            }
                            ContentPart::Image { .. } => {
                                // Images not yet supported in egress
                                debug!("Skipping image content in Anthropic request");
                            }
                        }
                    }

                    // Add tool calls if present
                    for tool_call in &m.tool_calls {
                        let input: serde_json::Value =
                            serde_json::from_str(&tool_call.function.arguments)
                                .unwrap_or(serde_json::Value::Null);

                        blocks.push(AnthropicContentBlock::ToolUse {
                            id: tool_call.id.clone(),
                            name: tool_call.function.name.clone(),
                            input,
                        });
                    }

                    // Add tool result if present
                    if let Some(tool_call_id) = &m.tool_call_id
                        && let MessageContent::Parts(parts) = &m.content
                    {
                        let content_text = parts
                            .iter()
                            .filter_map(|p| match p {
                                ContentPart::Text { text } => Some(text.clone()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n");

                        blocks.push(AnthropicContentBlock::ToolResult {
                            tool_use_id: tool_call_id.clone(),
                            content: content_text,
                            is_error: None,
                        });
                    }

                    AnthropicContent::Blocks(blocks)
                }
            };

            AnthropicMessage { role, content }
        })
        .collect();

    // Convert tools
    let tools = if req.tools.is_empty() {
        None
    } else {
        Some(
            req.tools
                .iter()
                .map(|t| AnthropicTool {
                    name: t.function.name.clone(),
                    description: t.function.description.clone(),
                    input_schema: t.function.parameters.clone(),
                })
                .collect(),
        )
    };

    Ok(AnthropicRequest {
        model: req.model,
        messages,
        max_tokens: req.max_tokens.unwrap_or(4096),
        system,
        temperature: req.temperature,
        top_p: req.top_p,
        top_k: req.top_k,
        stop_sequences: if req.stop_sequences.is_empty() {
            None
        } else {
            Some(req.stop_sequences)
        },
        stream: None,
        tools,
    })
}

fn from_anthropic_response(resp: AnthropicResponse) -> Result<NormalizedResponse> {
    let mut content_text = String::new();
    let mut tool_calls = Vec::new();

    for block in &resp.content {
        match block {
            AnthropicContentBlock::Text { text } => {
                if !content_text.is_empty() {
                    content_text.push('\n');
                }
                content_text.push_str(text);
            }
            AnthropicContentBlock::ToolUse { id, name, input } => {
                tool_calls.push(ToolCall {
                    id: id.clone(),
                    tool_type: "function".to_string(),
                    function: FunctionCall {
                        name: name.clone(),
                        arguments: input.to_string(),
                    },
                });
            }
            AnthropicContentBlock::ToolResult { .. } => {
                // Tool results shouldn't appear in assistant responses
                debug!("Unexpected tool_result in Anthropic response");
            }
        }
    }

    let message = Message {
        role: Role::Assistant,
        content: if content_text.is_empty() && !tool_calls.is_empty() {
            MessageContent::Text(String::new())
        } else {
            MessageContent::Text(content_text)
        },
        name: None,
        tool_calls,
        tool_call_id: None,
    };

    let finish_reason = match resp.stop_reason.as_deref() {
        Some("end_turn") => FinishReason::Stop,
        Some("max_tokens") => FinishReason::Length,
        Some("tool_use") => FinishReason::ToolCalls,
        Some("stop_sequence") => FinishReason::Stop,
        _ => FinishReason::Stop,
    };

    Ok(NormalizedResponse {
        id: resp.id,
        model: resp.model,
        choices: vec![lunaroute_core::normalized::Choice {
            index: 0,
            message,
            finish_reason: Some(finish_reason),
        }],
        usage: Usage {
            prompt_tokens: resp.usage.input_tokens,
            completion_tokens: resp.usage.output_tokens,
            total_tokens: resp.usage.input_tokens + resp.usage.output_tokens,
        },
        created: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_else(|_| std::time::Duration::from_secs(0))
            .as_secs() as i64,
        metadata: std::collections::HashMap::new(),
    })
}

// Streaming support

/// Anthropic SSE stream event types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicStreamEvent {
    MessageStart {
        message: AnthropicStreamMessage,
    },
    ContentBlockStart {
        index: u32,
        content_block: AnthropicStreamContentBlock,
    },
    ContentBlockDelta {
        index: u32,
        delta: AnthropicStreamDelta,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        delta: AnthropicStreamMessageDelta,
        usage: AnthropicStreamUsage,
    },
    MessageStop,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AnthropicStreamMessage {
    id: String,
    #[serde(rename = "type")]
    type_: String,
    role: String,
    model: String,
    usage: AnthropicUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicStreamContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicStreamDelta {
    TextDelta {
        text: String,
    },
    InputJsonDelta {
        partial_json: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AnthropicStreamMessageDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequence: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AnthropicStreamUsage {
    output_tokens: u32,
}

fn create_anthropic_stream(
    response: reqwest::Response,
) -> Pin<Box<dyn Stream<Item = lunaroute_core::Result<NormalizedStreamEvent>> + Send + Unpin>> {
    use futures::StreamExt;

    let byte_stream = response.bytes_stream();
    let event_stream = eventsource_stream::EventStream::new(byte_stream);

    // Track state across events
    let stream = event_stream.scan(
        (None, None, String::new()),
        |(stream_id, tool_call_state, tool_args_buffer): &mut (
            Option<String>,
            Option<(String, String)>,
            String,
        ),
         result| {
            let event = match result {
                Ok(event) => event,
                Err(e) => {
                    return futures::future::ready(Some(Err(
                        lunaroute_core::Error::Provider(format!(
                            "SSE stream error: {}",
                            e
                        )),
                    )));
                }
            };

            // Parse the event data
            let anthropic_event: AnthropicStreamEvent = match serde_json::from_str(&event.data) {
                Ok(evt) => evt,
                Err(e) => {
                    debug!("Failed to parse Anthropic stream event: {}", e);
                    return futures::future::ready(Some(Err(
                        lunaroute_core::Error::Provider(format!(
                            "Failed to parse stream event: {}",
                            e
                        )),
                    )));
                }
            };

            // Convert to normalized event
            let normalized = match anthropic_event {
                AnthropicStreamEvent::MessageStart { message } => {
                    *stream_id = Some(message.id.clone());
                    debug!("Anthropic stream started: id={}", message.id);
                    Ok(NormalizedStreamEvent::Start {
                        id: message.id,
                        model: message.model,
                    })
                }

                AnthropicStreamEvent::ContentBlockStart { index, content_block } => {
                    match content_block {
                        AnthropicStreamContentBlock::Text { .. } => {
                            // Text block start - just track state, don't emit event
                            debug!("Text content block started at index {}", index);
                            return futures::future::ready(None);
                        }
                        AnthropicStreamContentBlock::ToolUse { id, name } => {
                            // Start of tool call
                            debug!("Tool call started: id={}, name={}", id, name);
                            *tool_call_state = Some((id.clone(), name.clone()));
                            tool_args_buffer.clear();
                            return futures::future::ready(None);
                        }
                    }
                }

                AnthropicStreamEvent::ContentBlockDelta { index, delta } => {
                    match delta {
                        AnthropicStreamDelta::TextDelta { text } => {
                            Ok(NormalizedStreamEvent::Delta {
                                index,
                                delta: Delta {
                                    role: None,
                                    content: Some(text),
                                },
                            })
                        }
                        AnthropicStreamDelta::InputJsonDelta { partial_json } => {
                            // Accumulate tool call arguments
                            tool_args_buffer.push_str(&partial_json);

                            if let Some((id, name)) = tool_call_state.as_ref() {
                                Ok(NormalizedStreamEvent::ToolCallDelta {
                                    index,
                                    tool_call_index: 0,
                                    id: Some(id.clone()),
                                    function: Some(FunctionCallDelta {
                                        name: Some(name.clone()),
                                        arguments: Some(partial_json),
                                    }),
                                })
                            } else {
                                debug!("InputJsonDelta without active tool call");
                                return futures::future::ready(None);
                            }
                        }
                    }
                }

                AnthropicStreamEvent::ContentBlockStop { .. } => {
                    // Reset tool call state
                    *tool_call_state = None;
                    tool_args_buffer.clear();
                    return futures::future::ready(None);
                }

                AnthropicStreamEvent::MessageDelta { delta, usage } => {
                    // Collect events to emit (may have both usage and end)
                    let mut events_to_emit = Vec::new();

                    // Send usage event first if we have output tokens
                    if usage.output_tokens > 0 {
                        events_to_emit.push(Ok(NormalizedStreamEvent::Usage {
                            usage: Usage {
                                prompt_tokens: 0, // Not available in delta
                                completion_tokens: usage.output_tokens,
                                total_tokens: usage.output_tokens,
                            },
                        }));
                    }

                    if let Some(stop_reason) = delta.stop_reason {
                        let reason = match stop_reason.as_str() {
                            "end_turn" => FinishReason::Stop,
                            "max_tokens" => FinishReason::Length,
                            "tool_use" => FinishReason::ToolCalls,
                            "stop_sequence" => FinishReason::Stop,
                            _ => FinishReason::Stop,
                        };
                        events_to_emit.push(Ok(NormalizedStreamEvent::End { finish_reason: reason }));
                    }

                    if events_to_emit.is_empty() {
                        return futures::future::ready(None);
                    }

                    // For now, return the first event (scan returns Option<T>, not Option<Vec<T>>)
                    // This is a limitation - ideally we'd use flat_map instead
                    return futures::future::ready(Some(events_to_emit.into_iter().next().unwrap()));
                }

                AnthropicStreamEvent::MessageStop => {
                    // Final event - already sent End in MessageDelta
                    debug!("Anthropic stream stopped");
                    return futures::future::ready(None);
                }

                AnthropicStreamEvent::Unknown => {
                    debug!("Unknown Anthropic stream event type");
                    return futures::future::ready(None);
                }
            };

            futures::future::ready(Some(normalized))
        },
    );

    Box::pin(stream)
}

// Response handling trait extension

trait AnthropicResponseExt {
    async fn handle_anthropic_response(self) -> Result<AnthropicResponse>;
}

impl AnthropicResponseExt for reqwest::Response {
    async fn handle_anthropic_response(self) -> Result<AnthropicResponse> {
        let status = self.status();

        if !status.is_success() {
            let status_code = status.as_u16();
            let body = self
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());

            return Err(EgressError::ProviderError {
                status_code,
                message: body,
            });
        }

        let response = self.json::<AnthropicResponse>().await.map_err(|e| {
            EgressError::ParseError(format!("Failed to parse Anthropic response: {}", e))
        })?;

        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunaroute_core::normalized::{ContentPart, FunctionDefinition, ImageSource, Tool};

    #[test]
    fn test_config_creation() {
        let config = AnthropicConfig::new("test-key");
        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.base_url, "https://api.anthropic.com");
        assert_eq!(config.api_version, "2023-06-01");
    }

    #[test]
    fn test_config_with_base_url() {
        let config = AnthropicConfig::new("test-key")
            .with_base_url("https://custom.api.com")
            .with_api_version("2024-01-01");

        assert_eq!(config.base_url, "https://custom.api.com");
        assert_eq!(config.api_version, "2024-01-01");
    }

    #[test]
    fn test_connector_creation() {
        let config = AnthropicConfig::new("test-key");
        let connector = AnthropicConnector::new(config);
        assert!(connector.is_ok());
    }

    #[test]
    fn test_capabilities() {
        let config = AnthropicConfig::new("test-key");
        let connector = AnthropicConnector::new(config).unwrap();
        let caps = connector.capabilities();
        assert!(caps.supports_streaming);
        assert!(caps.supports_tools);
        assert!(caps.supports_vision);
    }

    #[test]
    fn test_to_anthropic_request_basic() {
        let normalized = NormalizedRequest {
            model: "claude-3-opus".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text("Hello".to_string()),
                name: None,
                tool_calls: vec![],
                tool_call_id: None,
            }],
            system: None,
            temperature: Some(0.7),
            max_tokens: Some(100),
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec![],
            tools: vec![],
            tool_choice: None,
            metadata: std::collections::HashMap::new(),
        };

        let anthropic_req = to_anthropic_request(normalized).unwrap();
        assert_eq!(anthropic_req.model, "claude-3-opus");
        assert_eq!(anthropic_req.messages.len(), 1);
        assert_eq!(anthropic_req.messages[0].role, "user");
        assert_eq!(anthropic_req.max_tokens, 100);
        assert_eq!(anthropic_req.temperature, Some(0.7));
    }

    #[test]
    fn test_to_anthropic_request_with_system() {
        let normalized = NormalizedRequest {
            model: "claude-3-opus".to_string(),
            messages: vec![
                Message {
                    role: Role::System,
                    content: MessageContent::Text("You are a helpful assistant".to_string()),
                    name: None,
                    tool_calls: vec![],
                    tool_call_id: None,
                },
                Message {
                    role: Role::User,
                    content: MessageContent::Text("Hello".to_string()),
                    name: None,
                    tool_calls: vec![],
                    tool_call_id: None,
                },
            ],
            system: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec![],
            tools: vec![],
            tool_choice: None,
            metadata: std::collections::HashMap::new(),
        };

        let anthropic_req = to_anthropic_request(normalized).unwrap();
        // System message should be extracted
        assert_eq!(
            anthropic_req.system,
            Some("You are a helpful assistant".to_string())
        );
        // Only non-system messages should remain
        assert_eq!(anthropic_req.messages.len(), 1);
        assert_eq!(anthropic_req.messages[0].role, "user");
    }

    #[test]
    fn test_to_anthropic_request_with_tools() {
        let normalized = NormalizedRequest {
            model: "claude-3-opus".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text("What's the weather?".to_string()),
                name: None,
                tool_calls: vec![],
                tool_call_id: None,
            }],
            system: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec![],
            tools: vec![Tool {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: "get_weather".to_string(),
                    description: Some("Get weather info".to_string()),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "location": {"type": "string"}
                        }
                    }),
                },
            }],
            tool_choice: None,
            metadata: std::collections::HashMap::new(),
        };

        let anthropic_req = to_anthropic_request(normalized).unwrap();
        assert!(anthropic_req.tools.is_some());
        let tools = anthropic_req.tools.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "get_weather");
        assert_eq!(tools[0].description, Some("Get weather info".to_string()));
    }

    #[test]
    fn test_to_anthropic_request_with_tool_calls() {
        let normalized = NormalizedRequest {
            model: "claude-3-opus".to_string(),
            messages: vec![Message {
                role: Role::Assistant,
                content: MessageContent::Parts(vec![ContentPart::Text {
                    text: "Let me check the weather".to_string(),
                }]),
                name: None,
                tool_calls: vec![ToolCall {
                    id: "call_123".to_string(),
                    tool_type: "function".to_string(),
                    function: FunctionCall {
                        name: "get_weather".to_string(),
                        arguments: r#"{"location":"San Francisco"}"#.to_string(),
                    },
                }],
                tool_call_id: None,
            }],
            system: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec![],
            tools: vec![],
            tool_choice: None,
            metadata: std::collections::HashMap::new(),
        };

        let anthropic_req = to_anthropic_request(normalized).unwrap();
        assert_eq!(anthropic_req.messages.len(), 1);

        match &anthropic_req.messages[0].content {
            AnthropicContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 2); // Text + ToolUse

                // Check text block
                match &blocks[0] {
                    AnthropicContentBlock::Text { text } => {
                        assert_eq!(text, "Let me check the weather");
                    }
                    _ => panic!("Expected Text block"),
                }

                // Check tool use block
                match &blocks[1] {
                    AnthropicContentBlock::ToolUse { id, name, input } => {
                        assert_eq!(id, "call_123");
                        assert_eq!(name, "get_weather");
                        assert_eq!(input["location"], "San Francisco");
                    }
                    _ => panic!("Expected ToolUse block"),
                }
            }
            _ => panic!("Expected Blocks content"),
        }
    }

    #[test]
    fn test_to_anthropic_request_with_tool_result() {
        let normalized = NormalizedRequest {
            model: "claude-3-opus".to_string(),
            messages: vec![Message {
                role: Role::Tool,
                content: MessageContent::Parts(vec![ContentPart::Text {
                    text: r#"{"temperature": 72, "conditions": "sunny"}"#.to_string(),
                }]),
                name: None,
                tool_calls: vec![],
                tool_call_id: Some("call_123".to_string()),
            }],
            system: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec![],
            tools: vec![],
            tool_choice: None,
            metadata: std::collections::HashMap::new(),
        };

        let anthropic_req = to_anthropic_request(normalized).unwrap();
        assert_eq!(anthropic_req.messages.len(), 1);
        assert_eq!(anthropic_req.messages[0].role, "user"); // Tool results go in user messages

        match &anthropic_req.messages[0].content {
            AnthropicContent::Blocks(blocks) => {
                // Both text block and tool result block are added
                assert_eq!(blocks.len(), 2);

                // First block is the text content
                match &blocks[0] {
                    AnthropicContentBlock::Text { text } => {
                        assert_eq!(text, r#"{"temperature": 72, "conditions": "sunny"}"#);
                    }
                    _ => panic!("Expected Text block"),
                }

                // Second block is the tool result
                match &blocks[1] {
                    AnthropicContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } => {
                        assert_eq!(tool_use_id, "call_123");
                        assert_eq!(content, r#"{"temperature": 72, "conditions": "sunny"}"#);
                    }
                    _ => panic!("Expected ToolResult block"),
                }
            }
            _ => panic!("Expected Blocks content"),
        }
    }

    #[test]
    fn test_from_anthropic_response_basic() {
        let anthropic_resp = AnthropicResponse {
            id: "msg_123".to_string(),
            type_: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![AnthropicContentBlock::Text {
                text: "Hello! How can I help you?".to_string(),
            }],
            model: "claude-3-opus".to_string(),
            stop_reason: Some("end_turn".to_string()),
            usage: AnthropicUsage {
                input_tokens: 10,
                output_tokens: 20,
            },
        };

        let normalized = from_anthropic_response(anthropic_resp).unwrap();
        assert_eq!(normalized.id, "msg_123");
        assert_eq!(normalized.model, "claude-3-opus");
        assert_eq!(normalized.choices.len(), 1);
        assert_eq!(normalized.choices[0].message.role, Role::Assistant);

        match &normalized.choices[0].message.content {
            MessageContent::Text(text) => {
                assert_eq!(text, "Hello! How can I help you?");
            }
            _ => panic!("Expected Text content"),
        }

        assert_eq!(normalized.choices[0].finish_reason, Some(FinishReason::Stop));
        assert_eq!(normalized.usage.prompt_tokens, 10);
        assert_eq!(normalized.usage.completion_tokens, 20);
        assert_eq!(normalized.usage.total_tokens, 30);
    }

    #[test]
    fn test_from_anthropic_response_with_tool_calls() {
        let anthropic_resp = AnthropicResponse {
            id: "msg_123".to_string(),
            type_: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![
                AnthropicContentBlock::Text {
                    text: "Let me check that for you".to_string(),
                },
                AnthropicContentBlock::ToolUse {
                    id: "call_456".to_string(),
                    name: "search_web".to_string(),
                    input: serde_json::json!({"query": "rust programming"}),
                },
            ],
            model: "claude-3-opus".to_string(),
            stop_reason: Some("tool_use".to_string()),
            usage: AnthropicUsage {
                input_tokens: 15,
                output_tokens: 25,
            },
        };

        let normalized = from_anthropic_response(anthropic_resp).unwrap();
        assert_eq!(normalized.choices[0].message.tool_calls.len(), 1);

        let tool_call = &normalized.choices[0].message.tool_calls[0];
        assert_eq!(tool_call.id, "call_456");
        assert_eq!(tool_call.tool_type, "function");
        assert_eq!(tool_call.function.name, "search_web");

        let args: serde_json::Value = serde_json::from_str(&tool_call.function.arguments).unwrap();
        assert_eq!(args["query"], "rust programming");

        assert_eq!(
            normalized.choices[0].finish_reason,
            Some(FinishReason::ToolCalls)
        );
    }

    #[test]
    fn test_from_anthropic_response_finish_reasons() {
        // Test end_turn
        let resp = AnthropicResponse {
            id: "msg_1".to_string(),
            type_: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![],
            model: "claude-3-opus".to_string(),
            stop_reason: Some("end_turn".to_string()),
            usage: AnthropicUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
        };
        let normalized = from_anthropic_response(resp).unwrap();
        assert_eq!(normalized.choices[0].finish_reason, Some(FinishReason::Stop));

        // Test max_tokens
        let resp = AnthropicResponse {
            id: "msg_2".to_string(),
            type_: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![],
            model: "claude-3-opus".to_string(),
            stop_reason: Some("max_tokens".to_string()),
            usage: AnthropicUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
        };
        let normalized = from_anthropic_response(resp).unwrap();
        assert_eq!(normalized.choices[0].finish_reason, Some(FinishReason::Length));

        // Test tool_use
        let resp = AnthropicResponse {
            id: "msg_3".to_string(),
            type_: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![],
            model: "claude-3-opus".to_string(),
            stop_reason: Some("tool_use".to_string()),
            usage: AnthropicUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
        };
        let normalized = from_anthropic_response(resp).unwrap();
        assert_eq!(
            normalized.choices[0].finish_reason,
            Some(FinishReason::ToolCalls)
        );

        // Test stop_sequence
        let resp = AnthropicResponse {
            id: "msg_4".to_string(),
            type_: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![],
            model: "claude-3-opus".to_string(),
            stop_reason: Some("stop_sequence".to_string()),
            usage: AnthropicUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
        };
        let normalized = from_anthropic_response(resp).unwrap();
        assert_eq!(normalized.choices[0].finish_reason, Some(FinishReason::Stop));
    }

    #[test]
    fn test_to_anthropic_request_multimodal_content() {
        let normalized = NormalizedRequest {
            model: "claude-3-opus".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Parts(vec![
                    ContentPart::Text {
                        text: "What is in this image?".to_string(),
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
            }],
            system: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec![],
            tools: vec![],
            tool_choice: None,
            metadata: std::collections::HashMap::new(),
        };

        let anthropic_req = to_anthropic_request(normalized).unwrap();
        assert_eq!(anthropic_req.messages.len(), 1);

        match &anthropic_req.messages[0].content {
            AnthropicContent::Blocks(blocks) => {
                // Images are currently skipped in egress, so only text block should remain
                assert_eq!(blocks.len(), 1);
                match &blocks[0] {
                    AnthropicContentBlock::Text { text } => {
                        assert_eq!(text, "What is in this image?");
                    }
                    _ => panic!("Expected Text block"),
                }
            }
            _ => panic!("Expected Blocks content"),
        }
    }

    #[test]
    fn test_to_anthropic_request_with_stop_sequences() {
        let normalized = NormalizedRequest {
            model: "claude-3-opus".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text("Hello".to_string()),
                name: None,
                tool_calls: vec![],
                tool_call_id: None,
            }],
            system: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec!["STOP".to_string(), "END".to_string()],
            tools: vec![],
            tool_choice: None,
            metadata: std::collections::HashMap::new(),
        };

        let anthropic_req = to_anthropic_request(normalized).unwrap();
        assert!(anthropic_req.stop_sequences.is_some());
        let stops = anthropic_req.stop_sequences.unwrap();
        assert_eq!(stops.len(), 2);
        assert!(stops.contains(&"STOP".to_string()));
        assert!(stops.contains(&"END".to_string()));
    }

    #[test]
    fn test_from_anthropic_response_multiple_text_blocks() {
        let anthropic_resp = AnthropicResponse {
            id: "msg_123".to_string(),
            type_: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![
                AnthropicContentBlock::Text {
                    text: "First paragraph.".to_string(),
                },
                AnthropicContentBlock::Text {
                    text: "Second paragraph.".to_string(),
                },
            ],
            model: "claude-3-opus".to_string(),
            stop_reason: Some("end_turn".to_string()),
            usage: AnthropicUsage {
                input_tokens: 5,
                output_tokens: 10,
            },
        };

        let normalized = from_anthropic_response(anthropic_resp).unwrap();

        match &normalized.choices[0].message.content {
            MessageContent::Text(text) => {
                // Multiple text blocks should be concatenated with newlines
                assert_eq!(text, "First paragraph.\nSecond paragraph.");
            }
            _ => panic!("Expected Text content"),
        }
    }

    #[test]
    fn test_from_anthropic_response_empty_content() {
        let anthropic_resp = AnthropicResponse {
            id: "msg_123".to_string(),
            type_: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![],
            model: "claude-3-opus".to_string(),
            stop_reason: Some("end_turn".to_string()),
            usage: AnthropicUsage {
                input_tokens: 5,
                output_tokens: 0,
            },
        };

        let normalized = from_anthropic_response(anthropic_resp).unwrap();

        match &normalized.choices[0].message.content {
            MessageContent::Text(text) => {
                assert_eq!(text, "");
            }
            _ => panic!("Expected Text content"),
        }
    }

    #[test]
    fn test_to_anthropic_request_empty_tools() {
        let normalized = NormalizedRequest {
            model: "claude-3-opus".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text("Hello".to_string()),
                name: None,
                tool_calls: vec![],
                tool_call_id: None,
            }],
            system: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec![],
            tools: vec![],
            tool_choice: None,
            metadata: std::collections::HashMap::new(),
        };

        let anthropic_req = to_anthropic_request(normalized).unwrap();
        assert!(anthropic_req.tools.is_none());
    }

    #[test]
    fn test_to_anthropic_request_multiple_tool_calls() {
        let normalized = NormalizedRequest {
            model: "claude-3-opus".to_string(),
            messages: vec![Message {
                role: Role::Assistant,
                content: MessageContent::Parts(vec![ContentPart::Text {
                    text: "I'll use multiple tools".to_string(),
                }]),
                name: None,
                tool_calls: vec![
                    ToolCall {
                        id: "call_1".to_string(),
                        tool_type: "function".to_string(),
                        function: FunctionCall {
                            name: "get_weather".to_string(),
                            arguments: r#"{"location":"NYC"}"#.to_string(),
                        },
                    },
                    ToolCall {
                        id: "call_2".to_string(),
                        tool_type: "function".to_string(),
                        function: FunctionCall {
                            name: "get_time".to_string(),
                            arguments: r#"{"timezone":"EST"}"#.to_string(),
                        },
                    },
                ],
                tool_call_id: None,
            }],
            system: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec![],
            tools: vec![],
            tool_choice: None,
            metadata: std::collections::HashMap::new(),
        };

        let anthropic_req = to_anthropic_request(normalized).unwrap();
        assert_eq!(anthropic_req.messages.len(), 1);

        match &anthropic_req.messages[0].content {
            AnthropicContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 3); // Text + ToolUse + ToolUse

                // Check text block
                match &blocks[0] {
                    AnthropicContentBlock::Text { text } => {
                        assert_eq!(text, "I'll use multiple tools");
                    }
                    _ => panic!("Expected Text block"),
                }

                // Check first tool use
                match &blocks[1] {
                    AnthropicContentBlock::ToolUse { id, name, input } => {
                        assert_eq!(id, "call_1");
                        assert_eq!(name, "get_weather");
                        assert_eq!(input["location"], "NYC");
                    }
                    _ => panic!("Expected ToolUse block"),
                }

                // Check second tool use
                match &blocks[2] {
                    AnthropicContentBlock::ToolUse { id, name, input } => {
                        assert_eq!(id, "call_2");
                        assert_eq!(name, "get_time");
                        assert_eq!(input["timezone"], "EST");
                    }
                    _ => panic!("Expected ToolUse block"),
                }
            }
            _ => panic!("Expected Blocks content"),
        }
    }

    // Streaming tests
    mod streaming_tests {
        use super::*;

        // Helper to parse SSE events and convert to normalized events
        // This tests the core conversion logic without needing HTTP mocking
        async fn parse_sse_events(events: Vec<&str>) -> Vec<lunaroute_core::Result<NormalizedStreamEvent>> {
            let mut results = Vec::new();
            let mut state: (Option<String>, Option<(String, String)>, String) = (None, None, String::new());

            for event_data in events {
                let anthropic_event: AnthropicStreamEvent = match serde_json::from_str(event_data) {
                    Ok(evt) => evt,
                    Err(e) => {
                        results.push(Err(lunaroute_core::Error::Provider(format!(
                            "Failed to parse stream event: {}",
                            e
                        ))));
                        continue;
                    }
                };

                let (stream_id, tool_call_state, tool_args_buffer) = &mut state;

                // This is the same logic as in create_anthropic_stream
                match anthropic_event {
                    AnthropicStreamEvent::MessageStart { message } => {
                        *stream_id = Some(message.id.clone());
                        debug!("Anthropic stream started: id={}", message.id);
                        results.push(Ok(NormalizedStreamEvent::Start {
                            id: message.id,
                            model: message.model,
                        }));
                    }

                    AnthropicStreamEvent::ContentBlockStart { index, content_block } => {
                        match content_block {
                            AnthropicStreamContentBlock::Text { .. } => {
                                debug!("Text content block started at index {}", index);
                            }
                            AnthropicStreamContentBlock::ToolUse { id, name } => {
                                debug!("Tool call started: id={}, name={}", id, name);
                                *tool_call_state = Some((id.clone(), name.clone()));
                                tool_args_buffer.clear();
                            }
                        }
                    }

                    AnthropicStreamEvent::ContentBlockDelta { index, delta } => {
                        match delta {
                            AnthropicStreamDelta::TextDelta { text } => {
                                results.push(Ok(NormalizedStreamEvent::Delta {
                                    index,
                                    delta: Delta {
                                        role: None,
                                        content: Some(text),
                                    },
                                }));
                            }
                            AnthropicStreamDelta::InputJsonDelta { partial_json } => {
                                tool_args_buffer.push_str(&partial_json);

                                if let Some((id, name)) = tool_call_state.as_ref() {
                                    results.push(Ok(NormalizedStreamEvent::ToolCallDelta {
                                        index,
                                        tool_call_index: 0,
                                        id: Some(id.clone()),
                                        function: Some(FunctionCallDelta {
                                            name: Some(name.clone()),
                                            arguments: Some(partial_json),
                                        }),
                                    }));
                                }
                            }
                        }
                    }

                    AnthropicStreamEvent::ContentBlockStop { .. } => {
                        *tool_call_state = None;
                        tool_args_buffer.clear();
                    }

                    AnthropicStreamEvent::MessageDelta { delta, usage } => {
                        if usage.output_tokens > 0 {
                            results.push(Ok(NormalizedStreamEvent::Usage {
                                usage: Usage {
                                    prompt_tokens: 0,
                                    completion_tokens: usage.output_tokens,
                                    total_tokens: usage.output_tokens,
                                },
                            }));
                        }
                        if let Some(stop_reason) = delta.stop_reason {
                            let reason = match stop_reason.as_str() {
                                "end_turn" => FinishReason::Stop,
                                "max_tokens" => FinishReason::Length,
                                "tool_use" => FinishReason::ToolCalls,
                                "stop_sequence" => FinishReason::Stop,
                                _ => FinishReason::Stop,
                            };
                            results.push(Ok(NormalizedStreamEvent::End { finish_reason: reason }));
                        }
                    }

                    AnthropicStreamEvent::MessageStop => {
                        debug!("Anthropic stream stopped");
                    }

                    AnthropicStreamEvent::Unknown => {
                        debug!("Unknown Anthropic stream event type");
                    }
                }
            }

            results
        }

        #[tokio::test]
        async fn test_stream_text_content() {
            let events = vec![
                r#"{"type":"message_start","message":{"id":"msg_123","type":"message","role":"assistant","model":"claude-3-opus","usage":{"input_tokens":10,"output_tokens":0}}}"#,
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" world"}}"#,
                r#"{"type":"content_block_stop","index":0}"#,
                r#"{"type":"message_delta","delta":{},"usage":{"output_tokens":5}}"#,
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":0}}"#,
                r#"{"type":"message_stop"}"#,
            ];

            let results = parse_sse_events(events).await;
            let collected: Vec<_> = results.into_iter().map(|r| r.unwrap()).collect();

            // Verify events
            assert_eq!(collected.len(), 5); // Start + 2 Deltas + Usage + End

            // Start event
            match &collected[0] {
                NormalizedStreamEvent::Start { id, model } => {
                    assert_eq!(id, "msg_123");
                    assert_eq!(model, "claude-3-opus");
                }
                _ => panic!("Expected Start event"),
            }

            // Delta events
            match &collected[1] {
                NormalizedStreamEvent::Delta { index, delta } => {
                    assert_eq!(*index, 0);
                    assert_eq!(delta.content.as_ref().unwrap(), "Hello");
                }
                _ => panic!("Expected Delta event"),
            }

            match &collected[2] {
                NormalizedStreamEvent::Delta { index, delta } => {
                    assert_eq!(*index, 0);
                    assert_eq!(delta.content.as_ref().unwrap(), " world");
                }
                _ => panic!("Expected Delta event"),
            }

            // Usage event
            match &collected[3] {
                NormalizedStreamEvent::Usage { usage } => {
                    assert_eq!(usage.completion_tokens, 5);
                }
                _ => panic!("Expected Usage event"),
            }

            // End event
            match &collected[4] {
                NormalizedStreamEvent::End { finish_reason } => {
                    assert_eq!(*finish_reason, FinishReason::Stop);
                }
                _ => panic!("Expected End event"),
            }
        }

        #[tokio::test]
        async fn test_stream_tool_calls() {
            let events = vec![
                r#"{"type":"message_start","message":{"id":"msg_456","type":"message","role":"assistant","model":"claude-3-opus","usage":{"input_tokens":15,"output_tokens":0}}}"#,
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_123","name":"get_weather"}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"location\":"}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"NYC\"}"}}"#,
                r#"{"type":"content_block_stop","index":0}"#,
                r#"{"type":"message_delta","delta":{},"usage":{"output_tokens":10}}"#,
                r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":0}}"#,
                r#"{"type":"message_stop"}"#,
            ];

            let results = parse_sse_events(events).await;
            let collected: Vec<_> = results.into_iter().map(|r| r.unwrap()).collect();

            // Verify we got Start + 2 ToolCallDeltas + Usage + End
            assert_eq!(collected.len(), 5);

            // Start
            match &collected[0] {
                NormalizedStreamEvent::Start { id, .. } => {
                    assert_eq!(id, "msg_456");
                }
                _ => panic!("Expected Start event"),
            }

            // Tool call deltas
            match &collected[1] {
                NormalizedStreamEvent::ToolCallDelta {
                    id,
                    function,
                    ..
                } => {
                    assert_eq!(id.as_ref().unwrap(), "call_123");
                    let func = function.as_ref().unwrap();
                    assert_eq!(func.name.as_ref().unwrap(), "get_weather");
                    assert_eq!(func.arguments.as_ref().unwrap(), "{\"location\":");
                }
                _ => panic!("Expected ToolCallDelta event"),
            }

            match &collected[2] {
                NormalizedStreamEvent::ToolCallDelta {
                    function,
                    ..
                } => {
                    let func = function.as_ref().unwrap();
                    assert_eq!(func.arguments.as_ref().unwrap(), "\"NYC\"}");
                }
                _ => panic!("Expected ToolCallDelta event"),
            }

            // Usage event
            match &collected[3] {
                NormalizedStreamEvent::Usage { usage } => {
                    assert_eq!(usage.completion_tokens, 10);
                }
                _ => panic!("Expected Usage event"),
            }

            // End with tool_use finish reason
            match &collected[4] {
                NormalizedStreamEvent::End { finish_reason } => {
                    assert_eq!(*finish_reason, FinishReason::ToolCalls);
                }
                _ => panic!("Expected End event"),
            }
        }

        #[tokio::test]
        async fn test_stream_finish_reasons() {
            // Test max_tokens finish reason
            let events = vec![
                r#"{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"claude-3-opus","usage":{"input_tokens":5,"output_tokens":0}}}"#,
                r#"{"type":"message_delta","delta":{},"usage":{"output_tokens":100}}"#,
                r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":0}}"#,
                r#"{"type":"message_stop"}"#,
            ];

            let results = parse_sse_events(events).await;
            let collected: Vec<_> = results.into_iter().map(|r| r.unwrap()).collect();

            // Should get Start + Usage + End
            assert_eq!(collected.len(), 3);

            // Verify Usage event
            match &collected[1] {
                NormalizedStreamEvent::Usage { usage } => {
                    assert_eq!(usage.completion_tokens, 100);
                }
                _ => panic!("Expected Usage event"),
            }

            // Verify End event with finish reason
            match &collected[2] {
                NormalizedStreamEvent::End { finish_reason } => {
                    assert_eq!(*finish_reason, FinishReason::Length);
                }
                _ => panic!("Expected End event with Length reason"),
            }
        }

        #[tokio::test]
        async fn test_stream_error_handling() {
            // Test with invalid JSON
            let events = vec![
                r#"{"invalid json"#,
            ];

            let results = parse_sse_events(events).await;

            // Should get an error
            assert_eq!(results.len(), 1);
            assert!(results[0].is_err());
        }

        #[tokio::test]
        async fn test_stream_multiple_content_blocks() {
            // Test text block followed by tool use
            let events = vec![
                r#"{"type":"message_start","message":{"id":"msg_789","type":"message","role":"assistant","model":"claude-3-opus","usage":{"input_tokens":20,"output_tokens":0}}}"#,
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Let me check"}}"#,
                r#"{"type":"content_block_stop","index":0}"#,
                r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_456","name":"search"}}"#,
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"query\":\"test\"}"}}"#,
                r#"{"type":"content_block_stop","index":1}"#,
                r#"{"type":"message_delta","delta":{},"usage":{"output_tokens":15}}"#,
                r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":0}}"#,
                r#"{"type":"message_stop"}"#,
            ];

            let results = parse_sse_events(events).await;
            let collected: Vec<_> = results.into_iter().map(|r| r.unwrap()).collect();

            // Start + text Delta + tool Delta + Usage + End
            assert_eq!(collected.len(), 5);

            // Verify we got both text and tool call
            let has_text_delta = collected.iter().any(|e| matches!(
                e,
                NormalizedStreamEvent::Delta { delta, .. } if delta.content.is_some()
            ));
            let has_tool_delta = collected.iter().any(|e| matches!(
                e,
                NormalizedStreamEvent::ToolCallDelta { .. }
            ));

            assert!(has_text_delta);
            assert!(has_tool_delta);
        }
    }
}
