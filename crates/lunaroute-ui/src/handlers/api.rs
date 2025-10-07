//! JSON API handlers

use crate::{models::*, queries, AppState};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;

/// Query parameters for stats
#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    #[serde(default = "default_hours")]
    hours: i64,
}

fn default_hours() -> i64 {
    24
}

/// Query parameters for time series
#[derive(Debug, Deserialize)]
pub struct TimeSeriesQuery {
    #[serde(default = "default_days")]
    days: i64,
}

fn default_days() -> i64 {
    7
}

/// Query parameters for timeline pagination
#[derive(Debug, Deserialize)]
pub struct TimelineQuery {
    #[serde(default = "default_offset")]
    offset: i64,
    #[serde(default = "default_limit")]
    limit: i64,
}

fn default_offset() -> i64 {
    0
}
fn default_limit() -> i64 {
    20
}

/// Query parameters for sessions list pagination
#[derive(Debug, Deserialize)]
pub struct SessionsQuery {
    #[serde(default = "default_offset")]
    offset: i64,
    #[serde(default = "default_sessions_limit")]
    limit: i64,
    user_agent: Option<String>,
    model: Option<String>,
}

fn default_sessions_limit() -> i64 {
    50
}

/// Overview statistics
pub async fn overview_stats(
    State(state): State<AppState>,
    Query(params): Query<StatsQuery>,
) -> Json<OverviewStats> {
    let stats = queries::get_overview_stats(&state.db, params.hours)
        .await
        .unwrap_or(OverviewStats {
            total_sessions: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_thinking_tokens: 0,
            total_tokens: 0,
            avg_duration_ms: 0.0,
            success_rate: 100.0,
            total_cost: 0.0,
        });
    Json(stats)
}

/// Token time series
pub async fn token_stats(
    State(state): State<AppState>,
    Query(params): Query<TimeSeriesQuery>,
) -> Json<Vec<TokenTimeSeries>> {
    let stats = queries::get_token_time_series(&state.db, params.days)
        .await
        .unwrap_or_default();
    Json(stats)
}

/// Tool usage statistics
pub async fn tool_stats(State(state): State<AppState>) -> Json<Vec<ToolUsage>> {
    let stats = queries::get_tool_usage(&state.db).await.unwrap_or_default();
    Json(stats)
}

/// Cost statistics
pub async fn cost_stats(State(state): State<AppState>) -> Json<CostStats> {
    let stats = queries::get_cost_stats(&state.db)
        .await
        .unwrap_or(CostStats {
            today: 0.0,
            this_week: 0.0,
            this_month: 0.0,
            total: 0.0,
            projection_monthly: 0.0,
        });
    Json(stats)
}

/// Model usage statistics
pub async fn model_stats(State(state): State<AppState>) -> Json<Vec<ModelUsage>> {
    let stats = queries::get_model_usage(&state.db)
        .await
        .unwrap_or_default();
    Json(stats)
}

/// Sessions list with pagination and filtering
pub async fn sessions_list(
    Query(params): Query<SessionsQuery>,
    State(state): State<AppState>,
) -> Json<Vec<SessionSummary>> {
    let sessions = queries::get_sessions_paginated(
        &state.db,
        params.offset,
        params.limit,
        params.user_agent.as_deref(),
        params.model.as_deref(),
    )
    .await
    .unwrap_or_default();
    Json(sessions)
}

/// Get available user agents
pub async fn user_agents_list(State(state): State<AppState>) -> Json<Vec<String>> {
    let user_agents = queries::get_user_agents(&state.db)
        .await
        .unwrap_or_default();
    Json(user_agents)
}

/// Get available models
pub async fn models_list(State(state): State<AppState>) -> Json<Vec<String>> {
    let models = queries::get_models(&state.db).await.unwrap_or_default();
    Json(models)
}

/// Recent sessions (for dashboard)
pub async fn recent_sessions(State(state): State<AppState>) -> Json<Vec<SessionSummary>> {
    let sessions = queries::get_recent_sessions(&state.db, 10)
        .await
        .unwrap_or_default();
    Json(sessions)
}

/// Session detail
pub async fn session_detail(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
) -> Json<SessionDetail> {
    let detail = queries::get_session_detail(&state.db, &session_id)
        .await
        .unwrap_or_else(|_| SessionDetail {
            session_id: session_id.clone(),
            request_id: None,
            started_at: "unknown".to_string(),
            completed_at: None,
            provider: "unknown".to_string(),
            listener: "unknown".to_string(),
            model_requested: "unknown".to_string(),
            model_used: None,
            success: false,
            error_message: Some("Session not found".to_string()),
            finish_reason: None,
            total_duration_ms: None,
            provider_latency_ms: None,
            time_to_first_token_ms: None,
            input_tokens: 0,
            output_tokens: 0,
            thinking_tokens: 0,
            reasoning_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            total_tokens: 0,
            is_streaming: false,
            chunk_count: None,
            streaming_duration_ms: None,
            client_ip: None,
            user_agent: None,
            cost: 0.0,
            request_count: 0,
        });
    Json(detail)
}

/// Session timeline
pub async fn session_timeline(
    Path(session_id): Path<String>,
    Query(params): Query<TimelineQuery>,
    State(state): State<AppState>,
) -> Json<Vec<TimelineEvent>> {
    let events = queries::get_session_timeline(&state.db, &session_id, params.offset, params.limit)
        .await
        .unwrap_or_default();
    Json(events)
}

/// Sessions by hour of day
pub async fn hour_of_day_stats(
    State(state): State<AppState>,
    Query(params): Query<StatsQuery>,
) -> Json<Vec<HourOfDayStats>> {
    let stats = queries::get_sessions_by_hour(&state.db, params.hours)
        .await
        .unwrap_or_default();
    Json(stats)
}

/// Spending statistics with per-model breakdown
pub async fn spending_stats(
    State(state): State<AppState>,
    Query(params): Query<StatsQuery>,
) -> Json<SpendingStats> {
    let stats = queries::get_spending_stats(&state.db, params.hours)
        .await
        .unwrap_or_else(|_| SpendingStats {
            total_cost: 0.0,
            avg_cost_per_session: 0.0,
            by_model: vec![],
        });
    Json(stats)
}

/// Tool usage statistics for a specific session
pub async fn session_tool_stats(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
) -> Json<SessionToolStats> {
    let stats = queries::get_session_tool_stats(&state.db, &session_id)
        .await
        .unwrap_or_else(|_| SessionToolStats {
            total_tool_calls: 0,
            tool_usage_percentage: 0.0,
            by_tool: vec![],
        });
    Json(stats)
}
