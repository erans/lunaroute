#!/usr/bin/env rust-script
//! Backfill SQLite database from JSONL session logs
//!
//! ```cargo
//! [dependencies]
//! tokio = { version = "1", features = ["full"] }
//! serde_json = "1"
//! anyhow = "1"
//! glob = "0.3"
//! ```

use std::fs::File;
use std::io::{BufRead, BufReader};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let sessions_dir = shellexpand::tilde("~/.lunaroute/sessions");
    let db_path = shellexpand::tilde("~/.lunaroute/sessions.db");

    println!("📦 LunaRoute SQLite Backfill");
    println!("============================");
    println!("Sessions dir: {}", sessions_dir);
    println!("Database: {}", db_path);
    println!();

    // Find all JSONL files
    let pattern = format!("{}/**/*.jsonl", sessions_dir);
    let files: Vec<_> = glob::glob(&pattern)?
        .filter_map(Result::ok)
        .collect();

    println!("Found {} JSONL files", files.len());

    if files.is_empty() {
        println!("No files to process!");
        return Ok(());
    }

    // Open SQLite connection
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}", db_path))
        .await?;

    println!("Connected to SQLite database\n");

    let mut total_events = 0;
    let mut sessions_processed = 0;

    for (idx, file_path) in files.iter().enumerate() {
        let file_name = file_path.file_name().unwrap().to_string_lossy();

        if (idx + 1) % 10 == 0 || idx == 0 {
            println!("Processing file {}/{}: {}", idx + 1, files.len(), file_name);
        }

        let file = File::open(file_path)?;
        let reader = BufReader::new(file);

        let mut events_in_file = 0;

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let event: serde_json::Value = serde_json::from_str(&line)?;

            // Process the event based on its type
            if let Some(event_type) = event.get("type").and_then(|v| v.as_str()) {
                match event_type {
                    "started" => {
                        process_started_event(&pool, &event).await?;
                    }
                    "stats_updated" => {
                        process_stats_event(&pool, &event).await?;
                    }
                    "request_recorded" => {
                        process_request_event(&pool, &event).await?;
                    }
                    "completed" => {
                        process_completed_event(&pool, &event).await?;
                    }
                    "tool_call_executed" => {
                        process_tool_call_event(&pool, &event).await?;
                    }
                    "stream_metrics_recorded" => {
                        process_stream_metrics_event(&pool, &event).await?;
                    }
                    _ => {
                        // Unknown event type, skip
                    }
                }
                events_in_file += 1;
                total_events += 1;
            }
        }

        if events_in_file > 0 {
            sessions_processed += 1;
        }
    }

    println!("\n✅ Backfill complete!");
    println!("   Sessions processed: {}", sessions_processed);
    println!("   Total events: {}", total_events);

    // Show database stats
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sessions")
        .fetch_one(&pool)
        .await?;

    println!("   Sessions in DB: {}", count.0);

    Ok(())
}

