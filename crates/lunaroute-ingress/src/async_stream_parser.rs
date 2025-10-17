//! Async stream parsing for passthrough mode
//!
//! This module provides non-blocking SSE stream parsing to extract tokens and tool calls
//! from provider responses without impacting client latency.
//!
//! ## Key Features
//!
//! - **Zero-latency**: Client receives responses immediately, parsing happens in background
//! - **Deduplication**: Uses tool IDs to prevent over-counting tool calls
//! - **Panic recovery**: Background tasks catch and log panics without affecting client
//! - **Memory bounded**: Respects event collection limits to prevent OOM
//!
//! ## Design
//!
//! In passthrough mode, we maintain 100% API fidelity by forwarding responses without
//! modification. To still capture metrics, we:
//!
//! 1. Collect SSE events during streaming (up to MAX_COLLECTED_EVENTS)
//! 2. Spawn background task after client receives full response
//! 3. Parse events asynchronously to extract tokens and tool calls
//! 4. Emit StatsUpdated event to update database records

use futures::StreamExt;
use lunaroute_core::session_store::SessionStore;
use lunaroute_session::events::{TokenTotals, ToolStats, ToolUsageSummary};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Information about a tool call extracted from stream
#[derive(Debug, Clone)]
pub struct ToolCallInfo {
    pub tool_name: String,
    pub tool_call_id: String,
    pub tool_arguments: String,
}

/// Result of async stream parsing
#[derive(Debug, Clone, Default)]
pub struct ParsedStreamData {
    pub tokens: TokenTotals,
    pub tool_summary: ToolUsageSummary,
    pub model_used: Option<String>,
    pub response_size_bytes: usize,
    pub content_blocks: usize,
    pub has_refusal: bool,
    pub tool_calls: Vec<ToolCallInfo>,
}

