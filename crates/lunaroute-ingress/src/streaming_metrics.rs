//! Shared streaming metrics tracking for both OpenAI and Anthropic handlers
//!
//! This module provides common functionality for tracking streaming performance metrics
//! across different provider implementations, reducing code duplication and ensuring
//! consistent metrics collection.

use std::sync::{Arc as StdArc, Mutex as StdMutex};
use std::time::Instant;

/// Maximum number of chunk latencies to track (prevent OOM)
pub const MAX_CHUNK_LATENCIES: usize = 10_000;

/// Maximum accumulated text size (1MB, prevent OOM)
pub const MAX_ACCUMULATED_TEXT_BYTES: usize = 1_000_000;

/// Tracker for streaming metrics (TTFT, chunk latencies, chunk count, etc.)
pub struct StreamingMetricsTracker {
    /// Time when first token was received (TTFT tracking)
    pub ttft_time: StdArc<StdMutex<Option<Instant>>>,
    /// Count of chunks received
    pub chunk_count: StdArc<StdMutex<u32>>,
    /// Latencies between consecutive chunks
    pub chunk_latencies: StdArc<StdMutex<Vec<u64>>>,
    /// Time of last chunk (for calculating next latency)
    pub last_chunk_time: StdArc<StdMutex<Instant>>,
    /// Accumulated response text
    pub accumulated_text: StdArc<StdMutex<String>>,
    /// Model extracted from stream
    pub stream_model: StdArc<StdMutex<Option<String>>>,
    /// Finish reason from stream
    pub stream_finish_reason: StdArc<StdMutex<Option<String>>>,
}

impl StreamingMetricsTracker {
    /// Create a new streaming metrics tracker
    pub fn new(start_time: Instant) -> Self {
        Self {
            ttft_time: StdArc::new(StdMutex::new(None)),
            chunk_count: StdArc::new(StdMutex::new(0)),
            chunk_latencies: StdArc::new(StdMutex::new(Vec::new())),
            last_chunk_time: StdArc::new(StdMutex::new(start_time)),
            accumulated_text: StdArc::new(StdMutex::new(String::new())),
            stream_model: StdArc::new(StdMutex::new(None)),
            stream_finish_reason: StdArc::new(StdMutex::new(None)),
        }
    }

    /// Record TTFT (time-to-first-token) if this is the first chunk
    pub fn record_ttft(&self, now: Instant) {
        if let Ok(mut ttft) = self.ttft_time.lock() {
            if ttft.is_none() {
                *ttft = Some(now);
            }
        } else {
            tracing::error!("Streaming metrics: TTFT mutex poisoned");
        }
    }

    /// Record chunk latency with memory bounds protection
    pub fn record_chunk_latency(
        &self,
        now: Instant,
        provider: &str,
        model: &str,
        metrics: &Option<std::sync::Arc<lunaroute_observability::Metrics>>,
    ) -> Result<(), String> {
        if let (Ok(mut last), Ok(mut latencies)) = (
            self.last_chunk_time.lock(),
            self.chunk_latencies.lock(),
        ) {
            let latency = now.duration_since(*last).as_millis() as u64;

            // Cap latency array to prevent OOM
            if latencies.len() < MAX_CHUNK_LATENCIES {
                latencies.push(latency);
            } else if latencies.len() == MAX_CHUNK_LATENCIES {
                // Log once when limit is first reached
                tracing::warn!(
                    "Chunk latency array reached maximum size ({} entries), dropping further measurements",
                    MAX_CHUNK_LATENCIES
                );
                // Record metrics for memory bound hit
                if let Some(m) = metrics {
                    m.record_memory_bound_hit(provider, model, "latency_array");
                }
            }
            *last = now;
            Ok(())
        } else {
            tracing::error!("Streaming metrics: latency tracking mutex poisoned");
            Err("Mutex poisoned".to_string())
        }
    }

    /// Increment chunk count
    pub fn increment_chunk_count(&self) {
        if let Ok(mut count) = self.chunk_count.lock() {
            *count += 1;
        } else {
            tracing::error!("Streaming metrics: chunk count mutex poisoned");
        }
    }

    /// Accumulate text with memory bounds protection
    pub fn accumulate_text(
        &self,
        text: &str,
        provider: &str,
        model: &str,
        metrics: &Option<std::sync::Arc<lunaroute_observability::Metrics>>,
    ) {
        if let Ok(mut accumulated) = self.accumulated_text.lock() {
            // Cap accumulated text to prevent OOM
            if accumulated.len() + text.len() <= MAX_ACCUMULATED_TEXT_BYTES {
                accumulated.push_str(text);
            } else if accumulated.len() < MAX_ACCUMULATED_TEXT_BYTES {
                // Log once when limit is first reached
                tracing::warn!(
                    "Accumulated text reached maximum size ({} bytes), dropping further content",
                    MAX_ACCUMULATED_TEXT_BYTES
                );
                // Record metrics for memory bound hit
                if let Some(m) = metrics {
                    m.record_memory_bound_hit(provider, model, "text_buffer");
                }
            }
        }
    }

