//! Normalized request and response types

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Normalized request structure that can represent requests from any provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedRequest {
    /// List of messages in the conversation
    pub messages: Vec<Message>,

    /// Optional system message/prompt
    pub system: Option<String>,

    /// Model identifier
    pub model: String,

    /// Maximum number of tokens to generate
    pub max_tokens: Option<u32>,

    /// Sampling temperature (0.0 to 2.0)
    pub temperature: Option<f32>,

    /// Nucleus sampling threshold
    pub top_p: Option<f32>,

    /// Top-k sampling
    pub top_k: Option<u32>,

    /// Stop sequences
    pub stop_sequences: Vec<String>,

    /// Whether to stream the response
    pub stream: bool,

    /// Available tools/functions
    pub tools: Vec<Tool>,

    /// Tool choice configuration
    pub tool_choice: Option<ToolChoice>,

    /// Tool results from previous execution (if this is a follow-up)
    #[serde(default)]
    pub tool_results: Vec<ToolResult>,

    /// Additional metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

/// A single message in a conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Role of the message sender
    pub role: Role,

    /// Content of the message (can be text or multimodal)
    pub content: MessageContent,

    /// Optional name of the sender
    pub name: Option<String>,

    /// Tool calls made in this message (for assistant messages)
    pub tool_calls: Vec<ToolCall>,

    /// Tool call ID (for tool response messages)
    pub tool_call_id: Option<String>,
}

/// Role of a message sender
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// Content of a message (text or multimodal)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// Simple text content
    Text(String),

    /// Multimodal content (text, images, etc.)
    Parts(Vec<ContentPart>),
}

/// A part of multimodal content
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    Image { source: ImageSource },
}

/// Source of an image
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    Url { url: String },
    Base64 { media_type: String, data: String },
}

/// Tool/function definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    /// Type of tool (currently only "function")
    #[serde(rename = "type")]
    pub tool_type: String,

    /// Function definition
    pub function: FunctionDefinition,
}

/// Function definition for a tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    /// Name of the function
    pub name: String,

    /// Description of what the function does
    pub description: Option<String>,

    /// JSON schema for the function parameters
    pub parameters: serde_json::Value,
}

/// Tool choice configuration
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolChoice {
    /// Auto mode - model decides
    Auto,

    /// Required - model must use a tool
    Required,

    /// None - model must not use a tool
    None,

    /// Specific tool to use
    Specific { name: String },
}

/// A tool call made by the assistant
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique ID for this tool call
    pub id: String,

    /// Type of tool (currently only "function")
    #[serde(rename = "type")]
    pub tool_type: String,

    /// Function call details
    pub function: FunctionCall,
}

/// Function call details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    /// Name of the function to call
    pub name: String,

    /// Arguments as JSON string
    pub arguments: String,
}

/// Tool result from client execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// ID of the tool call this is a result for
    pub tool_call_id: String,

    /// Whether this tool execution failed
    pub is_error: bool,

    /// Result content (error message or success data)
    pub content: String,

    /// Optional: tool name if we can determine it
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

/// Normalized response structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedResponse {
    /// Unique ID for this response
    pub id: String,

    /// Model that generated the response
    pub model: String,

    /// Response choices
    pub choices: Vec<Choice>,

    /// Token usage information
    pub usage: Usage,

    /// Timestamp of response creation
    pub created: i64,

    /// Additional metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

/// A single choice in a response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    /// Index of this choice
    pub index: u32,

    /// The message content
    pub message: Message,

    /// Reason why generation stopped
    pub finish_reason: Option<FinishReason>,
}

/// Reason why generation finished
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    /// Natural stop point
    Stop,

    /// Max tokens reached
    Length,

    /// Tool/function was called
    ToolCalls,

    /// Content filtered
    ContentFilter,

    /// Error occurred
    Error,
}

/// Token usage information
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Usage {
    /// Number of tokens in the prompt
    pub prompt_tokens: u32,

    /// Number of tokens in the completion
    pub completion_tokens: u32,

    /// Total tokens used
    pub total_tokens: u32,
}

/// Stream event during response generation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NormalizedStreamEvent {
    /// Stream started
    Start { id: String, model: String },

    /// Content delta
    Delta { index: u32, delta: Delta },

    /// Tool call delta
    ToolCallDelta {
        index: u32,
        tool_call_index: u32,
        id: Option<String>,
        function: Option<FunctionCallDelta>,
    },

    /// Usage information
    Usage { usage: Usage },

    /// Stream ended
    End { finish_reason: FinishReason },

    /// Error occurred
    Error { error: String },
}

/// Content delta in a stream event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delta {
    /// Role (only sent in first delta)
    pub role: Option<Role>,

    /// Content delta
    pub content: Option<String>,
}

/// Function call delta in a stream event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCallDelta {
    /// Function name delta
    pub name: Option<String>,

    /// Arguments delta
    pub arguments: Option<String>,
}

#[cfg(test)]
mod tests;
