//! Session events and statistics structures for async recording

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Session event types for async recording
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    Started {
        session_id: String,
        request_id: String,
        timestamp: DateTime<Utc>,
        model_requested: String,
        provider: String,
        listener: String,
        is_streaming: bool,
        #[serde(flatten)]
        metadata: SessionMetadata,
    },

    StreamStarted {
        session_id: String,
        request_id: String,
        timestamp: DateTime<Utc>,
        time_to_first_token_ms: u64,
    },

    RequestRecorded {
        session_id: String,
        request_id: String,
        timestamp: DateTime<Utc>,
        request_text: String,
        request_json: Value,
        estimated_tokens: u32,
        #[serde(flatten)]
        stats: RequestStats,
    },

    ResponseRecorded {
        session_id: String,
        request_id: String,
        timestamp: DateTime<Utc>,
        response_text: String,
        response_json: Value,
        model_used: String,
        #[serde(flatten)]
        stats: ResponseStats,
    },

    ToolCallRecorded {
        session_id: String,
        request_id: String,
        timestamp: DateTime<Utc>,
        tool_name: String,
        tool_call_id: String,
        execution_time_ms: Option<u64>,
        input_size_bytes: usize,
        output_size_bytes: Option<usize>,
        success: Option<bool>,
    },

    StatsSnapshot {
        session_id: String,
        request_id: String,
        timestamp: DateTime<Utc>,
        #[serde(flatten)]
        stats: SessionStats,
    },

    Completed {
        session_id: String,
        request_id: String,
        timestamp: DateTime<Utc>,
        success: bool,
        error: Option<String>,
        finish_reason: Option<String>,
        #[serde(flatten)]
        final_stats: Box<FinalSessionStats>,
    },

    /// Update session stats after async parsing completes (passthrough mode)
    /// This event is emitted when we parse streaming/response data asynchronously
    /// and discover token counts or tool calls that weren't available initially
    StatsUpdated {
        session_id: String,
        request_id: String,
        timestamp: DateTime<Utc>,
        /// Updated token counts (if parsed from response)
        token_updates: Option<TokenTotals>,
        /// Updated tool usage (if parsed from response)
        tool_call_updates: Option<ToolUsageSummary>,
        /// Model name extracted from response
        model_used: Option<String>,
        /// Response size in bytes
        response_size_bytes: usize,
        /// Number of content blocks in response
        content_blocks: usize,
        /// Whether response contains a refusal
        has_refusal: bool,
        /// User agent of the client that made the request
        user_agent: Option<String>,
    },
}

impl SessionEvent {
    pub fn session_id(&self) -> &str {
        match self {
            Self::Started { session_id, .. } => session_id,
            Self::StreamStarted { session_id, .. } => session_id,
            Self::RequestRecorded { session_id, .. } => session_id,
            Self::ResponseRecorded { session_id, .. } => session_id,
            Self::ToolCallRecorded { session_id, .. } => session_id,
            Self::StatsSnapshot { session_id, .. } => session_id,
            Self::Completed { session_id, .. } => session_id,
            Self::StatsUpdated { session_id, .. } => session_id,
        }
    }

