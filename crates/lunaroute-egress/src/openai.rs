//! OpenAI egress connector

use crate::{client::{create_client, with_retry, HttpClientConfig}, EgressError, Result};
use async_trait::async_trait;
use futures::Stream;
use lunaroute_core::{
    normalized::{
        ContentPart, Delta, FinishReason, FunctionCall, FunctionCallDelta,
        Message, MessageContent, NormalizedRequest, NormalizedResponse,
        NormalizedStreamEvent, Role, ToolCall, ToolChoice, Usage,
    },
    provider::{Provider, ProviderCapabilities},
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use tracing::{debug, instrument};

/// OpenAI connector configuration
#[derive(Debug, Clone)]
pub struct OpenAIConfig {
    /// API key for authentication
    pub api_key: String,

    /// Base URL for OpenAI API (default: https://api.openai.com/v1)
    pub base_url: String,

    /// Organization ID (optional)
    pub organization: Option<String>,

    /// HTTP client configuration
    pub client_config: HttpClientConfig,
}

impl OpenAIConfig {
    /// Create a new OpenAI configuration
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: "https://api.openai.com/v1".to_string(),
            organization: None,
            client_config: HttpClientConfig::default(),
        }
    }

    /// Set the base URL (for custom endpoints)
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Set the organization ID
    pub fn with_organization(mut self, organization: impl Into<String>) -> Self {
        self.organization = Some(organization.into());
        self
    }
}

/// OpenAI connector
pub struct OpenAIConnector {
    config: OpenAIConfig,
    client: Client,
}

impl OpenAIConnector {
    /// Create a new OpenAI connector
    pub fn new(config: OpenAIConfig) -> Result<Self> {
        let client = create_client(&config.client_config)?;
        Ok(Self { config, client })
    }

    /// Send a raw JSON request directly to OpenAI (passthrough mode)
    /// This skips normalization for OpenAI→OpenAI routing, preserving 100% API fidelity.
    /// Still parses the response to extract metrics (tokens, model, etc.)
    #[instrument(skip(self, request_json))]
    pub async fn send_passthrough(
        &self,
        request_json: serde_json::Value,
    ) -> Result<serde_json::Value> {
        debug!("Sending passthrough request to OpenAI (no normalization)");

        let max_retries = self.config.client_config.max_retries;
        let result = with_retry(max_retries, || {
            let request_json = request_json.clone();
            async move {
                let response = self.client
                    .post(format!("{}/chat/completions", self.config.base_url))
                    .header("Authorization", format!("Bearer {}", self.config.api_key))
                    .header("Content-Type", "application/json")
                    .apply_organization_header(&self.config)
                    .json(&request_json)
                    .send()
                    .await?;

                // Log response headers at debug level
                debug!("┌─────────────────────────────────────────────────────────");
                debug!("│ OpenAI Passthrough Response Headers");
                debug!("├─────────────────────────────────────────────────────────");
                debug!("│ Status: {}", response.status());
                for (name, value) in response.headers() {
                    if let Ok(val_str) = value.to_str() {
                        debug!("│ {}: {}", name, val_str);
                    }
                }
                debug!("└─────────────────────────────────────────────────────────");

                self.handle_openai_passthrough_response(response).await
            }
        })
        .await?;

        Ok(result)
    }

    /// Handle passthrough response (for send_passthrough)
    async fn handle_openai_passthrough_response(
        &self,
        response: reqwest::Response,
    ) -> Result<serde_json::Value> {
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

        response
            .json::<serde_json::Value>()
            .await
            .map_err(|e| {
                EgressError::ParseError(format!("Failed to parse OpenAI passthrough response: {}", e))
            })
    }

    /// Stream raw OpenAI request (passthrough mode - no normalization)
    /// Returns raw response for direct SSE forwarding
    #[instrument(skip(self, request_json))]
    pub async fn stream_passthrough(
        &self,
        request_json: serde_json::Value,
    ) -> Result<reqwest::Response> {
        debug!("Sending passthrough streaming request to OpenAI (no normalization)");

        let response = self
            .client
            .post(format!("{}/chat/completions", self.config.base_url))
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .apply_organization_header(&self.config)
            .json(&request_json)
            .send()
            .await
            .map_err(EgressError::from)?;

        // Log response headers at debug level
        debug!("┌─────────────────────────────────────────────────────────");
        debug!("│ OpenAI Streaming Passthrough Response Headers");
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
            let body = response.text().await.unwrap_or_else(|_| "Unable to read error body".to_string());
            return Err(EgressError::ProviderError {
                status_code: status,
                message: body,
            });
        }