/// Parse Anthropic SSE stream to extract tokens and tool calls
///
/// This runs asynchronously without blocking the client stream.
///
/// **Deduplication**: Uses tool IDs from `content_block.id` to prevent counting
/// the same tool call multiple times if it appears in multiple SSE events.
pub async fn parse_anthropic_stream<S, E>(mut stream: S) -> ParsedStreamData
where
    S: futures::Stream<
            Item = Result<eventsource_stream::Event, eventsource_stream::EventStreamError<E>>,
        > + Unpin,
{
    let mut data = ParsedStreamData::default();
    let mut tool_calls: HashMap<String, u32> = HashMap::new();
    let mut seen_tool_ids: HashSet<String> = HashSet::new(); // Prevent duplicate counting
    let mut seen_content_block_ids: HashSet<String> = HashSet::new(); // Track unique content blocks

    // Track tool arguments by content block index
    let mut tool_id_by_index: HashMap<u32, String> = HashMap::new();
    let mut tool_name_by_id: HashMap<String, String> = HashMap::new();
    let mut tool_args_by_id: HashMap<String, String> = HashMap::new();

    while let Some(event_result) = stream.next().await {
        if let Ok(event) = event_result {
            // Track response size
            data.response_size_bytes += event.data.len();

            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&event.data) {
                // Extract tokens from message_start
                if let Some("message_start") = json.get("type").and_then(|t| t.as_str())
                    && let Some(message) = json.get("message")
                {
                    // Input tokens
                    if let Some(usage) = message.get("usage")
                        && let Some(input) = usage.get("input_tokens").and_then(|t| t.as_u64())
                    {
                        data.tokens.total_input = input;
                    }
                    // Model
                    if let Some(model) = message.get("model").and_then(|m| m.as_str()) {
                        data.model_used = Some(model.to_string());
                    }
                }

                // Extract output tokens from message_delta
                if let Some("message_delta") = json.get("type").and_then(|t| t.as_str()) {
                    if let Some(usage) = json.get("usage")
                        && let Some(output) = usage.get("output_tokens").and_then(|t| t.as_u64())
                    {
                        data.tokens.total_output = output;
                    }

                    // Check for refusal in stop_reason
                    if let Some(delta) = json.get("delta")
                        && let Some(stop_reason) = delta.get("stop_reason").and_then(|s| s.as_str())
                        && stop_reason == "end_turn"
                    {
                        // Check for refusal content
                        data.has_refusal = false; // Will be set in content_block if needed
                    }
                }

                // Extract content blocks from content_block_start
                if let Some("content_block_start") = json.get("type").and_then(|t| t.as_str())
                    && let Some(index) = json.get("index").and_then(|i| i.as_u64())
                    && let Some(block) = json.get("content_block")
                {
                    // Count unique content blocks
                    if let Some(block_id) = block.get("id").and_then(|id| id.as_str())
                        && seen_content_block_ids.insert(block_id.to_string())
                    {
                        data.content_blocks += 1;
                    }

                    // Extract tool calls
                    if let Some("tool_use") = block.get("type").and_then(|t| t.as_str())
                        && let Some(name) = block.get("name").and_then(|n| n.as_str())
                    {
                        // Use tool ID if available, otherwise fall back to index-based tracking
                        let tool_id = block
                            .get("id")
                            .and_then(|id| id.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| format!("{}_{}", name, seen_tool_ids.len()));

                        // Track tool ID by index for input_json_delta events
                        tool_id_by_index.insert(index as u32, tool_id.clone());
                        tool_name_by_id.insert(tool_id.clone(), name.to_string());

                        // Only count if we haven't seen this tool ID before
                        if seen_tool_ids.insert(tool_id) {
                            *tool_calls.entry(name.to_string()).or_insert(0) += 1;
                        }
                    }
                }

                // Extract tool arguments from content_block_delta
                if let Some("content_block_delta") = json.get("type").and_then(|t| t.as_str())
                    && let Some(index) = json.get("index").and_then(|i| i.as_u64())
                    && let Some(delta) = json.get("delta")
                    && let Some("input_json_delta") = delta.get("type").and_then(|t| t.as_str())
                    && let Some(partial_json) = delta.get("partial_json").and_then(|p| p.as_str())
                {
                    // Look up tool ID by index
                    if let Some(tool_id) = tool_id_by_index.get(&(index as u32)) {
                        // Append partial JSON to build complete arguments
                        tool_args_by_id
                            .entry(tool_id.clone())
                            .or_default()
                            .push_str(partial_json);
                    }
                }
            }
        }
    }

    // Build tool summary
    if !tool_calls.is_empty() {
        data.tool_summary.total_tool_calls = tool_calls.values().sum();
        data.tool_summary.unique_tool_count = tool_calls.len() as u32;

        for (tool_name, count) in tool_calls {
            data.tool_summary.by_tool.insert(
                tool_name,
                ToolStats {
                    call_count: count,
                    total_execution_time_ms: 0, // Not available from stream
                    avg_execution_time_ms: 0,
                    error_count: 0,
                },
            );
        }
    }

    // Build tool_calls list with arguments
    for (tool_id, tool_name) in tool_name_by_id {
        let tool_arguments = tool_args_by_id.get(&tool_id).cloned().unwrap_or_default();
        data.tool_calls.push(ToolCallInfo {
            tool_name,
            tool_call_id: tool_id,
            tool_arguments,
        });
    }

    // Calculate grand total
    data.tokens.grand_total = data.tokens.total_input + data.tokens.total_output;

    data
}