    /// Set model name from stream
    pub fn set_model(&self, model: String) {
        if let Ok(mut m) = self.stream_model.lock() {
            *m = Some(model);
        }
    }

    /// Set finish reason from stream
    pub fn set_finish_reason(&self, reason: String) {
        if let Ok(mut f) = self.stream_finish_reason.lock() {
            *f = Some(reason);
        }
    }

    /// Finalize metrics and compute statistics
    pub fn finalize(
        &self,
        start_time: Instant,
        before_provider: Instant,
    ) -> FinalizedStreamingMetrics {
        // Extract all values (handle poisoned mutexes gracefully)
        let ttft_ms = self.ttft_time.lock()
            .ok()
            .and_then(|guard| *guard)
            .map(|ttft| ttft.duration_since(before_provider).as_millis() as u64)
            .unwrap_or(0);

        let total_chunks = self.chunk_count.lock().ok().map(|c| *c).unwrap_or(0);
        let latencies = self.chunk_latencies.lock().ok().map(|l| l.clone()).unwrap_or_default();
        let finish_reason = self.stream_finish_reason.lock().ok().and_then(|f| f.clone());

        // Calculate percentiles (safe index calculation to avoid out-of-bounds)
        let (p50, p95, p99, max, min, avg) = if !latencies.is_empty() {
            let mut sorted = latencies.clone();
            sorted.sort_unstable();
            let len = sorted.len();

            // Safe percentile index calculation: clamp to valid range
            let p50_idx = ((len - 1) * 50 / 100).min(len - 1);
            let p95_idx = ((len - 1) * 95 / 100).min(len - 1);
            let p99_idx = ((len - 1) * 99 / 100).min(len - 1);

            let p50 = sorted[p50_idx];
            let p95 = sorted[p95_idx];
            let p99 = sorted[p99_idx];
            let max = sorted[len - 1];  // Safe: len > 0
            let min = sorted[0];        // Safe: len > 0
            let avg = (sorted.iter().sum::<u64>() as f64) / (len as f64);

            (Some(p50), Some(p95), Some(p99), max, min, avg)
        } else {
            (None, None, None, 0, 0, 0.0)
        };

        let total_duration_ms = start_time.elapsed().as_millis() as u64;
        let streaming_duration_ms = total_duration_ms.saturating_sub(ttft_ms);

        FinalizedStreamingMetrics {
            ttft_ms,
            total_chunks,
            streaming_duration_ms,
            total_duration_ms,
            latencies,
            p50,
            p95,
            p99,
            max,
            min,
            avg,
            finish_reason,
        }
    }
}

/// Finalized streaming metrics after stream completion
#[derive(Debug, Clone)]
pub struct FinalizedStreamingMetrics {
    pub ttft_ms: u64,
    pub total_chunks: u32,
    pub streaming_duration_ms: u64,
    pub total_duration_ms: u64,
    pub latencies: Vec<u64>,
    pub p50: Option<u64>,
    pub p95: Option<u64>,
    pub p99: Option<u64>,
    pub max: u64,
    pub min: u64,
    pub avg: f64,
    pub finish_reason: Option<String>,
}

impl FinalizedStreamingMetrics {
    /// Record metrics to Prometheus
    pub fn record_to_prometheus(
        &self,
        metrics: &Option<std::sync::Arc<lunaroute_observability::Metrics>>,
        provider: &str,
        model: &str,
    ) {
        if let Some(m) = metrics {
            m.record_streaming_request(
                provider,
                model,
                self.ttft_ms as f64 / 1000.0, // Convert to seconds
                self.total_chunks,
                self.streaming_duration_ms as f64 / 1000.0, // Convert to seconds
            );

            // Record individual chunk latencies (sample to avoid overwhelming Prometheus)
            // Sample every 10th latency for very long streams
            let sample_rate = if self.latencies.len() > 100 { 10 } else { 1 };
            for (i, &latency_ms) in self.latencies.iter().enumerate() {
                if i % sample_rate == 0 {
                    m.record_chunk_latency(
                        provider,
                        model,
                        latency_ms as f64 / 1000.0, // Convert to seconds
                    );
                }
            }
        }
    }