        Ok(response)
    }
}

#[async_trait]
impl Provider for OpenAIConnector {
    #[instrument(skip(self, request), fields(model = %request.model))]
    async fn send(&self, request: NormalizedRequest) -> lunaroute_core::Result<NormalizedResponse> {
        debug!("Sending non-streaming request to OpenAI");

        let openai_req = to_openai_request(request)?;

        // Log request headers at debug level
        debug!("┌─────────────────────────────────────────────────────────");
        debug!("│ OpenAI Request Headers");
        debug!("├─────────────────────────────────────────────────────────");
        debug!("│ Authorization: Bearer <api_key>");
        debug!("│ Content-Type: application/json");
        if let Some(ref org) = self.config.organization {
            debug!("│ OpenAI-Organization: {}", org);
        }
        debug!("└─────────────────────────────────────────────────────────");

        let max_retries = self.config.client_config.max_retries;
        let result = with_retry(max_retries, || {
            let openai_req = openai_req.clone();
            async move {
                let response = self.client
                    .post(format!("{}/chat/completions", self.config.base_url))
                    .header("Authorization", format!("Bearer {}", self.config.api_key))
                    .header("Content-Type", "application/json")
                    .apply_organization_header(&self.config)
                    .json(&openai_req)
                    .send()
                    .await?;

                // Log response headers at debug level
                debug!("┌─────────────────────────────────────────────────────────");
                debug!("│ OpenAI Response Headers");
                debug!("├─────────────────────────────────────────────────────────");
                debug!("│ Status: {}", response.status());
                for (name, value) in response.headers() {
                    if let Ok(val_str) = value.to_str() {
                        debug!("│ {}: {}", name, val_str);
                    }
                }
                debug!("└─────────────────────────────────────────────────────────");

                response.handle_openai_response().await
            }
        })
        .await?;

        let normalized = from_openai_response(result)?;
        Ok(normalized)
    }

    async fn stream(
        &self,
        request: NormalizedRequest,
    ) -> lunaroute_core::Result<Box<dyn Stream<Item = lunaroute_core::Result<NormalizedStreamEvent>> + Send + Unpin>> {
        debug!("Sending streaming request to OpenAI");

        let mut openai_req = to_openai_request(request)?;
        openai_req.stream = Some(true);

        // Log request headers at debug level
        debug!("┌─────────────────────────────────────────────────────────");
        debug!("│ OpenAI Streaming Request Headers");
        debug!("├─────────────────────────────────────────────────────────");
        debug!("│ Authorization: Bearer <api_key>");
        debug!("│ Content-Type: application/json");
        if let Some(ref org) = self.config.organization {
            debug!("│ OpenAI-Organization: {}", org);
        }
        debug!("└─────────────────────────────────────────────────────────");

        let response = self
            .client
            .post(format!("{}/chat/completions", self.config.base_url))
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .apply_organization_header(&self.config)
            .json(&openai_req)
            .send()
            .await
            .map_err(EgressError::from)?;

        // Log response headers at debug level
        debug!("┌─────────────────────────────────────────────────────────");
        debug!("│ OpenAI Streaming Response Headers");
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
            let body = response.text().await.unwrap_or_else(|_| "Unable to read error body".to_string());
            return Err(EgressError::ProviderError {
                status_code: status,
                message: body,
            }
            .into());
        }

