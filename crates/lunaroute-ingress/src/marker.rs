use regex::Regex;
use std::sync::LazyLock;

/// Regex to extract the provider name from a [LUNAROUTE:xxx] marker
static MARKER_EXTRACT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[LUNAROUTE:([a-zA-Z0-9._-]+)\]").unwrap());

/// Result of scanning a request body for a LUNAROUTE marker
#[derive(Debug, Clone, PartialEq)]
pub enum MarkerResult {
    /// A provider override marker was found
    Provider(String),
    /// A "clear" marker was found — strip and route normally
    Clear,
    /// No marker found
    None,
}

/// Check if a user message contains only tool_result blocks (no text content).
fn is_tool_result_only(msg: &serde_json::Value) -> bool {
    if let Some(content_arr) = msg.get("content").and_then(|c| c.as_array()) {
        !content_arr.is_empty()
            && content_arr
                .iter()
                .all(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
    } else {
        false
    }
}

/// Try to extract a LUNAROUTE marker from a single message.
fn extract_marker_from_msg(msg: &serde_json::Value) -> Vec<String> {
    let mut found = Vec::new();
    if let Some(content_arr) = msg.get("content").and_then(|c| c.as_array()) {
        for block in content_arr {
            if let Some(text) = block.get("text").and_then(|t| t.as_str())
                && let Some(caps) = MARKER_EXTRACT_RE.captures(text)
            {
                found.push(caps[1].to_string());
            }
        }
    } else if let Some(text) = msg.get("content").and_then(|c| c.as_str())
        && let Some(caps) = MARKER_EXTRACT_RE.captures(text)
    {
        found.push(caps[1].to_string());
    }
    found
}

/// Scan a request body (serde_json::Value) for [LUNAROUTE:xxx] marker.
///
/// Searches the last user message first. If that message contains only
/// tool_result blocks (an automatic follow-up to a tool call), walks back
/// through earlier user messages to inherit routing from the message that
/// triggered the tool call chain. This ensures multi-step tool calling
/// stays on the same provider.
///
/// Returns the first marker found. Logs a warning if multiple markers exist.
pub fn extract_marker(req: &serde_json::Value) -> MarkerResult {
    let mut found: Vec<String> = Vec::new();

    if let Some(messages) = req.get("messages").and_then(|m| m.as_array()) {
        // Walk user messages from the end
        for msg in messages.iter().rev() {
            if msg.get("role").and_then(|r| r.as_str()) != Some("user") {
                continue;
            }

            // If this is a tool-result-only message, skip it and keep looking back
            if is_tool_result_only(msg) {
                continue;
            }

            found = extract_marker_from_msg(msg);
            break;
        }
    }

    if found.len() > 1 {
        tracing::warn!(
            "Multiple LUNAROUTE markers found: {:?}. Using first: {}",
            found,
            found[0]
        );
    }

    match found.into_iter().next() {
        Some(name) if name.eq_ignore_ascii_case("clear") => MarkerResult::Clear,
        Some(name) => MarkerResult::Provider(name),
        None => MarkerResult::None,
    }
}

/// Regex matching a standalone system-reminder block containing only a LUNAROUTE marker
static STANDALONE_STRIP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?s)^\s*<system-reminder>\s*\[LUNAROUTE:[a-zA-Z0-9._-]+\]\s*</system-reminder>\s*$",
    )
    .unwrap()
});

/// Regex matching just the marker text (for inline stripping)
static INLINE_STRIP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[LUNAROUTE:[a-zA-Z0-9._-]+\]").unwrap());

