//! Database queries for statistics

use crate::models::*;
use crate::stats;
use anyhow::Result;
use sqlx::{Row, SqlitePool};

/// Get overview statistics
pub async fn get_overview_stats(pool: &SqlitePool, hours: i64) -> Result<OverviewStats> {
    let row = sqlx::query(
        r#"
        SELECT
            COUNT(DISTINCT s.session_id) as total_sessions,
            COALESCE(SUM(ss.input_tokens), 0) as total_input_tokens,
            COALESCE(SUM(ss.output_tokens), 0) as total_output_tokens,
            COALESCE(SUM(ss.thinking_tokens), 0) as total_thinking_tokens,
            COALESCE(SUM(ss.input_tokens + ss.output_tokens + COALESCE(ss.thinking_tokens, 0) + COALESCE(ss.reasoning_tokens, 0)), 0) as total_tokens,
            COALESCE(AVG(s.total_duration_ms), 0.0) as avg_duration_ms,
            COALESCE(AVG(CASE WHEN s.success = 1 THEN 100.0 ELSE 0.0 END), 100.0) as success_rate
        FROM sessions s
        LEFT JOIN session_stats ss ON s.session_id = ss.session_id
        WHERE s.started_at >= datetime('now', '-' || ? || ' hours')
        "#
    )
    .bind(hours)
    .fetch_one(pool)
    .await?;

    // Calculate total cost by fetching all session_stats with model info
    let stats_rows = sqlx::query(
        r#"
        SELECT
            ss.model_name,
            ss.input_tokens,
            ss.output_tokens,
            ss.thinking_tokens
        FROM session_stats ss
        INNER JOIN sessions s ON ss.session_id = s.session_id
        WHERE s.started_at >= datetime('now', '-' || ? || ' hours')
            AND ss.model_name IS NOT NULL
        "#,
    )
    .bind(hours)
    .fetch_all(pool)
    .await?;

    let total_cost: f64 = stats_rows
        .iter()
        .map(|s| {
            let model: Option<String> = s.try_get("model_name").ok();
            let input: Option<i64> = s.try_get("input_tokens").ok().flatten();
            let output: Option<i64> = s.try_get("output_tokens").ok().flatten();
            let thinking: Option<i64> = s.try_get("thinking_tokens").ok().flatten();

            stats::calculate_cost(
                input.unwrap_or(0),
                output.unwrap_or(0),
                thinking.unwrap_or(0),
                model.as_deref().unwrap_or(""),
            )
        })
        .sum();

    Ok(OverviewStats {
        total_sessions: row.try_get("total_sessions").unwrap_or(0),
        total_input_tokens: row.try_get("total_input_tokens").unwrap_or(0),
        total_output_tokens: row.try_get("total_output_tokens").unwrap_or(0),
        total_thinking_tokens: row.try_get("total_thinking_tokens").unwrap_or(0),
        total_tokens: row.try_get("total_tokens").unwrap_or(0),
        avg_duration_ms: row.try_get("avg_duration_ms").unwrap_or(0.0),
        success_rate: row.try_get("success_rate").unwrap_or(100.0),
        total_cost,
    })
}

/// Get token time series
pub async fn get_token_time_series(pool: &SqlitePool, days: i64) -> Result<Vec<TokenTimeSeries>> {
    let rows = sqlx::query(
        r#"
        SELECT
            DATE(s.started_at) as date,
            COALESCE(SUM(ss.input_tokens), 0) as input_tokens,
            COALESCE(SUM(ss.output_tokens), 0) as output_tokens,
            COALESCE(SUM(ss.thinking_tokens), 0) as thinking_tokens
        FROM sessions s
        LEFT JOIN session_stats ss ON s.session_id = ss.session_id
        WHERE s.started_at >= datetime('now', '-' || ? || ' days')
        GROUP BY DATE(s.started_at)
        ORDER BY date ASC
        "#,
    )
    .bind(days)
    .fetch_all(pool)
    .await?;

    let series = rows
        .into_iter()
        .map(|row| TokenTimeSeries {
            date: row.try_get("date").unwrap_or_default(),
            input_tokens: row.try_get("input_tokens").unwrap_or(0),
            output_tokens: row.try_get("output_tokens").unwrap_or(0),
            thinking_tokens: row.try_get("thinking_tokens").unwrap_or(0),
        })
        .collect();

    Ok(series)
}