/// Parse OpenAI SSE stream to extract tokens and tool calls
///
/// This runs asynchronously without blocking the client stream.
///
/// **Deduplication**: Uses tool IDs from `tool_call.id` or `tool_call.index`
/// to prevent counting the same tool call multiple times across delta events.
pub async fn parse_openai_stream<S, E>(mut stream: S) -> ParsedStreamData
where
    S: futures::Stream<
            Item = Result<eventsource_stream::Event, eventsource_stream::EventStreamError<E>>,
        > + Unpin,
{
    let mut data = ParsedStreamData::default();
    let mut tool_calls: HashMap<String, u32> = HashMap::new();
    let mut seen_tool_ids: HashSet<String> = HashSet::new(); // Prevent duplicate counting

    while let Some(event_result) = stream.next().await {
        if let Ok(event) = event_result {
            // Track response size
            data.response_size_bytes += event.data.len();

            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&event.data) {
                // Extract model
                if let Some(model) = json.get("model").and_then(|m| m.as_str()) {
                    data.model_used = Some(model.to_string());
                }

                // Extract tokens from usage (usually in last chunk)
                if let Some(usage) = json.get("usage") {
                    if let Some(input) = usage.get("prompt_tokens").and_then(|t| t.as_u64()) {
                        data.tokens.total_input = input;
                    }
                    if let Some(output) = usage.get("completion_tokens").and_then(|t| t.as_u64()) {
                        data.tokens.total_output = output;
                    }

                    // Extract reasoning tokens from completion_tokens_details (o1/o3/o4 models)
                    if let Some(details) = usage.get("completion_tokens_details")
                        && let Some(reasoning) =
                            details.get("reasoning_tokens").and_then(|t| t.as_u64())
                    {
                        data.tokens.total_thinking = reasoning;
                    }

                    // Extract cached tokens from prompt_tokens_details
                    if let Some(details) = usage.get("prompt_tokens_details")
                        && let Some(cached) = details.get("cached_tokens").and_then(|t| t.as_u64())
                    {
                        data.tokens.total_cached = cached;
                    }
                }

                // Extract tool calls and content from delta
                if let Some(choices) = json.get("choices").and_then(|c| c.as_array()) {
                    for choice in choices {
                        if let Some(delta) = choice.get("delta") {
                            // Track content blocks (OpenAI doesn't have explicit blocks, count non-empty content)
                            if delta.get("content").and_then(|c| c.as_str()).is_some() {
                                data.content_blocks = data.content_blocks.max(1); // At least 1 content block
                            }

                            // Extract tool calls
                            if let Some(tool_calls_arr) =
                                delta.get("tool_calls").and_then(|t| t.as_array())
                            {
                                for tool_call in tool_calls_arr {
                                    if let Some(function) = tool_call.get("function")
                                        && let Some(name) =
                                            function.get("name").and_then(|n| n.as_str())
                                    {
                                        // Use tool call ID if available, otherwise fall back to index-based tracking
                                        let tool_id = tool_call
                                            .get("id")
                                            .and_then(|id| id.as_str())
                                            .map(|s| s.to_string())
                                            .or_else(|| {
                                                tool_call
                                                    .get("index")
                                                    .and_then(|i| i.as_u64())
                                                    .map(|i| format!("index_{}", i))
                                            })
                                            .unwrap_or_else(|| {
                                                format!("{}_{}", name, seen_tool_ids.len())
                                            });

                                        // Only count if we haven't seen this tool ID before
                                        if seen_tool_ids.insert(tool_id) {
                                            *tool_calls.entry(name.to_string()).or_insert(0) += 1;
                                        }
                                    }
                                }
                            }

                            // Check for refusal
                            if let Some(refusal) = delta.get("refusal").and_then(|r| r.as_str())
                                && !refusal.is_empty()
                            {
                                data.has_refusal = true;
                            }
                        }
                    }
                }
            }
        }
    }

    // Build tool summary
    if !tool_calls.is_empty() {
        data.tool_summary.total_tool_calls = tool_calls.values().sum();
        data.tool_summary.unique_tool_count = tool_calls.len() as u32;

        for (tool_name, count) in tool_calls {
            data.tool_summary.by_tool.insert(
                tool_name,
                ToolStats {
                    call_count: count,
                    total_execution_time_ms: 0, // Not available from stream
                    avg_execution_time_ms: 0,
                    error_count: 0,
                },
            );
        }
    }

    // Calculate grand total
    data.tokens.grand_total = data.tokens.total_input + data.tokens.total_output;

    data
}