/// Remove [LUNAROUTE:xxx] marker text from the request body.
/// Uses the same message selection logic as extract_marker: walks backward
/// from the end, skipping tool-result-only user messages.
/// - Standalone content block (entire text is system-reminder with marker): remove block.
/// - Inline in a larger text block: regex-replace the marker text.
/// - String content: regex-replace within the string.
///
/// After stripping, removes empty text blocks and messages with empty content arrays.
pub fn strip_marker(req: &mut serde_json::Value) {
    let Some(messages) = req.get_mut("messages").and_then(|m| m.as_array_mut()) else {
        return;
    };

    // Find the target user message using the same logic as extract_marker:
    // walk backwards, skipping tool-result-only user messages.
    let mut target_idx = None;
    for (i, msg) in messages.iter().enumerate().rev() {
        if msg.get("role").and_then(|r| r.as_str()) != Some("user") {
            continue;
        }
        if is_tool_result_only(msg) {
            continue;
        }
        target_idx = Some(i);
        break;
    }

    let Some(idx) = target_idx else { return };
    let msg = &mut messages[idx];

    // Handle array content
    if let Some(content_arr) = msg.get_mut("content").and_then(|c| c.as_array_mut()) {
        content_arr.retain(|block| {
            if let Some(text) = block.get("text").and_then(|t| t.as_str())
                && STANDALONE_STRIP_RE.is_match(text)
            {
                return false;
            }
            true
        });

        // For remaining blocks, strip inline markers
        for block in content_arr.iter_mut() {
            if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                let stripped = INLINE_STRIP_RE.replace_all(text, "").to_string();
                if stripped != text {
                    block["text"] = serde_json::Value::String(stripped);
                }
            }
        }

        // Remove blocks that became empty
        content_arr.retain(|block| {
            block
                .get("text")
                .and_then(|t| t.as_str())
                .is_none_or(|t| !t.trim().is_empty())
        });
    }
    // Handle string content
    else if let Some(text) = msg.get("content").and_then(|c| c.as_str()) {
        let stripped = INLINE_STRIP_RE.replace_all(text, "").to_string();
        if stripped != text {
            msg["content"] = serde_json::Value::String(stripped);
        }
    }

    // Remove the message if content array is now empty
    if let Some(arr) = messages[idx].get("content").and_then(|c| c.as_array())
        && arr.is_empty()
    {
        messages.remove(idx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_marker_from_system_reminder_block() {
        let req = json!({
            "model": "claude-opus-4-20250514",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "hello world"},
                    {"type": "text", "text": "<system-reminder>\n[LUNAROUTE:sonnet]\n</system-reminder>"}
                ]
            }]
        });
        assert_eq!(
            extract_marker(&req),
            MarkerResult::Provider("sonnet".to_string())
        );
    }

    #[test]
    fn test_extract_marker_clear() {
        let req = json!({
            "model": "claude-opus-4-20250514",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "<system-reminder>\n[LUNAROUTE:clear]\n</system-reminder>"}
                ]
            }]
        });
        assert_eq!(extract_marker(&req), MarkerResult::Clear);
    }

    #[test]
    fn test_extract_marker_none() {
        let req = json!({
            "model": "claude-opus-4-20250514",
            "messages": [{
                "role": "user",
                "content": [{"type": "text", "text": "hello world"}]
            }]
        });
        assert_eq!(extract_marker(&req), MarkerResult::None);
    }

    #[test]
    fn test_extract_marker_string_content() {
        let req = json!({
            "model": "gpt-4",
            "messages": [{
                "role": "user",
                "content": "rewrite this [LUNAROUTE:gpt4o] using streams"
            }]
        });
        assert_eq!(
            extract_marker(&req),
            MarkerResult::Provider("gpt4o".to_string())
        );
    }

    #[test]
    fn test_extract_marker_no_messages() {
        let req = json!({"model": "gpt-4"});
        assert_eq!(extract_marker(&req), MarkerResult::None);
    }

    #[test]
    fn test_extract_marker_dots_and_dashes() {
        let req = json!({
            "messages": [{
                "role": "user",
                "content": "[LUNAROUTE:my-provider.v2]"
            }]
        });
        assert_eq!(
            extract_marker(&req),
            MarkerResult::Provider("my-provider.v2".to_string())
        );
    }

    #[test]
    fn test_extract_marker_multiple_returns_first() {
        let req = json!({
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "[LUNAROUTE:sonnet]"},
                    {"type": "text", "text": "[LUNAROUTE:gpt4o]"}
                ]
            }]
        });
        assert_eq!(
            extract_marker(&req),
            MarkerResult::Provider("sonnet".to_string())
        );
    }

    #[test]
    fn test_extract_marker_ignores_old_messages() {
        let req = json!({
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "old request"},
                        {"type": "text", "text": "<system-reminder>\n[LUNAROUTE:sonnet]\n</system-reminder>"}
                    ]
                },
                {"role": "assistant", "content": "response"},
                {
                    "role": "user",
                    "content": [{"type": "text", "text": "new request without marker"}]
                }
            ]
        });
        assert_eq!(extract_marker(&req), MarkerResult::None);
    }

    #[test]
    fn test_extract_marker_uses_last_user_message() {
        let req = json!({
            "messages": [
                {
                    "role": "user",
                    "content": "<system-reminder>\n[LUNAROUTE:sonnet]\n</system-reminder>"
                },
                {"role": "assistant", "content": "response"},
                {
                    "role": "user",
                    "content": "<system-reminder>\n[LUNAROUTE:gpt4o]\n</system-reminder>"
                }
            ]
        });
        assert_eq!(
            extract_marker(&req),
            MarkerResult::Provider("gpt4o".to_string())
        );
    }

    #[test]
    fn test_strip_standalone_system_reminder_block() {
        let mut req = json!({
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "hello world"},
                    {"type": "text", "text": "<system-reminder>\n[LUNAROUTE:sonnet]\n</system-reminder>"}
                ]
            }]
        });
        strip_marker(&mut req);
        let content = req["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["text"], "hello world");
    }

    #[test]
    fn test_strip_inline_marker_from_text() {
        let mut req = json!({
            "messages": [{
                "role": "user",
                "content": "rewrite this [LUNAROUTE:sonnet] using streams"
            }]
        });
        strip_marker(&mut req);
        assert_eq!(req["messages"][0]["content"], "rewrite this  using streams");
    }

    #[test]
    fn test_strip_inline_marker_from_content_block() {
        let mut req = json!({
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "rewrite [LUNAROUTE:sonnet] this"}
                ]
            }]
        });
        strip_marker(&mut req);
        let content = req["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["text"], "rewrite  this");
    }

    #[test]
    fn test_strip_removes_empty_blocks_and_messages() {
        let mut req = json!({
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "<system-reminder>\n[LUNAROUTE:sonnet]\n</system-reminder>"}
                ]
            }]
        });
        strip_marker(&mut req);
        let messages = req["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 0);
    }

    #[test]
    fn test_strip_no_marker_is_noop() {
        let mut req = json!({
            "messages": [{
                "role": "user",
                "content": [{"type": "text", "text": "hello world"}]
            }]
        });
        let original = req.clone();
        strip_marker(&mut req);
        assert_eq!(req, original);
    }

    #[test]
    fn test_full_flow_extract_then_strip() {
        let mut req = json!({
            "model": "claude-opus-4-20250514",
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "rewrite this function"},
                        {"type": "text", "text": "<system-reminder>\n[LUNAROUTE:sonnet]\n</system-reminder>"}
                    ]
                }
            ]
        });

        // Extract
        let result = extract_marker(&req);
        assert_eq!(result, MarkerResult::Provider("sonnet".to_string()));

        // Strip
        strip_marker(&mut req);

        // Verify marker is gone
        assert_eq!(extract_marker(&req), MarkerResult::None);

        // Verify user message is preserved
        let content = req["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["text"], "rewrite this function");
    }

    #[test]
    fn test_full_flow_clear_marker() {
        let mut req = json!({
            "model": "claude-opus-4-20250514",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "hello"},
                    {"type": "text", "text": "<system-reminder>\n[LUNAROUTE:clear]\n</system-reminder>"}
                ]
            }]
        });

        assert_eq!(extract_marker(&req), MarkerResult::Clear);
        strip_marker(&mut req);
        assert_eq!(extract_marker(&req), MarkerResult::None);

        let content = req["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["text"], "hello");
    }

    #[test]
    fn test_model_override_applied() {
        let mut req = json!({
            "model": "claude-opus-4-20250514",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "hello"},
                    {"type": "text", "text": "<system-reminder>\n[LUNAROUTE:sonnet]\n</system-reminder>"}
                ]
            }]
        });

        // Simulate what the handler does: extract, override model, strip
        if let MarkerResult::Provider(_) = extract_marker(&req) {
            req["model"] = serde_json::Value::String("claude-sonnet-4-20250514".to_string());
        }
        strip_marker(&mut req);

        assert_eq!(req["model"], "claude-sonnet-4-20250514");
        assert_eq!(extract_marker(&req), MarkerResult::None);
    }

    #[test]
    fn test_extract_marker_skips_tool_result_messages() {
        // Simulates: user sends #!kimik25, model responds with tool_use,
        // Claude Code sends tool_result. The marker should be inherited
        // from the earlier user message.
        let req = json!({
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "<system-reminder>\n[LUNAROUTE:kimik25]\n</system-reminder>"},
                        {"type": "text", "text": "list files #!kimik25"}
                    ]
                },
                {
                    "role": "assistant",
                    "content": [
                        {"type": "tool_use", "id": "toolu_functionsBash0", "name": "Bash", "input": {"command": "ls"}}
                    ]
                },
                {
                    "role": "user",
                    "content": [
                        {"type": "tool_result", "tool_use_id": "toolu_functionsBash0", "content": "file1.txt\nfile2.txt"}
                    ]
                }
            ]
        });
        assert_eq!(
            extract_marker(&req),
            MarkerResult::Provider("kimik25".to_string())
        );
    }

    #[test]
    fn test_extract_marker_skips_multiple_tool_result_rounds() {
        // Multiple tool-call rounds: marker should still be found
        let req = json!({
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "[LUNAROUTE:kimik25]"},
                        {"type": "text", "text": "create and cat a file"}
                    ]
                },
                {
                    "role": "assistant",
                    "content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {}}]
                },
                {
                    "role": "user",
                    "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "ok"}]
                },
                {
                    "role": "assistant",
                    "content": [{"type": "tool_use", "id": "t2", "name": "Bash", "input": {}}]
                },
                {
                    "role": "user",
                    "content": [{"type": "tool_result", "tool_use_id": "t2", "content": "ok"}]
                }
            ]
        });
        assert_eq!(
            extract_marker(&req),
            MarkerResult::Provider("kimik25".to_string())
        );
    }

    #[test]
    fn test_extract_marker_no_inherit_when_new_text_message() {
        // If the user sends a new text message after tool results,
        // only the new message matters (no stale marker inheritance).
        let req = json!({
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "[LUNAROUTE:kimik25]"},
                        {"type": "text", "text": "do something"}
                    ]
                },
                {
                    "role": "assistant",
                    "content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {}}]
                },
                {
                    "role": "user",
                    "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "ok"}]
                },
                {
                    "role": "assistant",
                    "content": [{"type": "text", "text": "Done!"}]
                },
                {
                    "role": "user",
                    "content": [{"type": "text", "text": "now do something else"}]
                }
            ]
        });
        // New text message without marker — should NOT inherit kimik25
        assert_eq!(extract_marker(&req), MarkerResult::None);
    }

    #[test]
    fn test_strip_marker_skips_tool_result_messages() {
        // strip_marker should target the same message as extract_marker:
        // skip trailing tool-result-only user messages.
        let mut req = json!({
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "<system-reminder>\n[LUNAROUTE:kimik25]\n</system-reminder>"},
                        {"type": "text", "text": "list files"}
                    ]
                },
                {
                    "role": "assistant",
                    "content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {}}]
                },
                {
                    "role": "user",
                    "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "ok"}]
                }
            ]
        });

        // Extract should find the marker in the first user message
        assert_eq!(
            extract_marker(&req),
            MarkerResult::Provider("kimik25".to_string())
        );

        // Strip should also target the first user message (not the tool_result message)
        strip_marker(&mut req);

        // Marker should be gone from the first user message
        assert_eq!(extract_marker(&req), MarkerResult::None);

        // First user message should still have text content
        let content = req["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["text"], "list files");

        // Tool result message should be untouched
        let tool_msg = &req["messages"][2]["content"];
        assert!(tool_msg.as_array().unwrap()[0].get("tool_use_id").is_some());
    }
}
