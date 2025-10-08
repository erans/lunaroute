//! Maps tool_call_id to tool_name for tracking results
//!
//! When an LLM returns tool calls, we need to remember the mapping of
//! tool_call_id → tool_name so that when the client sends back tool results
//! (which only contain the tool_call_id), we can determine which tool succeeded
//! or failed.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Tracks tool calls to map IDs to names when results arrive
///
/// This maintains a temporary mapping of tool call IDs to tool names with a TTL.
/// Tool calls are recorded when responses are sent, and looked up when tool results
/// arrive in follow-up requests.
#[derive(Debug)]
pub struct ToolCallMapper {
    /// Map: tool_call_id → (tool_name, timestamp)
    mappings: HashMap<String, (String, Instant)>,
    /// TTL for mappings (default: 1 hour)
    ttl: Duration,
}

impl ToolCallMapper {
    /// Create a new tool call mapper with default TTL (1 hour)
    pub fn new() -> Self {
        Self::with_ttl(Duration::from_secs(3600))
    }

    /// Create a new tool call mapper with custom TTL
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            mappings: HashMap::new(),
            ttl,
        }
    }

    /// Record a tool call from a response
    ///
    /// This should be called when recording a response that contains tool calls,
    /// storing the mapping for future lookup.
    pub fn record_call(&mut self, tool_call_id: String, tool_name: String) {
        self.cleanup_expired();
        self.mappings
            .insert(tool_call_id, (tool_name, Instant::now()));
    }

    /// Look up tool name from result ID
    ///
    /// Returns None if the ID is not found or has expired.
    pub fn lookup(&self, tool_call_id: &str) -> Option<String> {
        self.mappings
            .get(tool_call_id)
            .filter(|(_, ts)| ts.elapsed() < self.ttl)
            .map(|(name, _)| name.clone())
    }

    /// Clean up expired entries
    ///
    /// This is called automatically during record_call to prevent unbounded growth.
    fn cleanup_expired(&mut self) {
        self.mappings.retain(|_, (_, ts)| ts.elapsed() < self.ttl);
    }

    /// Get the number of active mappings (for testing/debugging)
    #[cfg(test)]
    pub fn len(&self) -> usize {
        // Filter out expired entries
        self.mappings
            .values()
            .filter(|(_, ts)| ts.elapsed() < self.ttl)
            .count()
    }

    /// Check if the mapper is empty (for testing/debugging)
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for ToolCallMapper {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn test_record_and_lookup() {
        let mut mapper = ToolCallMapper::new();

        mapper.record_call("call_abc123".to_string(), "get_weather".to_string());
        mapper.record_call("call_def456".to_string(), "search_web".to_string());

        assert_eq!(mapper.lookup("call_abc123"), Some("get_weather".to_string()));
        assert_eq!(mapper.lookup("call_def456"), Some("search_web".to_string()));
        assert_eq!(mapper.lookup("call_xyz789"), None);
        assert_eq!(mapper.len(), 2);
    }

    #[test]
    fn test_expiry() {
        let mut mapper = ToolCallMapper::with_ttl(Duration::from_millis(100));

        mapper.record_call("call_abc123".to_string(), "get_weather".to_string());
        assert_eq!(mapper.lookup("call_abc123"), Some("get_weather".to_string()));

        // Wait for expiry
        sleep(Duration::from_millis(150));

        // Should be expired now
        assert_eq!(mapper.lookup("call_abc123"), None);
    }

    #[test]
    fn test_cleanup_on_record() {
        let mut mapper = ToolCallMapper::with_ttl(Duration::from_millis(100));

        // Record first call
        mapper.record_call("call_old".to_string(), "old_tool".to_string());
        assert_eq!(mapper.len(), 1);

        // Wait for it to expire
        sleep(Duration::from_millis(150));

        // Record new call - should trigger cleanup
        mapper.record_call("call_new".to_string(), "new_tool".to_string());

        // Old one should be gone, new one should remain
        assert_eq!(mapper.lookup("call_old"), None);
        assert_eq!(mapper.lookup("call_new"), Some("new_tool".to_string()));
        assert_eq!(mapper.len(), 1);
    }

    #[test]
    fn test_overwrite_same_id() {
        let mut mapper = ToolCallMapper::new();

        mapper.record_call("call_abc".to_string(), "tool_v1".to_string());
        mapper.record_call("call_abc".to_string(), "tool_v2".to_string());

        // Should have the latest value
        assert_eq!(mapper.lookup("call_abc"), Some("tool_v2".to_string()));
        assert_eq!(mapper.len(), 1);
    }

    #[test]
    fn test_empty_mapper() {
        let mapper = ToolCallMapper::new();
        assert!(mapper.is_empty());
        assert_eq!(mapper.len(), 0);
        assert_eq!(mapper.lookup("any_id"), None);
    }
}