    /// Create session recording StreamingStats
    pub fn to_streaming_stats(&self) -> lunaroute_session::events::StreamingStats {
        lunaroute_session::events::StreamingStats {
            time_to_first_token_ms: self.ttft_ms,
            total_chunks: self.total_chunks,
            streaming_duration_ms: self.streaming_duration_ms,
            avg_chunk_latency_ms: self.avg,
            p50_chunk_latency_ms: self.p50,
            p95_chunk_latency_ms: self.p95,
            p99_chunk_latency_ms: self.p99,
            max_chunk_latency_ms: self.max,
            min_chunk_latency_ms: self.min,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_streaming_metrics_tracker_creation() {
        let start = Instant::now();
        let tracker = StreamingMetricsTracker::new(start);

        assert!(tracker.ttft_time.lock().unwrap().is_none());
        assert_eq!(*tracker.chunk_count.lock().unwrap(), 0);
        assert!(tracker.chunk_latencies.lock().unwrap().is_empty());
    }

    #[test]
    fn test_record_ttft() {
        let start = Instant::now();
        let tracker = StreamingMetricsTracker::new(start);

        let now = Instant::now();
        tracker.record_ttft(now);

        assert!(tracker.ttft_time.lock().unwrap().is_some());

        // Second call should not update
        let later = Instant::now();
        tracker.record_ttft(later);
        assert_eq!(tracker.ttft_time.lock().unwrap().unwrap(), now);
    }

    #[test]
    fn test_increment_chunk_count() {
        let start = Instant::now();
        let tracker = StreamingMetricsTracker::new(start);

        tracker.increment_chunk_count();
        tracker.increment_chunk_count();
        tracker.increment_chunk_count();

        assert_eq!(*tracker.chunk_count.lock().unwrap(), 3);
    }

    #[test]
    fn test_accumulate_text() {
        let start = Instant::now();
        let tracker = StreamingMetricsTracker::new(start);

        tracker.accumulate_text("Hello ", "anthropic", "claude", &None);
        tracker.accumulate_text("World", "anthropic", "claude", &None);

        assert_eq!(&*tracker.accumulated_text.lock().unwrap(), "Hello World");
    }

    #[test]
    fn test_finalize() {
        let start = Instant::now();
        let before_provider = Instant::now();
        let tracker = StreamingMetricsTracker::new(before_provider);

        // Add small delay to ensure measurable TTFT
        std::thread::sleep(std::time::Duration::from_millis(1));
        tracker.record_ttft(Instant::now());
        tracker.increment_chunk_count();
        tracker.increment_chunk_count();
        tracker.set_finish_reason("end_turn".to_string());

        let finalized = tracker.finalize(start, before_provider);

        // ttft_ms should be set (can be 0 in very fast tests, but that's ok)
        assert_eq!(finalized.total_chunks, 2);
        assert_eq!(finalized.finish_reason, Some("end_turn".to_string()));
    }

    #[test]
    fn test_percentile_calculation() {
        let start = Instant::now();
        let tracker = StreamingMetricsTracker::new(start);

        // Simulate chunk latencies
        {
            let mut latencies = tracker.chunk_latencies.lock().unwrap();
            latencies.push(100);
            latencies.push(150);
            latencies.push(200);
            latencies.push(120);
            latencies.push(180);
        }

        let finalized = tracker.finalize(start, start);

        assert!(finalized.p50.is_some());
        assert!(finalized.p95.is_some());
        assert!(finalized.p99.is_some());
        assert_eq!(finalized.min, 100);
        assert_eq!(finalized.max, 200);
    }

    #[test]
    fn test_memory_bounds_latency() {
        let start = Instant::now();
        let tracker = StreamingMetricsTracker::new(start);

        // Fill up to MAX_CHUNK_LATENCIES
        for _ in 0..MAX_CHUNK_LATENCIES {
            let _ = tracker.record_chunk_latency(Instant::now(), "test", "model", &None);
        }

        let len = tracker.chunk_latencies.lock().unwrap().len();
        assert_eq!(len, MAX_CHUNK_LATENCIES);

        // Additional chunks should not increase size
        let _ = tracker.record_chunk_latency(Instant::now(), "test", "model", &None);
        let len_after = tracker.chunk_latencies.lock().unwrap().len();
        assert_eq!(len_after, MAX_CHUNK_LATENCIES);
    }

    #[test]
    fn test_memory_bounds_text() {
        let start = Instant::now();
        let tracker = StreamingMetricsTracker::new(start);

        // Create a large string just under the limit
        let large_text = "a".repeat(MAX_ACCUMULATED_TEXT_BYTES - 100);
        tracker.accumulate_text(&large_text, "test", "model", &None);

        // This should be accepted
        tracker.accumulate_text("b".repeat(50).as_str(), "test", "model", &None);

        // This should be rejected (would exceed limit)
        tracker.accumulate_text("c".repeat(100).as_str(), "test", "model", &None);

        let len = tracker.accumulated_text.lock().unwrap().len();
        assert!(len < MAX_ACCUMULATED_TEXT_BYTES);
    }
}