/// Get tool usage statistics
pub async fn get_tool_usage(pool: &SqlitePool) -> Result<Vec<ToolUsage>> {
    let rows = sqlx::query(
        r#"
        SELECT
            tool_name,
            SUM(call_count) as total_calls,
            SUM(error_count) as failure_count,
            SUM(call_count - error_count) as success_count,
            ROUND(100.0 * SUM(call_count - error_count) / NULLIF(SUM(call_count), 0), 1) as success_rate,
            AVG(avg_execution_time_ms) as avg_time,
            MIN(avg_execution_time_ms) as min_time,
            MAX(avg_execution_time_ms) as max_time
        FROM tool_calls
        GROUP BY tool_name
        ORDER BY total_calls DESC
        LIMIT 20
        "#,
    )
    .fetch_all(pool)
    .await?;

    let tools = rows
        .into_iter()
        .map(|row| ToolUsage {
            tool_name: row.try_get("tool_name").unwrap_or_default(),
            call_count: row.try_get("total_calls").unwrap_or(0),
            avg_time_ms: row.try_get("avg_time").unwrap_or(0.0),
            min_time_ms: row.try_get("min_time").unwrap_or(0.0),
            max_time_ms: row.try_get("max_time").unwrap_or(0.0),
            success_count: row.try_get("success_count").unwrap_or(0),
            failure_count: row.try_get("failure_count").unwrap_or(0),
            success_rate: row.try_get("success_rate").unwrap_or(100.0),
        })
        .collect();

    Ok(tools)
}