    pub fn request_id(&self) -> &str {
        match self {
            Self::Started { request_id, .. } => request_id,
            Self::StreamStarted { request_id, .. } => request_id,
            Self::RequestRecorded { request_id, .. } => request_id,
            Self::ResponseRecorded { request_id, .. } => request_id,
            Self::ToolCallRecorded { request_id, .. } => request_id,
            Self::StatsSnapshot { request_id, .. } => request_id,
            Self::Completed { request_id, .. } => request_id,
            Self::StatsUpdated { request_id, .. } => request_id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub client_ip: Option<String>,
    pub user_agent: Option<String>,
    pub api_version: Option<String>,
    #[serde(default)]
    pub request_headers: HashMap<String, String>,
    #[serde(default)]
    pub session_tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestStats {
    pub pre_processing_ms: f64,
    pub request_size_bytes: usize,
    pub message_count: usize,
    pub has_system_prompt: bool,
    pub has_tools: bool,
    pub tool_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseStats {
    pub provider_latency_ms: u64,
    pub post_processing_ms: f64,
    pub total_proxy_overhead_ms: f64,
    #[serde(flatten)]
    pub tokens: TokenStats,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallStats>,
    pub response_size_bytes: usize,
    pub content_blocks: usize,
    pub has_refusal: bool,
    // Streaming-specific fields
    pub is_streaming: bool,
    pub chunk_count: Option<u32>,
    pub streaming_duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenStats {
    // Core tokens (always present)
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,

    // Extended/reasoning tokens (model-specific)
    pub thinking_tokens: Option<u32>,         // Anthropic extended thinking
    pub reasoning_tokens: Option<u32>,        // OpenAI o1/o3/o4 reasoning

    // Cache tokens (separated by type)
    pub cache_read_tokens: Option<u32>,       // Tokens FROM cache (cheap)
    pub cache_creation_tokens: Option<u32>,   // Tokens TO cache (normal price)

    // Audio/multimodal tokens
    pub audio_input_tokens: Option<u32>,
    pub audio_output_tokens: Option<u32>,

    // Metrics
    pub thinking_percentage: Option<f32>,
    pub tokens_per_second: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallStats {
    pub tool_name: String,
    pub tool_call_id: Option<String>,
    pub execution_time_ms: Option<u64>,
    pub input_size_bytes: usize,
    pub output_size_bytes: Option<usize>,
    pub success: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStats {
    pub request_count: u32,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_thinking_tokens: u64,
    pub total_tool_calls: u32,
    #[serde(default)]
    pub unique_tools: Vec<String>,
    pub cumulative_latency_ms: u64,
    pub cumulative_proxy_overhead_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalSessionStats {
    pub total_duration_ms: u64,
    pub provider_time_ms: u64,
    pub proxy_overhead_ms: f64,
    #[serde(flatten)]
    pub total_tokens: TokenTotals,
    #[serde(flatten)]
    pub tool_summary: ToolUsageSummary,
    #[serde(flatten)]
    pub performance: PerformanceMetrics,
    pub streaming_stats: Option<StreamingStats>,
    pub estimated_cost: Option<CostEstimate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingStats {
    pub time_to_first_token_ms: u64,
    pub total_chunks: u32,
    pub streaming_duration_ms: u64,
    pub avg_chunk_latency_ms: f64,
    pub p50_chunk_latency_ms: Option<u64>,
    pub p95_chunk_latency_ms: Option<u64>,
    pub p99_chunk_latency_ms: Option<u64>,
    pub max_chunk_latency_ms: u64,
    pub min_chunk_latency_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenTotals {
    pub total_input: u64,
    pub total_output: u64,
    pub total_thinking: u64,  // Anthropic extended thinking
    #[serde(default)]
    pub total_reasoning: u64,  // OpenAI o1/o3/o4 reasoning
    pub total_cached: u64,  // Deprecated: use total_cache_read instead
    #[serde(default)]
    pub total_cache_read: u64,  // Tokens FROM cache (discounted)
    #[serde(default)]
    pub total_cache_creation: u64,  // Tokens TO cache (normal price)
    #[serde(default)]
    pub total_audio_input: u64,  // Audio input tokens
    #[serde(default)]
    pub total_audio_output: u64,  // Audio output tokens
    pub grand_total: u64,
    #[serde(default)]
    pub by_model: HashMap<String, TokenStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolUsageSummary {
    pub total_tool_calls: u32,
    pub unique_tool_count: u32,
    #[serde(default)]
    pub by_tool: HashMap<String, ToolStats>,
    pub total_tool_time_ms: u64,
    pub tool_error_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolStats {
    pub call_count: u32,
    pub total_execution_time_ms: u64,
    pub avg_execution_time_ms: u64,
    pub error_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PerformanceMetrics {
    pub avg_provider_latency_ms: f64,
    pub p50_latency_ms: Option<u64>,
    pub p95_latency_ms: Option<u64>,
    pub p99_latency_ms: Option<u64>,
    pub max_latency_ms: u64,
    pub min_latency_ms: u64,
    pub avg_pre_processing_ms: f64,
    pub avg_post_processing_ms: f64,
    pub proxy_overhead_percentage: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEstimate {
    pub provider: String,
    pub model: String,
    pub input_cost_usd: f64,
    pub output_cost_usd: f64,
    pub thinking_cost_usd: Option<f64>,
    pub total_cost_usd: f64,
    pub cost_per_1k_tokens: f64,
}