/// Spawn async parsing task for Anthropic stream
///
/// Returns immediately, parsing happens in background without blocking client response.
///
/// **Panic Safety**: Uses `catch_unwind` to prevent panics from crashing the server.
/// All errors are logged with session ID for debugging.
pub fn spawn_anthropic_parser<E>(
    stream: impl futures::Stream<
        Item = Result<eventsource_stream::Event, eventsource_stream::EventStreamError<E>>,
    > + Send
    + Unpin
    + 'static,
    session_id: String,
    request_id: String,
    session_store: Arc<dyn SessionStore>,
    user_agent: Option<String>,
) {
    tokio::spawn(async move {
        // Catch and log any panics/errors in background parsing
        let session_id_for_error = session_id.clone();
        let result = std::panic::AssertUnwindSafe(async {
            let parsed = parse_anthropic_stream::<_, E>(stream).await;

            // Emit ToolCallRecorded events for each tool call with arguments
            for tool_call in &parsed.tool_calls {
                let store_clone = session_store.clone();
                let session_id_clone = session_id.clone();
                let request_id_clone = request_id.clone();
                let tool_name = tool_call.tool_name.clone();
                let tool_call_id = tool_call.tool_call_id.clone();
                let input_size = tool_call.tool_arguments.len();
                let tool_arguments = tool_call.tool_arguments.clone();

                tokio::spawn(async move {
                    let event = lunaroute_session::SessionEvent::ToolCallRecorded {
                        session_id: session_id_clone,
                        request_id: request_id_clone,
                        timestamp: chrono::Utc::now(),
                        tool_name,
                        tool_call_id,
                        execution_time_ms: None,
                        input_size_bytes: input_size,
                        output_size_bytes: None,
                        success: None,
                        tool_arguments: Some(tool_arguments),
                    };
                    if let Ok(json) = serde_json::to_value(event) {
                        let _ = store_clone.write_event(None, json).await;
                    }
                });
            }

            // Only record StatsUpdated if we found meaningful data
            if parsed.tokens.grand_total > 0 || parsed.tool_summary.total_tool_calls > 0 {
                tracing::debug!(
                    "Async parsed Anthropic stream: tokens={}, tools={}, tool_calls_with_args={}",
                    parsed.tokens.grand_total,
                    parsed.tool_summary.total_tool_calls,
                    parsed.tool_calls.len()
                );

                let store_clone = session_store.clone();
                let session_id_clone = session_id.clone();
                let request_id_clone = request_id.clone();

                tokio::spawn(async move {
                    let event = lunaroute_session::SessionEvent::StatsUpdated {
                        session_id: session_id_clone,
                        request_id: request_id_clone,
                        timestamp: chrono::Utc::now(),
                        token_updates: if parsed.tokens.grand_total > 0 {
                            Some(parsed.tokens)
                        } else {
                            None
                        },
                        tool_call_updates: if parsed.tool_summary.total_tool_calls > 0 {
                            Some(parsed.tool_summary)
                        } else {
                            None
                        },
                        model_used: parsed.model_used,
                        response_size_bytes: parsed.response_size_bytes,
                        content_blocks: parsed.content_blocks,
                        has_refusal: parsed.has_refusal,
                        user_agent,
                    };
                    if let Ok(json) = serde_json::to_value(event) {
                        let _ = store_clone.write_event(None, json).await;
                    }
                });
            }
        });

        if let Err(e) = futures::FutureExt::catch_unwind(result).await {
            tracing::error!(
                "Panic in async Anthropic stream parser for session {}: {:?}",
                session_id_for_error,
                e
            );
        }
    });
}

