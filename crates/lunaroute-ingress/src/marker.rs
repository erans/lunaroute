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

/// Scan a request body (serde_json::Value) for [LUNAROUTE:xxx] marker.
/// Searches the LAST user message only — old markers from previous turns
/// may persist in conversation history and must be ignored.
/// Returns the first marker found in the last user message.
/// Logs a warning if multiple markers exist.
pub fn extract_marker(req: &serde_json::Value) -> MarkerResult {
    let mut found: Vec<String> = Vec::new();

    if let Some(messages) = req.get("messages").and_then(|m| m.as_array()) {
        let last_user_msg = messages
            .iter()
            .rev()
            .find(|msg| msg.get("role").and_then(|r| r.as_str()) == Some("user"));

        if let Some(msg) = last_user_msg {
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
}