        let stream = create_openai_stream(response);
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

// OpenAI API types (simplified, matching ingress types)

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIChatRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    /// GPT-5 models use max_completion_tokens instead of max_tokens
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAITool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<OpenAIToolChoice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAITool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OpenAIFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIFunction {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum OpenAIToolChoice {
    String(String),
    Object { r#type: String, function: OpenAIFunctionName },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIFunctionName {
    name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIToolCall {
    id: String,
    #[serde(rename = "type")]
    tool_type: String,
    function: OpenAIFunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIChatResponse {
    id: String,
    model: String,
    choices: Vec<OpenAIChoice>,
    usage: OpenAIUsage,
    created: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIChoice {
    index: u32,
    message: OpenAIMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIStreamChunk {
    id: String,
    choices: Vec<OpenAIStreamChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<OpenAIUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIStreamChoice {
    index: u32,
    delta: OpenAIDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCallDelta>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIToolCallDelta {
    index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function: Option<OpenAIFunctionCallDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIFunctionCallDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    arguments: Option<String>,
}

// Conversion functions

fn to_openai_request(req: NormalizedRequest) -> Result<OpenAIChatRequest> {
    let messages = req
        .messages
        .into_iter()
        .map(|msg| {
            let role = match msg.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            }
            .to_string();

            let content = match msg.content {
                MessageContent::Text(text) => {
                    if text.is_empty() && !msg.tool_calls.is_empty() {
                        None // OpenAI allows null content for tool call messages
                    } else {
                        Some(text)
                    }
                }
                MessageContent::Parts(parts) => {
                    // Extract text from parts
                    let text: String = parts
                        .into_iter()
                        .filter_map(|part| match part {
                            ContentPart::Text { text } => Some(text),
                            ContentPart::Image { .. } => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");

                    if text.is_empty() {
                        None
                    } else {
                        Some(text)
                    }
                }
            };

            let tool_calls = if msg.tool_calls.is_empty() {
                None
            } else {
                Some(
                    msg.tool_calls
                        .into_iter()
                        .map(|tc| OpenAIToolCall {
                            id: tc.id,
                            tool_type: tc.tool_type,
                            function: OpenAIFunctionCall {
                                name: tc.function.name,
                                arguments: tc.function.arguments,
                            },
                        })
                        .collect(),
                )
            };

            OpenAIMessage {
                role,
                content,
                name: msg.name,
                tool_calls,
                tool_call_id: msg.tool_call_id,
            }
        })
        .collect();

    let tools = if req.tools.is_empty() {
        None
    } else {
        Some(
            req.tools
                .into_iter()
                .map(|t| OpenAITool {
                    tool_type: t.tool_type,
                    function: OpenAIFunction {
                        name: t.function.name,
                        description: t.function.description,
                        parameters: t.function.parameters,
                    },
                })
                .collect(),
        )
    };

    let tool_choice = req.tool_choice.map(|tc| match tc {
        ToolChoice::Auto => OpenAIToolChoice::String("auto".to_string()),
        ToolChoice::Required => OpenAIToolChoice::String("required".to_string()),
        ToolChoice::None => OpenAIToolChoice::String("none".to_string()),
        ToolChoice::Specific { name } => OpenAIToolChoice::Object {
            r#type: "function".to_string(),
            function: OpenAIFunctionName { name },
        },
    });

    // GPT-5 models use max_completion_tokens instead of max_tokens
    let is_gpt5 = req.model.starts_with("gpt-5") || req.model.starts_with("o1") || req.model.starts_with("o3");
    let (max_tokens, max_completion_tokens) = if is_gpt5 {
        (None, req.max_tokens)
    } else {
        (req.max_tokens, None)
    };

    Ok(OpenAIChatRequest {
        model: req.model,
        messages,
        temperature: req.temperature,
        top_p: req.top_p,
        max_tokens,
        max_completion_tokens,
        stream: Some(req.stream),
        stop: if req.stop_sequences.is_empty() {
            None
        } else {
            Some(req.stop_sequences)
        },
        tools,
        tool_choice,
    })
}

fn from_openai_response(resp: OpenAIChatResponse) -> Result<NormalizedResponse> {
    let choices = resp
        .choices
        .into_iter()
        .map(|choice| {
            let role = match choice.message.role.as_str() {
                "assistant" => Role::Assistant,
                "system" => Role::System,
                "user" => Role::User,
                "tool" => Role::Tool,
                _ => Role::Assistant,
            };

            let content = choice
                .message
                .content
                .map(MessageContent::Text)
                .unwrap_or_else(|| MessageContent::Text(String::new()));

            let tool_calls = choice
                .message
                .tool_calls
                .unwrap_or_default()
                .into_iter()
                .map(|tc| ToolCall {
                    id: tc.id,
                    tool_type: tc.tool_type,
                    function: FunctionCall {
                        name: tc.function.name,
                        arguments: tc.function.arguments,
                    },
                })
                .collect();

            let finish_reason = choice.finish_reason.and_then(|fr| match fr.as_str() {
                "stop" => Some(FinishReason::Stop),
                "length" => Some(FinishReason::Length),
                "tool_calls" => Some(FinishReason::ToolCalls),
                "content_filter" => Some(FinishReason::ContentFilter),
                _ => None,
            });

            lunaroute_core::normalized::Choice {
                index: choice.index,
                message: Message {
                    role,
                    content,
                    name: choice.message.name,
                    tool_calls,
                    tool_call_id: choice.message.tool_call_id,
                },
                finish_reason,
            }
        })
        .collect();

    Ok(NormalizedResponse {
        id: resp.id,
        model: resp.model,
        choices,
        usage: Usage {
            prompt_tokens: resp.usage.prompt_tokens,
            completion_tokens: resp.usage.completion_tokens,
            total_tokens: resp.usage.total_tokens,
        },
        created: resp.created,
        metadata: std::collections::HashMap::new(),
    })
}

fn create_openai_stream(
    response: reqwest::Response,
) -> Pin<Box<dyn Stream<Item = lunaroute_core::Result<NormalizedStreamEvent>> + Send + Unpin>> {
    use futures::StreamExt;

    let byte_stream = response.bytes_stream();
    let event_stream = eventsource_stream::EventStream::new(byte_stream);

    let stream = event_stream.map(|result| match result {
        Ok(event) => {
            if event.data == "[DONE]" {
                return Ok(NormalizedStreamEvent::End {
                    finish_reason: FinishReason::Stop,
                });
            }

            match serde_json::from_str::<OpenAIStreamChunk>(&event.data) {
                Ok(chunk) => {
                    // Convert chunk to normalized events
                    if let Some(usage) = chunk.usage {
                        return Ok(NormalizedStreamEvent::Usage {
                            usage: Usage {
                                prompt_tokens: usage.prompt_tokens,
                                completion_tokens: usage.completion_tokens,
                                total_tokens: usage.total_tokens,
                            },
                        });
                    }

                    if let Some(choice) = chunk.choices.first() {
                        if let Some(ref finish_reason) = choice.finish_reason {
                            let reason = match finish_reason.as_str() {
                                "stop" => FinishReason::Stop,
                                "length" => FinishReason::Length,
                                "tool_calls" => FinishReason::ToolCalls,
                                "content_filter" => FinishReason::ContentFilter,
                                _ => FinishReason::Stop,
                            };
                            return Ok(NormalizedStreamEvent::End { finish_reason: reason });
                        }

                        if let Some(ref content) = choice.delta.content {
                            return Ok(NormalizedStreamEvent::Delta {
                                index: choice.index,
                                delta: Delta {
                                    role: choice.delta.role.as_ref().and_then(|r| match r.as_str() {
                                        "assistant" => Some(Role::Assistant),
                                        "user" => Some(Role::User),
                                        "system" => Some(Role::System),
                                        "tool" => Some(Role::Tool),
                                        _ => None,
                                    }),
                                    content: Some(content.clone()),
                                },
                            });
                        }

                        if let Some(ref tool_calls) = choice.delta.tool_calls
                            && let Some(tool_call) = tool_calls.first()
                        {
                            return Ok(NormalizedStreamEvent::ToolCallDelta {
                                index: choice.index,
                                tool_call_index: tool_call.index,
                                id: tool_call.id.clone(),
                                function: tool_call.function.as_ref().map(|f| FunctionCallDelta {
                                    name: f.name.clone(),
                                    arguments: f.arguments.clone(),
                                }),
                            });
                        }
                    }

                    // Default to start event
                    Ok(NormalizedStreamEvent::Start {
                        id: chunk.id,
                        model: String::new(),
                    })
                }
                Err(e) => Err(lunaroute_core::Error::Provider(format!(
                    "Failed to parse OpenAI stream chunk: {}",
                    e
                ))),
            }
        }
        Err(e) => Err(lunaroute_core::Error::Provider(format!("Stream error: {}", e))),
    });

    Box::pin(stream)
}

// Helper trait for adding organization header
trait OrganizationHeader {
    fn apply_organization_header(self, config: &OpenAIConfig) -> Self;
}

impl OrganizationHeader for reqwest::RequestBuilder {
    fn apply_organization_header(self, config: &OpenAIConfig) -> Self {
        if let Some(ref org) = config.organization {
            self.header("OpenAI-Organization", org)
        } else {
            self
        }
    }
}

// Helper trait for handling responses
#[async_trait]
trait OpenAIResponseHandler {
    async fn handle_openai_response(self) -> Result<OpenAIChatResponse>;
}

#[async_trait]
impl OpenAIResponseHandler for reqwest::Response {
    async fn handle_openai_response(self) -> Result<OpenAIChatResponse> {
        let status = self.status();

        if !status.is_success() {
            let status_code = status.as_u16();
            let body = self
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());

            return Err(if status_code == 429 {
                EgressError::RateLimitExceeded {
                    retry_after_secs: None, // Could parse from headers
                }
            } else {
                EgressError::ProviderError {
                    status_code,
                    message: body,
                }
            });
        }

        self.json::<OpenAIChatResponse>()
            .await
            .map_err(|e| EgressError::ParseError(format!("Failed to parse OpenAI response: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunaroute_core::normalized::{Tool, FunctionDefinition, FunctionCall};

    #[test]
    fn test_openai_config_builder() {
        let config = OpenAIConfig::new("test-key")
            .with_base_url("https://custom.api.com")
            .with_organization("org-123");

        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.base_url, "https://custom.api.com");
        assert_eq!(config.organization, Some("org-123".to_string()));
    }

    #[test]
    fn test_connector_creation() {
        let config = OpenAIConfig::new("test-key");
        let connector = OpenAIConnector::new(config);
        assert!(connector.is_ok());
    }

    #[test]
    fn test_capabilities() {
        let config = OpenAIConfig::new("test-key");
        let connector = OpenAIConnector::new(config).unwrap();
        let caps = connector.capabilities();
        assert!(caps.supports_streaming);
        assert!(caps.supports_tools);
        assert!(caps.supports_vision);
    }

    #[test]
    fn test_to_openai_request_basic() {
        let normalized = NormalizedRequest {
            model: "gpt-4".to_string(),
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

        let openai_req = to_openai_request(normalized).unwrap();
        assert_eq!(openai_req.model, "gpt-4");
        assert_eq!(openai_req.messages.len(), 1);
        assert_eq!(openai_req.messages[0].role, "user");
        assert_eq!(openai_req.messages[0].content, Some("Hello".to_string()));
    }

    #[test]
    fn test_from_openai_response() {
        let openai_resp = OpenAIChatResponse {
            id: "chatcmpl-123".to_string(),
            model: "gpt-4".to_string(),
            choices: vec![OpenAIChoice {
                index: 0,
                message: OpenAIMessage {
                    role: "assistant".to_string(),
                    content: Some("Hello!".to_string()),
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
            created: 1234567890,
        };

        let normalized = from_openai_response(openai_resp).unwrap();
        assert_eq!(normalized.id, "chatcmpl-123");
        assert_eq!(normalized.choices[0].message.role, Role::Assistant);
        assert_eq!(normalized.usage.total_tokens, 15);
    }

    // Tool conversion tests
    #[test]
    fn test_to_openai_request_with_tools() {
        let normalized = NormalizedRequest {
            model: "gpt-4".to_string(),
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
            tool_choice: Some(ToolChoice::Auto),
            metadata: std::collections::HashMap::new(),
        };

        let openai_req = to_openai_request(normalized).unwrap();
        assert_eq!(openai_req.tools.as_ref().unwrap().len(), 1);
        assert_eq!(openai_req.tools.as_ref().unwrap()[0].function.name, "get_weather");
        assert!(matches!(openai_req.tool_choice, Some(OpenAIToolChoice::String(ref s)) if s == "auto"));
    }

    #[test]
    fn test_to_openai_request_tool_choice_variants() {
        // Test Auto
        let req = NormalizedRequest {
            model: "gpt-4".to_string(),
            messages: vec![],
            system: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec![],
            tools: vec![],
            tool_choice: Some(ToolChoice::Auto),
            metadata: std::collections::HashMap::new(),
        };
        let openai = to_openai_request(req).unwrap();
        assert!(matches!(openai.tool_choice, Some(OpenAIToolChoice::String(ref s)) if s == "auto"));

        // Test Required
        let req = NormalizedRequest {
            model: "gpt-4".to_string(),
            messages: vec![],
            system: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec![],
            tools: vec![],
            tool_choice: Some(ToolChoice::Required),
            metadata: std::collections::HashMap::new(),
        };
        let openai = to_openai_request(req).unwrap();
        assert!(matches!(openai.tool_choice, Some(OpenAIToolChoice::String(ref s)) if s == "required"));

        // Test None
        let req = NormalizedRequest {
            model: "gpt-4".to_string(),
            messages: vec![],
            system: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec![],
            tools: vec![],
            tool_choice: Some(ToolChoice::None),
            metadata: std::collections::HashMap::new(),
        };
        let openai = to_openai_request(req).unwrap();
        assert!(matches!(openai.tool_choice, Some(OpenAIToolChoice::String(ref s)) if s == "none"));

        // Test Specific
        let req = NormalizedRequest {
            model: "gpt-4".to_string(),
            messages: vec![],
            system: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec![],
            tools: vec![],
            tool_choice: Some(ToolChoice::Specific { name: "my_func".to_string() }),
            metadata: std::collections::HashMap::new(),
        };
        let openai = to_openai_request(req).unwrap();
        match openai.tool_choice {
            Some(OpenAIToolChoice::Object { function, .. }) => {
                assert_eq!(function.name, "my_func");
            }
            _ => panic!("Expected Object variant"),
        }
    }

    #[test]
    fn test_to_openai_request_with_tool_calls() {
        let normalized = NormalizedRequest {
            model: "gpt-4".to_string(),
            messages: vec![
                Message {
                    role: Role::User,
                    content: MessageContent::Text("Get weather".to_string()),
                    name: None,
                    tool_calls: vec![],
                    tool_call_id: None,
                },
                Message {
                    role: Role::Assistant,
                    content: MessageContent::Text("".to_string()),
                    name: None,
                    tool_calls: vec![ToolCall {
                        id: "call_123".to_string(),
                        tool_type: "function".to_string(),
                        function: FunctionCall {
                            name: "get_weather".to_string(),
                            arguments: r#"{"location":"NYC"}"#.to_string(),
                        },
                    }],
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

        let openai_req = to_openai_request(normalized).unwrap();
        assert_eq!(openai_req.messages.len(), 2);
        assert!(openai_req.messages[1].tool_calls.is_some());
        assert_eq!(openai_req.messages[1].tool_calls.as_ref().unwrap()[0].id, "call_123");
        // Content should be None when message has tool calls
        assert_eq!(openai_req.messages[1].content, None);
    }

    #[test]
    fn test_from_openai_response_with_tool_calls() {
        let openai_resp = OpenAIChatResponse {
            id: "chatcmpl-123".to_string(),
            model: "gpt-4".to_string(),
            choices: vec![OpenAIChoice {
                index: 0,
                message: OpenAIMessage {
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
                finish_reason: Some("tool_calls".to_string()),
            }],
            usage: OpenAIUsage {
                prompt_tokens: 20,
                completion_tokens: 10,
                total_tokens: 30,
            },
            created: 1234567890,
        };

        let normalized = from_openai_response(openai_resp).unwrap();
        assert_eq!(normalized.choices[0].message.tool_calls.len(), 1);
        assert_eq!(normalized.choices[0].message.tool_calls[0].function.name, "get_weather");
        assert_eq!(normalized.choices[0].finish_reason, Some(FinishReason::ToolCalls));
    }

    #[test]
    fn test_to_openai_request_multiple_tool_calls() {
        let normalized = NormalizedRequest {
            model: "gpt-4".to_string(),
            messages: vec![Message {
                role: Role::Assistant,
                content: MessageContent::Text("".to_string()),
                name: None,
                tool_calls: vec![
                    ToolCall {
                        id: "call_1".to_string(),
                        tool_type: "function".to_string(),
                        function: FunctionCall {
                            name: "func1".to_string(),
                            arguments: "{}".to_string(),
                        },
                    },
                    ToolCall {
                        id: "call_2".to_string(),
                        tool_type: "function".to_string(),
                        function: FunctionCall {
                            name: "func2".to_string(),
                            arguments: "{}".to_string(),
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

        let openai_req = to_openai_request(normalized).unwrap();
        assert_eq!(openai_req.messages[0].tool_calls.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_from_openai_response_all_finish_reasons() {
        let finish_reasons = vec![
            ("stop", FinishReason::Stop),
            ("length", FinishReason::Length),
            ("tool_calls", FinishReason::ToolCalls),
            ("content_filter", FinishReason::ContentFilter),
        ];

        for (openai_reason, expected_reason) in finish_reasons {
            let openai_resp = OpenAIChatResponse {
                id: "test".to_string(),
                model: "gpt-4".to_string(),
                choices: vec![OpenAIChoice {
                    index: 0,
                    message: OpenAIMessage {
                        role: "assistant".to_string(),
                        content: Some("test".to_string()),
                        name: None,
                        tool_calls: None,
                        tool_call_id: None,
                    },
                    finish_reason: Some(openai_reason.to_string()),
                }],
                usage: OpenAIUsage {
                    prompt_tokens: 1,
                    completion_tokens: 1,
                    total_tokens: 2,
                },
                created: 0,
            };

            let normalized = from_openai_response(openai_resp).unwrap();
            assert_eq!(normalized.choices[0].finish_reason, Some(expected_reason));
        }
    }

    // Edge case tests
    #[test]
    fn test_to_openai_request_multimodal_content() {
        use lunaroute_core::normalized::{ContentPart, ImageSource};

        let normalized = NormalizedRequest {
            model: "gpt-4-vision".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Parts(vec![
                    ContentPart::Text { text: "First".to_string() },
                    ContentPart::Image {
                        source: ImageSource::Url {
                            url: "https://example.com/image.jpg".to_string(),
                        },
                    },
                    ContentPart::Text { text: "Second".to_string() },
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

        let openai_req = to_openai_request(normalized).unwrap();
        // Should extract text and join with newlines, ignoring images
        assert_eq!(openai_req.messages[0].content, Some("First\nSecond".to_string()));
    }

    #[test]
    fn test_to_openai_request_empty_tools() {
        let normalized = NormalizedRequest {
            model: "gpt-4".to_string(),
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

        let openai_req = to_openai_request(normalized).unwrap();
        assert!(openai_req.tools.is_none());
    }

    #[test]
    fn test_to_openai_request_with_stop_sequences() {
        let normalized = NormalizedRequest {
            model: "gpt-4".to_string(),
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

        let openai_req = to_openai_request(normalized).unwrap();
        assert_eq!(openai_req.stop, Some(vec!["STOP".to_string(), "END".to_string()]));
    }

    #[test]
    fn test_from_openai_response_multiple_choices() {
        let openai_resp = OpenAIChatResponse {
            id: "test".to_string(),
            model: "gpt-4".to_string(),
            choices: vec![
                OpenAIChoice {
                    index: 0,
                    message: OpenAIMessage {
                        role: "assistant".to_string(),
                        content: Some("First".to_string()),
                        name: None,
                        tool_calls: None,
                        tool_call_id: None,
                    },
                    finish_reason: Some("stop".to_string()),
                },
                OpenAIChoice {
                    index: 1,
                    message: OpenAIMessage {
                        role: "assistant".to_string(),
                        content: Some("Second".to_string()),
                        name: None,
                        tool_calls: None,
                        tool_call_id: None,
                    },
                    finish_reason: Some("stop".to_string()),
                },
            ],
            usage: OpenAIUsage {
                prompt_tokens: 10,
                completion_tokens: 10,
                total_tokens: 20,
            },
            created: 0,
        };

        let normalized = from_openai_response(openai_resp).unwrap();
        assert_eq!(normalized.choices.len(), 2);
        assert_eq!(normalized.choices[0].index, 0);
        assert_eq!(normalized.choices[1].index, 1);
    }

    #[test]
    fn test_from_openai_response_empty_content() {
        let openai_resp = OpenAIChatResponse {
            id: "test".to_string(),
            model: "gpt-4".to_string(),
            choices: vec![OpenAIChoice {
                index: 0,
                message: OpenAIMessage {
                    role: "assistant".to_string(),
                    content: None,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: OpenAIUsage {
                prompt_tokens: 10,
                completion_tokens: 0,
                total_tokens: 10,
            },
            created: 0,
        };

        let normalized = from_openai_response(openai_resp).unwrap();
        // Should default to empty string
        match &normalized.choices[0].message.content {
            MessageContent::Text(text) => assert_eq!(text, ""),
            _ => panic!("Expected Text content"),
        }
    }
}
