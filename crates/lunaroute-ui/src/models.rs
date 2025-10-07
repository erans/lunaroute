//! Data models for templates and API responses

use serde::{Deserialize, Serialize};

/// Overview statistics for dashboard
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverviewStats {
    pub total_sessions: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_thinking_tokens: i64,
    pub total_tokens: i64,
    pub avg_duration_ms: f64,
    pub success_rate: f64,
    pub total_cost: f64,
}

/// Token time series data point
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenTimeSeries {
    pub date: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub thinking_tokens: i64,
}

/// Tool usage statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUsage {
    pub tool_name: String,
    pub call_count: i64,
    pub avg_time_ms: f64,
    pub min_time_ms: f64,
    pub max_time_ms: f64,
    pub success_count: i64,
}

/// Cost statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostStats {
    pub today: f64,
    pub this_week: f64,
    pub this_month: f64,
    pub total: f64,
    pub projection_monthly: f64,
}

/// Model usage statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsage {
    pub model_name: String,
    pub request_count: i64,
    pub percentage: f64,
}

/// Recent session summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub started_at: String,
    pub model: String,
    pub request_count: i64,
    pub total_tokens: i64,
    pub cost: f64,
    pub duration_ms: i64,
    pub success: bool,
}

/// Detailed session information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDetail {
    pub session_id: String,
    pub request_id: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub provider: String,
    pub listener: String,
    pub model_requested: String,
    pub model_used: Option<String>,
    pub success: bool,
    pub error_message: Option<String>,
    pub finish_reason: Option<String>,

    // Timing
    pub total_duration_ms: Option<i64>,
    pub provider_latency_ms: Option<i64>,
    pub time_to_first_token_ms: Option<i64>,

    // Tokens
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub thinking_tokens: i64,
    pub reasoning_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub total_tokens: i64,

    // Streaming
    pub is_streaming: bool,
    pub chunk_count: Option<i64>,
    pub streaming_duration_ms: Option<i64>,

    // Metadata
    pub client_ip: Option<String>,
    pub user_agent: Option<String>,

    // Calculated
    pub cost: f64,
    pub request_count: i64,
}

/// Timeline event for session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEvent {
    pub timestamp: String,
    pub event_type: String,
    pub description: String,
    pub metadata: Option<serde_json::Value>,
}

/// Sessions by hour of day
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HourOfDayStats {
    pub hour: i64,
    pub session_count: i64,
}

/// Spending statistics by model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSpending {
    pub model_name: String,
    pub total_cost: f64,
    pub session_count: i64,
    pub avg_cost_per_session: f64,
}

/// Extended spending statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpendingStats {
    pub total_cost: f64,
    pub avg_cost_per_session: f64,
    pub by_model: Vec<ModelSpending>,
}

/// Tool usage statistics for a session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionToolStats {
    pub total_tool_calls: i64,
    pub tool_usage_percentage: f64,
    pub by_tool: Vec<ToolBreakdown>,
}

/// Tool breakdown by type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolBreakdown {
    pub tool_name: String,
    pub call_count: i64,
    pub percentage: f64,
}