/// Spawn async parsing task for OpenAI stream
///
/// Returns immediately, parsing happens in background without blocking client response.
///
/// **Panic Safety**: Uses `catch_unwind` to prevent panics from crashing the server.
/// All errors are logged with session ID for debugging.
pub fn spawn_openai_parser<E>(
    stream: impl futures::Stream<
        Item = Result<eventsource_stream::Event, eventsource_stream::EventStreamError<E>>,
    > + Send
    + Unpin
    + 'static,
    session_id: String,
    request_id: String,
    session_store: Arc<dyn SessionStore>,
    user_agent: Option<String>,
) {
    tokio::spawn(async move {
        // Catch and log any panics/errors in background parsing
        let session_id_for_error = session_id.clone();
        let result = std::panic::AssertUnwindSafe(async {
            let parsed = parse_openai_stream::<_, E>(stream).await;

            // Only record if we found meaningful data
            if parsed.tokens.grand_total > 0 || parsed.tool_summary.total_tool_calls > 0 {
                tracing::debug!(
                    "Async parsed OpenAI stream: tokens={}, tools={}",
                    parsed.tokens.grand_total,
                    parsed.tool_summary.total_tool_calls
                );

                let store_clone = session_store.clone();
                let session_id_clone = session_id.clone();
                let request_id_clone = request_id.clone();

                tokio::spawn(async move {
                    let event = lunaroute_session::SessionEvent::StatsUpdated {
                        session_id: session_id_clone,
                        request_id: request_id_clone,
                        timestamp: chrono::Utc::now(),
                        token_updates: if parsed.tokens.grand_total > 0 {
                            Some(parsed.tokens)
                        } else {
                            None
                        },
                        tool_call_updates: if parsed.tool_summary.total_tool_calls > 0 {
                            Some(parsed.tool_summary)
                        } else {
                            None
                        },
                        model_used: parsed.model_used,
                        response_size_bytes: parsed.response_size_bytes,
                        content_blocks: parsed.content_blocks,
                        has_refusal: parsed.has_refusal,
                        user_agent,
                    };
                    if let Ok(json) = serde_json::to_value(event) {
                        let _ = store_clone.write_event(None, json).await;
                    }
                });
            }
        });

        if let Err(e) = futures::FutureExt::catch_unwind(result).await {
            tracing::error!(
                "Panic in async OpenAI stream parser for session {}: {:?}",
                session_id_for_error,
                e
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;

    #[tokio::test]
    async fn test_parse_anthropic_stream_with_tokens() {
        let events: Vec<Result<eventsource_stream::Event, eventsource_stream::EventStreamError<std::convert::Infallible>>> = vec![
            Ok(eventsource_stream::Event {
                event: "message_start".to_string(),
                data: r#"{"type":"message_start","message":{"model":"claude-3-opus","usage":{"input_tokens":100}}}"#.to_string(),
                id: String::new(),
                retry: None,
            }),
            Ok(eventsource_stream::Event {
                event: "message_delta".to_string(),
                data: r#"{"type":"message_delta","usage":{"output_tokens":50}}"#.to_string(),
                id: String::new(),
                retry: None,
            }),
        ];

        let stream = stream::iter(events);
        let parsed = parse_anthropic_stream(stream).await;

        assert_eq!(parsed.tokens.total_input, 100);
        assert_eq!(parsed.tokens.total_output, 50);
        assert_eq!(parsed.tokens.grand_total, 150);
        assert_eq!(parsed.model_used, Some("claude-3-opus".to_string()));
    }

    #[tokio::test]
    async fn test_parse_anthropic_stream_with_tools() {
        let events: Vec<Result<eventsource_stream::Event, eventsource_stream::EventStreamError<std::convert::Infallible>>> = vec![
            Ok(eventsource_stream::Event {
                event: "content_block_start".to_string(),
                data: r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"get_weather"}}"#.to_string(),
                id: String::new(),
                retry: None,
            }),
            Ok(eventsource_stream::Event {
                event: "content_block_start".to_string(),
                data: r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_2","name":"search"}}"#.to_string(),
                id: String::new(),
                retry: None,
            }),
        ];

        let stream = stream::iter(events);
        let parsed = parse_anthropic_stream(stream).await;

        assert_eq!(parsed.tool_summary.total_tool_calls, 2);
        assert_eq!(parsed.tool_summary.unique_tool_count, 2);
        assert!(parsed.tool_summary.by_tool.contains_key("get_weather"));
        assert!(parsed.tool_summary.by_tool.contains_key("search"));
    }

    #[tokio::test]
    async fn test_parse_openai_stream_with_tokens() {
        let events: Vec<
            Result<
                eventsource_stream::Event,
                eventsource_stream::EventStreamError<std::convert::Infallible>,
            >,
        > = vec![Ok(eventsource_stream::Event {
            event: "data".to_string(),
            data: r#"{"model":"gpt-4","usage":{"prompt_tokens":100,"completion_tokens":50}}"#
                .to_string(),
            id: String::new(),
            retry: None,
        })];

        let stream = stream::iter(events);
        let parsed = parse_openai_stream(stream).await;

        assert_eq!(parsed.tokens.total_input, 100);
        assert_eq!(parsed.tokens.total_output, 50);
        assert_eq!(parsed.tokens.grand_total, 150);
        assert_eq!(parsed.model_used, Some("gpt-4".to_string()));
    }

    #[tokio::test]
    async fn test_parse_openai_stream_with_tools() {
        let events: Vec<Result<eventsource_stream::Event, eventsource_stream::EventStreamError<std::convert::Infallible>>> = vec![
            Ok(eventsource_stream::Event {
                event: "data".to_string(),
                data: r#"{"choices":[{"delta":{"tool_calls":[{"function":{"name":"get_weather"}}]}}]}"#.to_string(),
                id: String::new(),
                retry: None,
            }),
            Ok(eventsource_stream::Event {
                event: "data".to_string(),
                data: r#"{"choices":[{"delta":{"tool_calls":[{"function":{"name":"search"}}]}}]}"#.to_string(),
                id: String::new(),
                retry: None,
            }),
        ];

        let stream = stream::iter(events);
        let parsed = parse_openai_stream(stream).await;

        assert_eq!(parsed.tool_summary.total_tool_calls, 2);
        assert_eq!(parsed.tool_summary.unique_tool_count, 2);
        assert!(parsed.tool_summary.by_tool.contains_key("get_weather"));
        assert!(parsed.tool_summary.by_tool.contains_key("search"));
    }
}
