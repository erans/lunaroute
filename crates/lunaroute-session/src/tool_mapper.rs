//! Maps tool_call_id to tool_name for tracking results
//!
//! When an LLM returns tool calls, we need to remember the mapping of
//! tool_call_id → tool_name so that when the client sends back tool results
//! (which only contain the tool_call_id), we can determine which tool succeeded
//! or failed.
//!
//! Uses DashMap for lock-free concurrent access with automatic cleanup.

use dashmap::DashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Tracks tool calls to map IDs to names when results arrive
///
/// This maintains a temporary mapping of tool call IDs to tool names with a TTL.
/// Tool calls are recorded when responses are sent, and looked up when tool results
/// arrive in follow-up requests.
///
/// Uses DashMap for lock-free concurrent access, preventing thread contention
/// in high-throughput scenarios.
#[derive(Debug)]
pub struct ToolCallMapper {
    /// Map: tool_call_id → (tool_name, timestamp)
    mappings: DashMap<String, (String, Instant)>,
    /// TTL for mappings (default: 1 hour)
    ttl: Duration,
    /// Counter for operations since last cleanup (for periodic cleanup)
    ops_since_cleanup: AtomicUsize,
    /// Cleanup threshold (cleanup every N operations)
    cleanup_threshold: usize,
}

impl ToolCallMapper {
    /// Create a new tool call mapper with default TTL (1 hour)
    pub fn new() -> Self {
        Self::with_ttl(Duration::from_secs(3600))
    }

    /// Create a new tool call mapper with custom TTL
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            mappings: DashMap::new(),
            ttl,
            ops_since_cleanup: AtomicUsize::new(0),
            cleanup_threshold: 100, // Cleanup every 100 operations
        }
    }

    /// Record a tool call from a response
    ///
    /// This should be called when recording a response that contains tool calls,
    /// storing the mapping for future lookup.
    pub fn record_call(&self, tool_call_id: String, tool_name: String) {
        self.mappings
            .insert(tool_call_id, (tool_name, Instant::now()));
        self.maybe_cleanup();
    }

    /// Look up tool name from result ID
    ///
    /// Returns None if the ID is not found or has expired.
    pub fn lookup(&self, tool_call_id: &str) -> Option<String> {
        self.maybe_cleanup();

        self.mappings.get(tool_call_id).and_then(|entry| {
            let (name, timestamp) = entry.value();
            if timestamp.elapsed() < self.ttl {
                Some(name.clone())
            } else {
                None
            }
        })
    }

    /// Maybe trigger cleanup based on operation count
    ///
    /// This performs opportunistic cleanup to prevent unbounded growth.
    /// Cleanup happens every N operations (defined by cleanup_threshold).
    fn maybe_cleanup(&self) {
        let ops = self.ops_since_cleanup.fetch_add(1, Ordering::Relaxed);

        // Trigger cleanup periodically
        if ops >= self.cleanup_threshold {
            self.ops_since_cleanup.store(0, Ordering::Relaxed);
            self.cleanup_expired();
        }
    }

    /// Clean up expired entries
    ///
    /// This is called periodically to prevent memory leaks from expired entries.
    fn cleanup_expired(&self) {
        let now = Instant::now();
        self.mappings
            .retain(|_, (_, timestamp)| now.duration_since(*timestamp) < self.ttl);
    }

    /// Get the number of active mappings (for testing/debugging)
    ///
    /// Note: This includes both expired and non-expired entries until cleanup runs.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.mappings.len()
    }

    /// Check if the mapper is empty (for testing/debugging)
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.mappings.is_empty()
    }

    /// Force immediate cleanup (for testing)
    #[cfg(test)]
    pub fn force_cleanup(&self) {
        self.cleanup_expired();
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
        let mapper = ToolCallMapper::new();

        mapper.record_call("call_abc123".to_string(), "get_weather".to_string());
        mapper.record_call("call_def456".to_string(), "search_web".to_string());

        assert_eq!(
            mapper.lookup("call_abc123"),
            Some("get_weather".to_string())
        );
        assert_eq!(mapper.lookup("call_def456"), Some("search_web".to_string()));
        assert_eq!(mapper.lookup("call_xyz789"), None);
    }

    #[test]
    fn test_expiry() {
        let mapper = ToolCallMapper::with_ttl(Duration::from_millis(100));

        mapper.record_call("call_abc123".to_string(), "get_weather".to_string());
        assert_eq!(
            mapper.lookup("call_abc123"),
            Some("get_weather".to_string())
        );

        // Wait for expiry
        sleep(Duration::from_millis(150));

        // Should be expired now
        assert_eq!(mapper.lookup("call_abc123"), None);
    }

    #[test]
    fn test_periodic_cleanup() {
        let mapper = ToolCallMapper::with_ttl(Duration::from_millis(100));

        // Record first call
        mapper.record_call("call_old".to_string(), "old_tool".to_string());
        assert_eq!(mapper.len(), 1);

        // Wait for it to expire
        sleep(Duration::from_millis(150));

        // Trigger cleanup by doing operations
        // The cleanup threshold is 100, so we need to trigger it
        mapper.force_cleanup();

        // Old one should be gone
        assert_eq!(mapper.lookup("call_old"), None);
        assert_eq!(mapper.len(), 0);
    }

    #[test]
    fn test_overwrite_same_id() {
        let mapper = ToolCallMapper::new();

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

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let mapper = Arc::new(ToolCallMapper::new());
        let mut handles = vec![];

        // Spawn multiple threads doing concurrent operations
        for i in 0..10 {
            let mapper_clone = mapper.clone();
            let handle = thread::spawn(move || {
                for j in 0..100 {
                    let id = format!("call_{}_{}", i, j);
                    let tool = format!("tool_{}_{}", i, j);
                    mapper_clone.record_call(id.clone(), tool.clone());

                    // Immediately lookup
                    assert_eq!(mapper_clone.lookup(&id), Some(tool));
                }
            });
            handles.push(handle);
        }

        // Wait for all threads
        for handle in handles {
            handle.join().unwrap();
        }

        // Should have 1000 entries (10 threads * 100 calls)
        assert_eq!(mapper.len(), 1000);
    }

    #[test]
    fn test_cleanup_prevents_unbounded_growth() {
        let mapper = ToolCallMapper::with_ttl(Duration::from_millis(50));

        // Add many entries
        for i in 0..200 {
            mapper.record_call(format!("call_{}", i), format!("tool_{}", i));
        }

        assert_eq!(mapper.len(), 200);

        // Wait for expiry
        sleep(Duration::from_millis(100));

        // Force cleanup
        mapper.force_cleanup();

        // All should be expired and removed
        assert_eq!(mapper.len(), 0);
    }
}
