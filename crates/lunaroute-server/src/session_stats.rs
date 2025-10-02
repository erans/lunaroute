//! Session statistics tracking for proxy performance monitoring

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// Statistics for a single session
#[derive(Debug, Clone, Default)]
pub struct SessionStats {
    /// Total number of requests in this session
    pub request_count: u64,
    /// Total input tokens consumed
    pub input_tokens: u64,
    /// Total output tokens generated
    pub output_tokens: u64,
    /// Total thinking tokens used (Anthropic extended thinking)
    pub thinking_tokens: u64,
    /// Number of requests that used thinking
    pub thinking_requests: u64,
    /// Tool calls by name (tool_name -> call_count)
    pub tool_calls: HashMap<String, u64>,
    /// Total number of tool calls across all tools
    pub total_tool_calls: u64,
    /// Total time spent processing requests before proxying to provider
    pub pre_proxy_time: Duration,
    /// Total time spent processing responses after receiving from provider
    pub post_proxy_time: Duration,
}

impl SessionStats {
    /// Calculate average processing time per request
    pub fn avg_processing_time(&self) -> Duration {
        if self.request_count == 0 {
            return Duration::ZERO;
        }
        (self.pre_proxy_time + self.post_proxy_time) / self.request_count as u32
    }

    /// Total processing overhead time
    pub fn total_processing_time(&self) -> Duration {
        self.pre_proxy_time + self.post_proxy_time
    }

    /// Average thinking tokens per thinking-enabled request
    pub fn avg_thinking_tokens(&self) -> f64 {
        if self.thinking_requests == 0 {
            return 0.0;
        }
        self.thinking_tokens as f64 / self.thinking_requests as f64
    }
}

/// Configuration for session stats tracking
#[derive(Debug, Clone)]
pub struct SessionStatsConfig {
    /// Maximum number of sessions to track (LRU eviction)
    pub max_sessions: usize,
}

impl Default for SessionStatsConfig {
    fn default() -> Self {
        Self { max_sessions: 100 }
    }
}

/// Thread-safe session statistics tracker
#[derive(Clone)]
pub struct SessionStatsTracker {
    config: SessionStatsConfig,
    stats: Arc<RwLock<HashMap<String, SessionStats>>>,
}

