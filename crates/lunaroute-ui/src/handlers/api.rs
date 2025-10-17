//! JSON API handlers

use crate::{models::*, queries, AppState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;

/// Query parameters for stats
#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    /// Time range in hours. If None, returns all-time stats
    hours: Option<i64>,
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
    #[serde(default = "default_order")]
    order: String, // "asc" or "desc"
}

fn default_offset() -> i64 {
    0
}
fn default_limit() -> i64 {
    20
}
fn default_order() -> String {
    "asc".to_string()
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
    let stats = queries::get_overview_stats(&state.db, params.hours.unwrap_or(24))
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

/// Query parameters for model stats
#[derive(Debug, Deserialize)]
pub struct ModelStatsQuery {
    hours: Option<i64>,
    user_agent: Option<String>,
}

/// Model usage statistics
pub async fn model_stats(
    State(state): State<AppState>,
    Query(params): Query<ModelStatsQuery>,
) -> Json<Vec<ModelUsage>> {
    let stats = queries::get_model_usage(
        &state.db,
        params.hours.unwrap_or(24),
        params.user_agent.as_deref(),
    )
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
    let order = if params.order.to_lowercase() == "desc" {
        "DESC"
    } else {
        "ASC"
    };

    let events =
        queries::get_session_timeline(&state.db, &session_id, params.offset, params.limit, order)
            .await
            .unwrap_or_default();
    Json(events)
}

/// Sessions by hour of day
pub async fn hour_of_day_stats(
    State(state): State<AppState>,
    Query(params): Query<StatsQuery>,
) -> Json<Vec<HourOfDayStats>> {
    let stats = queries::get_sessions_by_hour(&state.db, params.hours.unwrap_or(24))
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

/// Get raw request and response from JSONL file
pub async fn session_raw_data(
    Path((session_id, request_id)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Response {
    // Check if sessions_dir is configured
    let sessions_dir = match &state.config.sessions_dir {
        Some(dir) => dir,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "JSONL sessions directory not configured"
                })),
            )
                .into_response();
        }
    };

    // Get session details to find the date
    let session = match queries::get_session_detail(&state.db, &session_id).await {
        Ok(s) => s,
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "Session not found"
                })),
            )
                .into_response();
        }
    };

    // Parse the date from started_at
    // Handle multiple formats:
    // - ISO 8601: "2025-10-07T19:34:04.213124729+00:00"
    // - SQLite: "YYYY-MM-DD HH:MM:SS"
    // - Date only: "YYYY-MM-DD"
    let date = if session.started_at.contains('T') {
        // ISO 8601 format - split on 'T' and take date part
        session
            .started_at
            .split('T')
            .next()
            .unwrap_or(&session.started_at)
            .to_string()
    } else if let Some(date_part) = session.started_at.split_whitespace().next() {
        // SQLite format with space
        date_part.to_string()
    } else if session.started_at.len() >= 10 {
        // Take first 10 characters
        session.started_at[0..10].to_string()
    } else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "Invalid session timestamp format",
                "started_at": session.started_at
            })),
        )
            .into_response();
    };

    // Try to find the JSONL file - check current date and neighboring dates
    // (in case of timezone differences or date boundary issues)
    let mut dates_to_try = vec![date.clone()];

    // Try to parse the date and add +/- 1 day variants
    if let Ok(parsed_date) = chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d") {
        let prev_day = parsed_date - chrono::Duration::days(1);
        let next_day = parsed_date + chrono::Duration::days(1);
        dates_to_try.push(prev_day.format("%Y-%m-%d").to_string());
        dates_to_try.push(next_day.format("%Y-%m-%d").to_string());
    }

    let mut jsonl_path = None;
    let mut attempted_paths = Vec::new();

    for check_date in &dates_to_try {
        let path = sessions_dir
            .join(check_date)
            .join(format!("{}.jsonl", session_id));
        attempted_paths.push(path.display().to_string());

        if tokio::fs::metadata(&path).await.is_ok() {
            jsonl_path = Some(path);
            break;
        }
    }

    let jsonl_path = match jsonl_path {
        Some(p) => p,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "JSONL file not found",
                    "session_id": session_id,
                    "started_at": session.started_at,
                    "parsed_date": date,
                    "attempted_paths": attempted_paths,
                    "sessions_dir": sessions_dir.display().to_string()
                })),
            )
                .into_response();
        }
    };

    // Read the JSONL file
    let content = match tokio::fs::read_to_string(&jsonl_path).await {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("Failed to read JSONL file: {}", e),
                    "path": jsonl_path.display().to_string()
                })),
            )
                .into_response();
        }
    };

    // Parse each line and find events with matching request_id
    let mut request_json = None;
    let mut response_json = None;
    let mut all_request_ids = Vec::new(); // Track all request IDs we see
    let mut all_event_types = Vec::new(); // Track all event types we see

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }

        // Parse as SessionEvent
        let event: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue, // Skip invalid lines
        };

        // Collect debug info
        if let Some(rid) = event.get("request_id").and_then(|v| v.as_str()) {
            all_request_ids.push(rid.to_string());
        }
        if let Some(et) = event.get("type").and_then(|v| v.as_str()) {
            all_event_types.push(et.to_string());
        }

        // Check if this event has the matching request_id
        if event.get("request_id").and_then(|v| v.as_str()) != Some(&request_id) {
            continue;
        }

        // Check event type
        match event.get("type").and_then(|v| v.as_str()) {
            Some("request_recorded") => {
                request_json = event.get("request_json").cloned();
            }
            Some("response_recorded") => {
                response_json = event.get("response_json").cloned();
            }
            _ => {}
        }

        // If we found both, we can stop
        if request_json.is_some() && response_json.is_some() {
            break;
        }
    }

    // If nothing found, return debug info
    if request_json.is_none() && response_json.is_none() {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "error": "No matching request/response found",
                "request_id": request_id,
                "session_id": session_id,
                "file_path": jsonl_path.display().to_string(),
                "total_lines": content.lines().count(),
                "request_ids_in_file": all_request_ids,
                "event_types_in_file": all_event_types,
                "hint": "Check if the request_id matches any events in this session"
            })),
        )
            .into_response();
    }

    // Return the data
    Json(RawRequestResponse {
        session_id: session_id.clone(),
        request_id: request_id.clone(),
        request: request_json,
        response: response_json,
    })
    .into_response()
}