/// Get cost statistics
pub async fn get_cost_stats(pool: &SqlitePool) -> Result<CostStats> {
    // Fetch all session_stats with cost-relevant data
    let stats_rows = sqlx::query(
        r#"
        SELECT
            s.started_at,
            ss.model_name,
            ss.input_tokens,
            ss.output_tokens,
            ss.thinking_tokens
        FROM session_stats ss
        INNER JOIN sessions s ON ss.session_id = s.session_id
        WHERE ss.model_name IS NOT NULL
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut today_cost = 0.0;
    let mut week_cost = 0.0;
    let mut month_cost = 0.0;
    let mut total_cost = 0.0;
    let mut daily_costs = vec![0.0; 30]; // Track last 30 days for projection

    for stat in &stats_rows {
        let model: Option<String> = stat.try_get("model_name").ok();
        let input: Option<i64> = stat.try_get("input_tokens").ok().flatten();
        let output: Option<i64> = stat.try_get("output_tokens").ok().flatten();
        let thinking: Option<i64> = stat.try_get("thinking_tokens").ok().flatten();

        let cost = stats::calculate_cost(
            input.unwrap_or(0),
            output.unwrap_or(0),
            thinking.unwrap_or(0),
            model.as_deref().unwrap_or(""),
        );

        total_cost += cost;

        // Parse timestamp and categorize
        if let Ok(Some(started_at)) = stat.try_get::<Option<String>, _>("started_at") {
            // SQLite timestamps are typically in UTC
            // For simplicity, we'll use string comparison
            // In production, you'd want proper timezone handling
            let now = chrono::Utc::now();
            let today_str = now.format("%Y-%m-%d").to_string();
            let week_ago = (now - chrono::Duration::days(7))
                .format("%Y-%m-%d")
                .to_string();
            let month_ago = (now - chrono::Duration::days(30))
                .format("%Y-%m-%d")
                .to_string();

            if started_at.starts_with(&today_str) {
                today_cost += cost;
                daily_costs[0] += cost;
            }

            if started_at.as_str() >= week_ago.as_str() {
                week_cost += cost;
            }

            if started_at.as_str() >= month_ago.as_str() {
                month_cost += cost;

                // Calculate day index for daily tracking
                if let Ok(date) =
                    chrono::NaiveDateTime::parse_from_str(&started_at, "%Y-%m-%d %H:%M:%S")
                {
                    let days_ago = (now.naive_utc().date() - date.date()).num_days();
                    if (0..30).contains(&days_ago) {
                        daily_costs[days_ago as usize] += cost;
                    }
                }
            }
        }
    }

    let projection_monthly = stats::project_monthly_cost(&daily_costs);

    Ok(CostStats {
        today: today_cost,
        this_week: week_cost,
        this_month: month_cost,
        total: total_cost,
        projection_monthly,
    })
}

/// Get model usage statistics
pub async fn get_model_usage(
    pool: &SqlitePool,
    hours: i64,
    user_agent_filter: Option<&str>,
) -> Result<Vec<ModelUsage>> {
    // Build WHERE clause for filters
    // Query session_stats joined with sessions for user_agent filtering
    let mut where_conditions = vec!["ss.model_name IS NOT NULL"];

    let time_filter;
    if hours > 0 {
        time_filter = format!("s.started_at >= datetime('now', '-{} hours')", hours);
        where_conditions.push(&time_filter);
    }

    let use_prefix_match = user_agent_filter
        .map(|ua| ua.ends_with("/*"))
        .unwrap_or(false);

    let ua_filter_str;
    if user_agent_filter.is_some() {
        if use_prefix_match {
            ua_filter_str = "s.user_agent LIKE ?".to_string();
        } else {
            ua_filter_str = "s.user_agent = ?".to_string();
        }
        where_conditions.push(&ua_filter_str);
    }

    let where_clause = where_conditions.join(" AND ");

    // First get total count of requests with filters
    let total_query = format!(
        r#"
        SELECT COUNT(*) as count
        FROM session_stats ss
        INNER JOIN sessions s ON ss.session_id = s.session_id
        WHERE {}
        "#,
        where_clause
    );
    let mut total_sqlx_query = sqlx::query(&total_query);

    if let Some(ua) = user_agent_filter {
        if use_prefix_match {
            let prefix = ua.trim_end_matches("/*");
            total_sqlx_query = total_sqlx_query.bind(format!("{}/%", prefix));
        } else {
            total_sqlx_query = total_sqlx_query.bind(ua);
        }
    }

    let total = total_sqlx_query.fetch_one(pool).await?;
    let total_count = total.try_get::<i64, _>("count").unwrap_or(1).max(1) as f64; // Avoid division by zero

    // Get model usage from session_stats with filters
    let query = format!(
        r#"
        SELECT
            ss.model_name as model,
            COUNT(*) as count
        FROM session_stats ss
        INNER JOIN sessions s ON ss.session_id = s.session_id
        WHERE {}
        GROUP BY ss.model_name
        ORDER BY count DESC
        LIMIT 15
        "#,
        where_clause
    );

    let mut sqlx_query = sqlx::query(&query);

    if let Some(ua) = user_agent_filter {
        if use_prefix_match {
            let prefix = ua.trim_end_matches("/*");
            sqlx_query = sqlx_query.bind(format!("{}/%", prefix));
        } else {
            sqlx_query = sqlx_query.bind(ua);
        }
    }

    let rows = sqlx_query.fetch_all(pool).await?;

    let models = rows
        .into_iter()
        .map(|row| {
            let count = row.try_get::<i64, _>("count").unwrap_or(0);
            let percentage = (count as f64 / total_count) * 100.0;
            ModelUsage {
                model_name: row
                    .try_get::<Option<String>, _>("model")
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| "unknown".to_string()),
                request_count: count,
                percentage,
            }
        })
        .collect();

    Ok(models)
}