impl SessionStatsTracker {
    /// Create a new session stats tracker
    pub fn new(config: SessionStatsConfig) -> Self {
        Self {
            config,
            stats: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Record statistics for a request
    #[allow(clippy::too_many_arguments)]
    pub fn record_request(
        &self,
        session_id: String,
        input_tokens: u64,
        output_tokens: u64,
        thinking_tokens: u64,
        tool_calls: HashMap<String, u64>,
        pre_proxy_time: Duration,
        post_proxy_time: Duration,
    ) {
        let mut stats = self.stats.write().unwrap();

        // Check if we need to evict an entry (simple FIFO, not true LRU)
        if stats.len() >= self.config.max_sessions && !stats.contains_key(&session_id) {
            // Remove the first entry (not ideal LRU, but simple)
            if let Some(key) = stats.keys().next().cloned() {
                stats.remove(&key);
            }
        }

        let entry = stats.entry(session_id).or_default();
        entry.request_count += 1;
        entry.input_tokens += input_tokens;
        entry.output_tokens += output_tokens;
        entry.thinking_tokens += thinking_tokens;
        if thinking_tokens > 0 {
            entry.thinking_requests += 1;
        }

        // Track tool calls
        for (tool_name, count) in tool_calls {
            *entry.tool_calls.entry(tool_name).or_insert(0) += count;
            entry.total_tool_calls += count;
        }

        entry.pre_proxy_time += pre_proxy_time;
        entry.post_proxy_time += post_proxy_time;
    }

    /// Get statistics for a specific session
    #[allow(dead_code)]
    pub fn get_session_stats(&self, session_id: &str) -> Option<SessionStats> {
        self.stats.read().unwrap().get(session_id).cloned()
    }

    /// Get all session statistics
    #[allow(dead_code)]
    pub fn get_all_stats(&self) -> HashMap<String, SessionStats> {
        self.stats.read().unwrap().clone()
    }

    /// Get total number of tracked sessions
    #[allow(dead_code)]
    pub fn session_count(&self) -> usize {
        self.stats.read().unwrap().len()
    }

    /// Print statistics summary
    pub fn print_summary(&self) {
        let stats = self.stats.read().unwrap();

        if stats.is_empty() {
            tracing::info!("No session statistics to report");
            return;
        }

        tracing::info!("═══════════════════════════════════════════════════════════");
        tracing::info!("              Session Statistics Summary");
        tracing::info!("═══════════════════════════════════════════════════════════");
        tracing::info!("Total sessions tracked: {}", stats.len());
        tracing::info!("");

        // Calculate aggregate statistics
        let mut total_requests = 0u64;
        let mut total_input_tokens = 0u64;
        let mut total_output_tokens = 0u64;
        let mut total_thinking_tokens = 0u64;
        let mut total_thinking_requests = 0u64;
        let mut total_pre_time = Duration::ZERO;
        let mut total_post_time = Duration::ZERO;
        let mut aggregate_tool_calls: HashMap<String, u64> = HashMap::new();
        let mut total_tool_calls = 0u64;

        // Sort sessions by session ID for consistent output
        let mut sorted_sessions: Vec<_> = stats.iter().collect();
        sorted_sessions.sort_by(|a, b| a.0.cmp(b.0));

        for (session_id, session_stats) in &sorted_sessions {
            total_requests += session_stats.request_count;
            total_input_tokens += session_stats.input_tokens;
            total_output_tokens += session_stats.output_tokens;
            total_thinking_tokens += session_stats.thinking_tokens;
            total_thinking_requests += session_stats.thinking_requests;
            total_pre_time += session_stats.pre_proxy_time;
            total_post_time += session_stats.post_proxy_time;
            total_tool_calls += session_stats.total_tool_calls;

            // Aggregate tool calls across sessions
            for (tool_name, count) in &session_stats.tool_calls {
                *aggregate_tool_calls.entry(tool_name.clone()).or_insert(0) += count;
            }

            tracing::info!("Session: {}", session_id);
            tracing::info!("  Requests: {}", session_stats.request_count);
            tracing::info!("  Input tokens: {}", session_stats.input_tokens);
            tracing::info!("  Output tokens: {}", session_stats.output_tokens);
            tracing::info!("  Total tokens: {}", session_stats.input_tokens + session_stats.output_tokens);
            if session_stats.thinking_tokens > 0 {
                tracing::info!("  Thinking tokens: {} ({} requests with thinking)",
                    session_stats.thinking_tokens, session_stats.thinking_requests);
                tracing::info!("  Avg thinking tokens/thinking request: {:.1}", session_stats.avg_thinking_tokens());
            }
            if session_stats.total_tool_calls > 0 {
                tracing::info!("  Tool calls: {} total", session_stats.total_tool_calls);
                // Sort tool calls by count (descending)
                let mut tool_list: Vec<_> = session_stats.tool_calls.iter().collect();
                tool_list.sort_by(|a, b| b.1.cmp(a.1));
                for (tool_name, count) in tool_list {
                    tracing::info!("    {}: {}", tool_name, count);
                }
            }
            tracing::info!("  Pre-proxy time: {:.2}ms", session_stats.pre_proxy_time.as_secs_f64() * 1000.0);
            tracing::info!("  Post-proxy time: {:.2}ms", session_stats.post_proxy_time.as_secs_f64() * 1000.0);
            tracing::info!("  Total processing time: {:.2}ms", session_stats.total_processing_time().as_secs_f64() * 1000.0);
            tracing::info!("  Avg processing time/request: {:.2}ms", session_stats.avg_processing_time().as_secs_f64() * 1000.0);
            tracing::info!("");
        }

        tracing::info!("───────────────────────────────────────────────────────────");
        tracing::info!("              Aggregate Statistics");
        tracing::info!("───────────────────────────────────────────────────────────");
        tracing::info!("Total requests across all sessions: {}", total_requests);
        tracing::info!("Total input tokens: {}", total_input_tokens);
        tracing::info!("Total output tokens: {}", total_output_tokens);
        tracing::info!("Total tokens: {}", total_input_tokens + total_output_tokens);
        if total_thinking_tokens > 0 {
            tracing::info!("Total thinking tokens: {} ({} requests with thinking)",
                total_thinking_tokens, total_thinking_requests);
            if total_thinking_requests > 0 {
                tracing::info!("Avg thinking tokens/thinking request: {:.1}",
                    total_thinking_tokens as f64 / total_thinking_requests as f64);
            }
        }
        tracing::info!("Total pre-proxy time: {:.2}ms", total_pre_time.as_secs_f64() * 1000.0);
        tracing::info!("Total post-proxy time: {:.2}ms", total_post_time.as_secs_f64() * 1000.0);
        tracing::info!("Total processing overhead: {:.2}ms", (total_pre_time + total_post_time).as_secs_f64() * 1000.0);

        if total_requests > 0 {
            let avg_pre = total_pre_time / total_requests as u32;
            let avg_post = total_post_time / total_requests as u32;
            tracing::info!("Avg pre-proxy time/request: {:.2}ms", avg_pre.as_secs_f64() * 1000.0);
            tracing::info!("Avg post-proxy time/request: {:.2}ms", avg_post.as_secs_f64() * 1000.0);
            tracing::info!("Avg total processing time/request: {:.2}ms", (avg_pre + avg_post).as_secs_f64() * 1000.0);
        }

        if total_tool_calls > 0 {
            tracing::info!("");
            tracing::info!("Tool Usage Across All Sessions:");
            tracing::info!("Total tool calls: {}", total_tool_calls);
            // Sort aggregate tool calls by count (descending)
            let mut tool_list: Vec<_> = aggregate_tool_calls.iter().collect();
            tool_list.sort_by(|a, b| b.1.cmp(a.1));
            for (tool_name, count) in tool_list {
                let percentage = (*count as f64 / total_tool_calls as f64) * 100.0;
                tracing::info!("  {}: {} ({:.1}%)", tool_name, count, percentage);
            }
        }

        tracing::info!("═══════════════════════════════════════════════════════════");
    }
}

impl lunaroute_ingress::types::SessionStatsTracker for SessionStatsTracker {
    fn record_request(&self, session_id: String, stats: lunaroute_ingress::types::SessionRequestStats) {
        self.record_request(
            session_id,
            stats.input_tokens,
            stats.output_tokens,
            stats.thinking_tokens,
            stats.tool_calls,
            stats.pre_proxy_time,
            stats.post_proxy_time,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_stats_default() {
        let stats = SessionStats::default();
        assert_eq!(stats.request_count, 0);
        assert_eq!(stats.input_tokens, 0);
        assert_eq!(stats.output_tokens, 0);
        assert_eq!(stats.thinking_tokens, 0);
        assert_eq!(stats.thinking_requests, 0);
        assert_eq!(stats.pre_proxy_time, Duration::ZERO);
        assert_eq!(stats.post_proxy_time, Duration::ZERO);
    }

    #[test]
    fn test_avg_processing_time() {
        let mut stats = SessionStats::default();
        stats.request_count = 2;
        stats.pre_proxy_time = Duration::from_millis(10);
        stats.post_proxy_time = Duration::from_millis(20);

        let avg = stats.avg_processing_time();
        assert_eq!(avg, Duration::from_millis(15));
    }

    #[test]
    fn test_avg_processing_time_zero_requests() {
        let stats = SessionStats::default();
        assert_eq!(stats.avg_processing_time(), Duration::ZERO);
    }

    #[test]
    fn test_tracker_record_request() {
        let tracker = SessionStatsTracker::new(SessionStatsConfig::default());

        tracker.record_request(
            "session1".to_string(),
            100,
            200,
            0,
            HashMap::new(),
            Duration::from_millis(5),
            Duration::from_millis(10),
        );

        let stats = tracker.get_session_stats("session1").unwrap();
        assert_eq!(stats.request_count, 1);
        assert_eq!(stats.input_tokens, 100);
        assert_eq!(stats.output_tokens, 200);
        assert_eq!(stats.thinking_tokens, 0);
        assert_eq!(stats.pre_proxy_time, Duration::from_millis(5));
        assert_eq!(stats.post_proxy_time, Duration::from_millis(10));
    }

    #[test]
    fn test_tracker_multiple_requests_same_session() {
        let tracker = SessionStatsTracker::new(SessionStatsConfig::default());

        tracker.record_request(
            "session1".to_string(),
            100,
            200,
            1500,
            HashMap::new(),
            Duration::from_millis(5),
            Duration::from_millis(10),
        );

        tracker.record_request(
            "session1".to_string(),
            50,
            100,
            0,
            HashMap::new(),
            Duration::from_millis(3),
            Duration::from_millis(7),
        );

        let stats = tracker.get_session_stats("session1").unwrap();
        assert_eq!(stats.request_count, 2);
        assert_eq!(stats.input_tokens, 150);
        assert_eq!(stats.output_tokens, 300);
        assert_eq!(stats.thinking_tokens, 1500);
        assert_eq!(stats.thinking_requests, 1);
        assert_eq!(stats.pre_proxy_time, Duration::from_millis(8));
        assert_eq!(stats.post_proxy_time, Duration::from_millis(17));
    }

    #[test]
    fn test_tracker_max_sessions_eviction() {
        let config = SessionStatsConfig { max_sessions: 2 };
        let tracker = SessionStatsTracker::new(config);

        tracker.record_request("session1".to_string(), 100, 200, 0, HashMap::new(), Duration::from_millis(5), Duration::from_millis(10));
        tracker.record_request("session2".to_string(), 100, 200, 0, HashMap::new(), Duration::from_millis(5), Duration::from_millis(10));
        tracker.record_request("session3".to_string(), 100, 200, 0, HashMap::new(), Duration::from_millis(5), Duration::from_millis(10));

        // Should have evicted one session
        assert_eq!(tracker.session_count(), 2);
    }

    #[test]
    fn test_get_all_stats() {
        let tracker = SessionStatsTracker::new(SessionStatsConfig::default());

        tracker.record_request("session1".to_string(), 100, 200, 0, HashMap::new(), Duration::from_millis(5), Duration::from_millis(10));
        tracker.record_request("session2".to_string(), 50, 100, 0, HashMap::new(), Duration::from_millis(3), Duration::from_millis(7));

        let all_stats = tracker.get_all_stats();
        assert_eq!(all_stats.len(), 2);
        assert!(all_stats.contains_key("session1"));
        assert!(all_stats.contains_key("session2"));
    }

    #[test]
    fn test_tool_call_tracking() {
        let tracker = SessionStatsTracker::new(SessionStatsConfig::default());

        let mut tool_calls1 = HashMap::new();
        tool_calls1.insert("Read".to_string(), 3);
        tool_calls1.insert("Write".to_string(), 1);

        let mut tool_calls2 = HashMap::new();
        tool_calls2.insert("Read".to_string(), 2);
        tool_calls2.insert("Bash".to_string(), 1);

        tracker.record_request(
            "session1".to_string(),
            100,
            200,
            0,
            tool_calls1,
            Duration::from_millis(5),
            Duration::from_millis(10),
        );

        tracker.record_request(
            "session1".to_string(),
            50,
            100,
            0,
            tool_calls2,
            Duration::from_millis(3),
            Duration::from_millis(7),
        );

        let stats = tracker.get_session_stats("session1").unwrap();
        assert_eq!(stats.total_tool_calls, 7);
        assert_eq!(stats.tool_calls.get("Read"), Some(&5));
        assert_eq!(stats.tool_calls.get("Write"), Some(&1));
        assert_eq!(stats.tool_calls.get("Bash"), Some(&1));
    }
}