async fn process_started_event(
    pool: &sqlx::SqlitePool,
    event: &serde_json::Value,
) -> anyhow::Result<()> {
    let session_id = event["session_id"].as_str().unwrap_or("");
    let request_id = event["request_id"].as_str().unwrap_or("");
    let timestamp = event["timestamp"].as_str().unwrap_or("");
    let model_requested = event["model_requested"].as_str().unwrap_or("");
    let provider = event["provider"].as_str().unwrap_or("");
    let listener = event["listener"].as_str().unwrap_or("");
    let is_streaming = event["is_streaming"].as_bool().unwrap_or(false);
    let user_agent = event.get("user_agent").and_then(|v| v.as_str());
    let client_ip = event.get("client_ip").and_then(|v| v.as_str());

    // Insert or ignore (in case session already exists)
    sqlx::query(
        r#"
        INSERT OR IGNORE INTO sessions (
            session_id, request_id, started_at, provider, listener,
            model_requested, is_streaming, user_agent, client_ip
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(session_id)
    .bind(request_id)
    .bind(timestamp)
    .bind(provider)
    .bind(listener)
    .bind(model_requested)
    .bind(is_streaming)
    .bind(user_agent)
    .bind(client_ip)
    .execute(pool)
    .await?;

    Ok(())
}

async fn process_stats_event(
    pool: &sqlx::SqlitePool,
    event: &serde_json::Value,
) -> anyhow::Result<()> {
    let session_id = event["session_id"].as_str().unwrap_or("");
    let model_used = event.get("model_used").and_then(|v| v.as_str());

    // Extract token updates
    let token_updates = &event["token_updates"];
    let input_tokens = token_updates.get("total_input").and_then(|v| v.as_i64()).unwrap_or(0);
    let output_tokens = token_updates.get("total_output").and_then(|v| v.as_i64()).unwrap_or(0);
    let thinking_tokens = token_updates.get("total_thinking").and_then(|v| v.as_i64()).unwrap_or(0);
    let cache_read_tokens = token_updates.get("total_cache_read").and_then(|v| v.as_i64()).unwrap_or(0);
    let cache_creation_tokens = token_updates.get("total_cache_creation").and_then(|v| v.as_i64()).unwrap_or(0);

    // Update session with token counts
    sqlx::query(
        r#"
        UPDATE sessions
        SET model_used = COALESCE(?, model_used),
            input_tokens = ?,
            output_tokens = ?,
            thinking_tokens = ?,
            cache_read_tokens = ?,
            cache_creation_tokens = ?
        WHERE session_id = ?
        "#,
    )
    .bind(model_used)
    .bind(input_tokens)
    .bind(output_tokens)
    .bind(thinking_tokens)
    .bind(cache_read_tokens)
    .bind(cache_creation_tokens)
    .bind(session_id)
    .execute(pool)
    .await?;

    Ok(())
}

async fn process_request_event(
    pool: &sqlx::SqlitePool,
    event: &serde_json::Value,
) -> anyhow::Result<()> {
    let session_id = event["session_id"].as_str().unwrap_or("");
    let request_text = event.get("request_text").and_then(|v| v.as_str());

    // Update session with request text
    sqlx::query(
        r#"
        UPDATE sessions
        SET request_text = ?
        WHERE session_id = ?
        "#,
    )
    .bind(request_text)
    .bind(session_id)
    .execute(pool)
    .await?;

    Ok(())
}

async fn process_completed_event(
    pool: &sqlx::SqlitePool,
    event: &serde_json::Value,
) -> anyhow::Result<()> {
    let session_id = event["session_id"].as_str().unwrap_or("");
    let timestamp = event.get("timestamp").and_then(|v| v.as_str());
    let success = event.get("success").and_then(|v| v.as_bool());
    let error_message = event.get("error_message").and_then(|v| v.as_str());
    let finish_reason = event.get("finish_reason").and_then(|v| v.as_str());
    let total_duration_ms = event.get("total_duration_ms").and_then(|v| v.as_i64());
    let provider_latency_ms = event.get("provider_latency_ms").and_then(|v| v.as_i64());
    let response_text = event.get("response_text").and_then(|v| v.as_str());

    // Update session as completed
    sqlx::query(
        r#"
        UPDATE sessions
        SET completed_at = ?,
            success = ?,
            error_message = ?,
            finish_reason = ?,
            total_duration_ms = ?,
            provider_latency_ms = ?,
            response_text = ?
        WHERE session_id = ?
        "#,
    )
    .bind(timestamp)
    .bind(success)
    .bind(error_message)
    .bind(finish_reason)
    .bind(total_duration_ms)
    .bind(provider_latency_ms)
    .bind(response_text)
    .bind(session_id)
    .execute(pool)
    .await?;

    Ok(())
}

async fn process_tool_call_event(
    pool: &sqlx::SqlitePool,
    event: &serde_json::Value,
) -> anyhow::Result<()> {
    let session_id = event["session_id"].as_str().unwrap_or("");
    let request_id = event["request_id"].as_str().unwrap_or("");
    let tool_call_id = event.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("");
    let tool_name = event.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
    let execution_time_ms = event.get("execution_time_ms").and_then(|v| v.as_i64());
    let success = event.get("success").and_then(|v| v.as_bool());

    // Insert tool call execution
    sqlx::query(
        r#"
        INSERT OR IGNORE INTO tool_call_executions (
            session_id, request_id, tool_call_id, tool_name,
            execution_time_ms, success
        ) VALUES (?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(session_id)
    .bind(request_id)
    .bind(tool_call_id)
    .bind(tool_name)
    .bind(execution_time_ms)
    .bind(success)
    .execute(pool)
    .await?;

    Ok(())
}

async fn process_stream_metrics_event(
    pool: &sqlx::SqlitePool,
    event: &serde_json::Value,
) -> anyhow::Result<()> {
    let session_id = event["session_id"].as_str().unwrap_or("");
    let request_id = event["request_id"].as_str().unwrap_or("");
    let time_to_first_token_ms = event.get("time_to_first_token_ms").and_then(|v| v.as_i64());
    let streaming_duration_ms = event.get("streaming_duration_ms").and_then(|v| v.as_i64());
    let chunk_count = event.get("chunk_count").and_then(|v| v.as_i64());

    // Update session with streaming metrics
    sqlx::query(
        r#"
        UPDATE sessions
        SET time_to_first_token_ms = ?,
            streaming_duration_ms = ?,
            chunk_count = ?
        WHERE session_id = ? AND request_id = ?
        "#,
    )
    .bind(time_to_first_token_ms)
    .bind(streaming_duration_ms)
    .bind(chunk_count)
    .bind(session_id)
    .bind(request_id)
    .execute(pool)
    .await?;

    // Insert into stream_metrics table if we have all required fields
    if let (Some(ttft), Some(duration), Some(chunks)) =
        (time_to_first_token_ms, streaming_duration_ms, chunk_count) {
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO stream_metrics (
                session_id, request_id, time_to_first_token_ms,
                total_chunks, streaming_duration_ms
            ) VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(session_id)
        .bind(request_id)
        .bind(ttft)
        .bind(chunks)
        .bind(duration)
        .execute(pool)
        .await?;
    }

    Ok(())
}