/// Get recent sessions
pub async fn get_recent_sessions(pool: &SqlitePool, limit: i64) -> Result<Vec<SessionSummary>> {
    let sessions = sqlx::query(
        r#"
        SELECT
            s.session_id,
            s.started_at,
            s.model_used,
            s.input_tokens,
            s.output_tokens,
            s.thinking_tokens,
            s.total_tokens,
            s.total_duration_ms,
            s.success,
            COUNT(DISTINCT ss.id) as request_count,
            GROUP_CONCAT(DISTINCT ss.model_name) as all_models
        FROM sessions s
        LEFT JOIN session_stats ss ON s.session_id = ss.session_id
        GROUP BY s.session_id
        ORDER BY s.started_at DESC
        LIMIT ?
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let summaries = sessions
        .into_iter()
        .map(|s| {
            // Get all models from session_stats, fallback to model_used from sessions table
            let all_models: Option<String> = s.try_get("all_models").ok().flatten();
            let model_used: Option<String> = s.try_get("model_used").ok().flatten();
            let models = all_models.filter(|m| !m.is_empty()).or(model_used);

            let input: Option<i64> = s.try_get("input_tokens").ok().flatten();
            let output: Option<i64> = s.try_get("output_tokens").ok().flatten();
            let thinking: Option<i64> = s.try_get("thinking_tokens").ok().flatten();

            let cost = stats::calculate_cost(
                input.unwrap_or(0),
                output.unwrap_or(0),
                thinking.unwrap_or(0),
                models.as_deref().unwrap_or(""),
            );

            SessionSummary {
                session_id: s.try_get("session_id").unwrap_or_default(),
                started_at: s
                    .try_get::<Option<String>, _>("started_at")
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| "unknown".to_string()),
                model: models.unwrap_or_else(|| "unknown".to_string()),
                request_count: s.try_get("request_count").unwrap_or(0),
                total_tokens: s.try_get("total_tokens").ok().flatten().unwrap_or(0),
                cost,
                duration_ms: s.try_get("total_duration_ms").ok().flatten().unwrap_or(0),
                success: s.try_get("success").ok().flatten().unwrap_or(false),
            }
        })
        .collect();

    Ok(summaries)
}

/// Get sessions with pagination and filtering
pub async fn get_sessions_paginated(
    pool: &SqlitePool,
    offset: i64,
    limit: i64,
    user_agent_filter: Option<&str>,
    model_filter: Option<&str>,
) -> Result<Vec<SessionSummary>> {
    let mut query = String::from(
        r#"
        SELECT
            s.session_id,
            s.started_at,
            s.model_used,
            s.input_tokens,
            s.output_tokens,
            s.thinking_tokens,
            s.total_tokens,
            s.total_duration_ms,
            s.success,
            COUNT(DISTINCT ss.id) as request_count,
            GROUP_CONCAT(DISTINCT ss.model_name) as all_models
        FROM sessions s
        LEFT JOIN session_stats ss ON s.session_id = ss.session_id
        "#,
    );

    let mut conditions = Vec::new();
    let mut use_prefix_match = false;

    if let Some(ua) = user_agent_filter {
        if ua.ends_with("/*") {
            // Prefix match for "All [agent]" selections
            conditions.push("s.user_agent LIKE ?");
            use_prefix_match = true;
        } else {
            // Exact match for specific versions
            conditions.push("s.user_agent = ?");
        }
    }

    if model_filter.is_some() {
        conditions.push(
            "s.session_id IN (SELECT DISTINCT session_id FROM session_stats WHERE model_name = ?)",
        );
    }

    if !conditions.is_empty() {
        query.push_str(" WHERE ");
        query.push_str(&conditions.join(" AND "));
    }

    query.push_str(
        r#"
        GROUP BY s.session_id
        ORDER BY s.started_at DESC
        LIMIT ? OFFSET ?
        "#,
    );

    let mut sqlx_query = sqlx::query(&query);

    if let Some(ua) = user_agent_filter {
        if use_prefix_match {
            // Convert "prefix/*" to "prefix/%" for LIKE query
            let prefix = ua.trim_end_matches("/*");
            sqlx_query = sqlx_query.bind(format!("{}/%", prefix));
        } else {
            sqlx_query = sqlx_query.bind(ua);
        }
    }

    if let Some(model) = model_filter {
        sqlx_query = sqlx_query.bind(model);
    }

    let sessions = sqlx_query.bind(limit).bind(offset).fetch_all(pool).await?;

    let summaries = sessions
        .into_iter()
        .map(|s| {
            // Get all models from session_stats, fallback to model_used from sessions table
            let all_models: Option<String> = s.try_get("all_models").ok().flatten();
            let model_used: Option<String> = s.try_get("model_used").ok().flatten();
            let models = all_models.filter(|m| !m.is_empty()).or(model_used);

            let input: Option<i64> = s.try_get("input_tokens").ok().flatten();
            let output: Option<i64> = s.try_get("output_tokens").ok().flatten();
            let thinking: Option<i64> = s.try_get("thinking_tokens").ok().flatten();

            let cost = stats::calculate_cost(
                input.unwrap_or(0),
                output.unwrap_or(0),
                thinking.unwrap_or(0),
                models.as_deref().unwrap_or(""),
            );

            SessionSummary {
                session_id: s.try_get("session_id").unwrap_or_default(),
                started_at: s
                    .try_get::<Option<String>, _>("started_at")
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| "unknown".to_string()),
                model: models.unwrap_or_else(|| "unknown".to_string()),
                request_count: s.try_get("request_count").unwrap_or(0),
                total_tokens: s.try_get("total_tokens").ok().flatten().unwrap_or(0),
                cost,
                duration_ms: s.try_get("total_duration_ms").ok().flatten().unwrap_or(0),
                success: s.try_get("success").ok().flatten().unwrap_or(false),
            }
        })
        .collect();

    Ok(summaries)
}

/// Get distinct user agents from sessions
pub async fn get_user_agents(pool: &SqlitePool) -> Result<Vec<String>> {
    let rows = sqlx::query(
        r#"
        SELECT DISTINCT user_agent
        FROM sessions
        WHERE user_agent IS NOT NULL
        ORDER BY user_agent ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    let user_agents = rows
        .into_iter()
        .filter_map(|row| {
            row.try_get::<Option<String>, _>("user_agent")
                .ok()
                .flatten()
        })
        .collect();

    Ok(user_agents)
}

/// Get distinct models from session stats
pub async fn get_models(pool: &SqlitePool) -> Result<Vec<String>> {
    let rows = sqlx::query(
        r#"
        SELECT DISTINCT model_name
        FROM session_stats
        WHERE model_name IS NOT NULL
        ORDER BY model_name ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    let models = rows
        .into_iter()
        .filter_map(|row| {
            row.try_get::<Option<String>, _>("model_name")
                .ok()
                .flatten()
        })
        .collect();

    Ok(models)
}

/// Get detailed session information
pub async fn get_session_detail(pool: &SqlitePool, session_id: &str) -> Result<SessionDetail> {
    let session = sqlx::query(
        r#"
        SELECT
            s.session_id,
            s.request_id,
            s.started_at,
            s.completed_at,
            s.provider,
            s.listener,
            s.model_requested,
            s.model_used,
            s.success,
            s.error_message,
            s.finish_reason,
            s.total_duration_ms,
            s.provider_latency_ms,
            s.time_to_first_token_ms,
            s.input_tokens,
            s.output_tokens,
            s.thinking_tokens,
            s.reasoning_tokens,
            s.cache_read_tokens,
            s.cache_creation_tokens,
            s.total_tokens,
            s.is_streaming,
            s.chunk_count,
            s.streaming_duration_ms,
            s.client_ip,
            s.user_agent,
            COUNT(DISTINCT ss.id) as request_count,
            GROUP_CONCAT(DISTINCT ss.model_name) as all_models
        FROM sessions s
        LEFT JOIN session_stats ss ON s.session_id = ss.session_id
        WHERE s.session_id = ?
        GROUP BY s.session_id
        "#,
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?;

    if let Some(s) = session {
        // Get all models from session_stats, fallback to model_used from sessions table
        let all_models: Option<String> = s.try_get("all_models").ok().flatten();
        let model_used: Option<String> = s.try_get("model_used").ok().flatten();
        let model: Option<String> = all_models.filter(|m| !m.is_empty()).or(model_used);
        let input: i64 = s.try_get("input_tokens").ok().flatten().unwrap_or(0);
        let output: i64 = s.try_get("output_tokens").ok().flatten().unwrap_or(0);
        let thinking: i64 = s.try_get("thinking_tokens").ok().flatten().unwrap_or(0);

        let cost = stats::calculate_cost(input, output, thinking, model.as_deref().unwrap_or(""));

        Ok(SessionDetail {
            session_id: s.try_get("session_id").unwrap_or_default(),
            request_id: s.try_get("request_id").ok().flatten(),
            started_at: s
                .try_get::<Option<String>, _>("started_at")
                .ok()
                .flatten()
                .unwrap_or_default(),
            completed_at: s.try_get("completed_at").ok().flatten(),
            provider: s.try_get("provider").unwrap_or_default(),
            listener: s.try_get("listener").unwrap_or_default(),
            model_requested: s.try_get("model_requested").unwrap_or_default(),
            model_used: model.clone(),
            success: s.try_get("success").ok().flatten().unwrap_or(false),
            error_message: s.try_get("error_message").ok().flatten(),
            finish_reason: s.try_get("finish_reason").ok().flatten(),
            total_duration_ms: s.try_get("total_duration_ms").ok().flatten(),
            provider_latency_ms: s.try_get("provider_latency_ms").ok().flatten(),
            time_to_first_token_ms: s.try_get("time_to_first_token_ms").ok().flatten(),
            input_tokens: input,
            output_tokens: output,
            thinking_tokens: thinking,
            reasoning_tokens: s.try_get("reasoning_tokens").ok().flatten().unwrap_or(0),
            cache_read_tokens: s.try_get("cache_read_tokens").ok().flatten().unwrap_or(0),
            cache_creation_tokens: s
                .try_get("cache_creation_tokens")
                .ok()
                .flatten()
                .unwrap_or(0),
            total_tokens: s.try_get("total_tokens").ok().flatten().unwrap_or(0),
            is_streaming: s.try_get("is_streaming").ok().flatten().unwrap_or(false),
            chunk_count: s.try_get("chunk_count").ok().flatten(),
            streaming_duration_ms: s.try_get("streaming_duration_ms").ok().flatten(),
            client_ip: s.try_get("client_ip").ok().flatten(),
            user_agent: s.try_get("user_agent").ok().flatten(),
            cost,
            request_count: s.try_get("request_count").unwrap_or(0),
        })
    } else {
        Err(anyhow::anyhow!("Session not found"))
    }
}

/// Get session timeline events with pagination
pub async fn get_session_timeline(
    pool: &SqlitePool,
    session_id: &str,
    offset: i64,
    limit: i64,
    order: &str,
) -> Result<Vec<TimelineEvent>> {
    // Validate order parameter to prevent SQL injection
    let order_clause = if order.to_uppercase() == "DESC" {
        "DESC"
    } else {
        "ASC"
    };

    // Get session stats entries for this session (each represents a request/response)
    let stats_query = format!(
        r#"
        SELECT
            created_at,
            request_id,
            model_name,
            input_tokens,
            output_tokens,
            thinking_tokens,
            pre_processing_ms,
            post_processing_ms,
            proxy_overhead_ms,
            tokens_per_second
        FROM session_stats
        WHERE session_id = ?
        ORDER BY created_at {}
        LIMIT ? OFFSET ?
        "#,
        order_clause
    );

    let stats = sqlx::query(&stats_query)
        .bind(session_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;

    let mut events = Vec::new();

    for (idx, stat) in stats.iter().enumerate() {
        let created_at: Option<String> = stat.try_get("created_at").ok().flatten();
        let request_id: Option<String> = stat.try_get("request_id").ok().flatten();
        let model_name: Option<String> = stat.try_get("model_name").ok().flatten();
        let input_tokens: Option<i64> = stat.try_get("input_tokens").ok().flatten();
        let output_tokens: Option<i64> = stat.try_get("output_tokens").ok().flatten();

        events.push(TimelineEvent {
            timestamp: created_at.unwrap_or_default(),
            event_type: "request".to_string(),
            description: format!(
                "Request #{}: {} - {} input, {} output tokens",
                offset + idx as i64 + 1,
                model_name.as_deref().unwrap_or("unknown"),
                input_tokens.unwrap_or(0),
                output_tokens.unwrap_or(0)
            ),
            metadata: serde_json::json!({
                "model": model_name,
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
                "thinking_tokens": stat.try_get::<Option<i64>, _>("thinking_tokens").ok().flatten(),
                "pre_processing_ms": stat.try_get::<Option<f64>, _>("pre_processing_ms").ok().flatten(),
                "post_processing_ms": stat.try_get::<Option<f64>, _>("post_processing_ms").ok().flatten(),
                "proxy_overhead_ms": stat.try_get::<Option<f64>, _>("proxy_overhead_ms").ok().flatten(),
                "tokens_per_second": stat.try_get::<Option<f64>, _>("tokens_per_second").ok().flatten(),
            }).into(),
            request_id,
        });
    }

    // Get tool calls for this session (within the same pagination window)
    let tools_query = format!(
        r#"
        SELECT
            created_at,
            tool_name,
            call_count,
            avg_execution_time_ms,
            error_count
        FROM tool_calls
        WHERE session_id = ?
        ORDER BY created_at {}
        LIMIT ? OFFSET ?
        "#,
        order_clause
    );

    let tools = sqlx::query(&tools_query)
        .bind(session_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;

    for tool in tools {
        let created_at: Option<String> = tool.try_get("created_at").ok().flatten();
        let tool_name: Option<String> = tool.try_get("tool_name").ok().flatten();
        let call_count: Option<i64> = tool.try_get("call_count").ok().flatten();

        events.push(TimelineEvent {
            timestamp: created_at.unwrap_or_default(),
            event_type: "tool_call".to_string(),
            description: format!(
                "Tool: {} (called {} times)",
                tool_name.as_deref().unwrap_or("unknown"),
                call_count.unwrap_or(0)
            ),
            metadata: serde_json::json!({
                "tool_name": tool_name,
                "call_count": call_count,
                "avg_execution_time_ms": tool.try_get::<Option<i64>, _>("avg_execution_time_ms").ok().flatten(),
                "error_count": tool.try_get::<Option<i64>, _>("error_count").ok().flatten(),
            }).into(),
            request_id: None,
        });
    }

    // Sort events by timestamp (since we're combining stats and tools)
    events.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

    Ok(events)
}

/// Get sessions by hour of day
pub async fn get_sessions_by_hour(pool: &SqlitePool, hours: i64) -> Result<Vec<HourOfDayStats>> {
    let rows = sqlx::query(
        r#"
        SELECT
            CAST(strftime('%H', started_at) AS INTEGER) as hour,
            COUNT(*) as session_count
        FROM sessions
        WHERE started_at >= datetime('now', '-' || ? || ' hours')
        GROUP BY hour
        ORDER BY hour ASC
        "#,
    )
    .bind(hours)
    .fetch_all(pool)
    .await?;

    let stats = rows
        .into_iter()
        .map(|row| HourOfDayStats {
            hour: row.try_get("hour").unwrap_or(0),
            session_count: row.try_get("session_count").unwrap_or(0),
        })
        .collect();

    Ok(stats)
}

/// Get spending statistics with breakdown by model
pub async fn get_spending_stats(pool: &SqlitePool, hours: i64) -> Result<SpendingStats> {
    // Fetch all session_stats with cost-relevant data for the time period
    let stats_rows = sqlx::query(
        r#"
        SELECT
            s.session_id,
            ss.model_name,
            ss.input_tokens,
            ss.output_tokens,
            ss.thinking_tokens
        FROM session_stats ss
        INNER JOIN sessions s ON ss.session_id = s.session_id
        WHERE s.started_at >= datetime('now', '-' || ? || ' hours')
            AND ss.model_name IS NOT NULL
        "#,
    )
    .bind(hours)
    .fetch_all(pool)
    .await?;

    // Calculate total cost and per-model breakdown
    let mut total_cost = 0.0;
    let mut model_costs: std::collections::HashMap<
        String,
        (f64, std::collections::HashSet<String>),
    > = std::collections::HashMap::new();

    for stat in &stats_rows {
        let model: Option<String> = stat.try_get("model_name").ok();
        let input: Option<i64> = stat.try_get("input_tokens").ok().flatten();
        let output: Option<i64> = stat.try_get("output_tokens").ok().flatten();
        let thinking: Option<i64> = stat.try_get("thinking_tokens").ok().flatten();
        let session_id: Option<String> = stat.try_get("session_id").ok();

        let cost = stats::calculate_cost(
            input.unwrap_or(0),
            output.unwrap_or(0),
            thinking.unwrap_or(0),
            model.as_deref().unwrap_or(""),
        );

        total_cost += cost;

        if let Some(model_name) = model {
            let entry = model_costs
                .entry(model_name)
                .or_insert((0.0, std::collections::HashSet::new()));
            entry.0 += cost;
            if let Some(sid) = session_id {
                entry.1.insert(sid);
            }
        }
    }

    // Get total unique sessions count
    let session_count_row = sqlx::query(
        r#"
        SELECT COUNT(DISTINCT session_id) as count
        FROM sessions
        WHERE started_at >= datetime('now', '-' || ? || ' hours')
        "#,
    )
    .bind(hours)
    .fetch_one(pool)
    .await?;

    let total_sessions: i64 = session_count_row.try_get("count").unwrap_or(1).max(1);
    let avg_cost_per_session = total_cost / total_sessions as f64;

    // Build per-model spending stats
    let mut by_model: Vec<ModelSpending> = model_costs
        .into_iter()
        .map(|(model_name, (cost, sessions))| {
            let session_count = sessions.len() as i64;
            ModelSpending {
                model_name,
                total_cost: cost,
                session_count,
                avg_cost_per_session: cost / session_count.max(1) as f64,
            }
        })
        .collect();

    // Sort by total cost descending
    by_model.sort_by(|a, b| b.total_cost.partial_cmp(&a.total_cost).unwrap());

    Ok(SpendingStats {
        total_cost,
        avg_cost_per_session,
        by_model,
    })
}

/// Get tool usage statistics for a specific session
pub async fn get_session_tool_stats(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<SessionToolStats> {
    let rows = sqlx::query(
        r#"
        SELECT
            tool_name,
            SUM(call_count) as total_calls
        FROM tool_calls
        WHERE session_id = ?
        GROUP BY tool_name
        ORDER BY total_calls DESC
        "#,
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;

    let total_tool_calls: i64 = rows
        .iter()
        .map(|row| row.try_get::<i64, _>("total_calls").unwrap_or(0))
        .sum();

    let by_tool: Vec<ToolBreakdown> = rows
        .into_iter()
        .map(|row| {
            let call_count: i64 = row.try_get("total_calls").unwrap_or(0);
            let percentage = if total_tool_calls > 0 {
                (call_count as f64 / total_tool_calls as f64) * 100.0
            } else {
                0.0
            };

            ToolBreakdown {
                tool_name: row.try_get("tool_name").unwrap_or_default(),
                call_count,
                percentage,
            }
        })
        .collect();

    // Calculate tool usage percentage of session (assuming sessions can have 0 to many tool calls)
    // This is the percentage of requests that used tools
    let session_stats_count = sqlx::query(
        r#"
        SELECT COUNT(*) as count
        FROM session_stats
        WHERE session_id = ?
        "#,
    )
    .bind(session_id)
    .fetch_one(pool)
    .await?;

    let request_count: i64 = session_stats_count.try_get("count").unwrap_or(1).max(1);

    // Tool usage percentage: what % of requests had tool calls
    let tool_usage_percentage = if request_count > 0 {
        (total_tool_calls as f64 / request_count as f64) * 100.0
    } else {
        0.0
    };

    Ok(SessionToolStats {
        total_tool_calls,
        tool_usage_percentage,
        by_tool,
    })
}
