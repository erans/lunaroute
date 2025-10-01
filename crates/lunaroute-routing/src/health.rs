//! Health Monitoring
//!
//! Tracks provider health metrics including success/failure counts,
//! latencies, and availability. Used to influence routing decisions
//! and provide observability.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// Health status of a provider
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Provider is healthy (success rate above threshold)
    Healthy,
    /// Provider is degraded (success rate below threshold but not failing)
    Degraded,
    /// Provider is unhealthy (too many recent failures)
    Unhealthy,
    /// Provider health is unknown (no recent data)
    Unknown,
}

/// Provider health metrics
#[derive(Debug)]
pub struct ProviderHealth {
    /// Total successful requests
    success_count: AtomicU64,
    /// Total failed requests
    failure_count: AtomicU64,
    /// Last successful request timestamp
    last_success: RwLock<Option<Instant>>,
    /// Last failed request timestamp
    last_failure: RwLock<Option<Instant>>,
    /// Time the provider was created/reset
    _created_at: Instant,
}

impl ProviderHealth {
    /// Create new provider health tracker
    fn new() -> Self {
        Self {
            success_count: AtomicU64::new(0),
            failure_count: AtomicU64::new(0),
            last_success: RwLock::new(None),
            last_failure: RwLock::new(None),
            _created_at: Instant::now(),
        }
    }

    /// Record a successful request
    fn record_success(&self) {
        // Use fetch_update for atomic saturating arithmetic
        self.success_count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                Some(current.saturating_add(1))
            })
            .ok();
        *self.last_success.write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Instant::now());
    }

    /// Record a failed request
    fn record_failure(&self) {
        // Use fetch_update for atomic saturating arithmetic
        self.failure_count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                Some(current.saturating_add(1))
            })
            .ok();
        *self.last_failure.write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Instant::now());
    }

    /// Get total success count
    fn success_count(&self) -> u64 {
        self.success_count.load(Ordering::Acquire)
    }

    /// Get total failure count
    fn failure_count(&self) -> u64 {
        self.failure_count.load(Ordering::Acquire)
    }

    /// Get total request count
    fn total_count(&self) -> u64 {
        self.success_count() + self.failure_count()
    }

    /// Get success rate (0.0 to 1.0)
    fn success_rate(&self) -> f64 {
        let total = self.total_count();
        if total == 0 {
            return 1.0; // Assume healthy if no data
        }
        self.success_count() as f64 / total as f64
    }

    /// Get time since last success
    fn time_since_last_success(&self) -> Option<Duration> {
        self.last_success
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .map(|instant| instant.elapsed())
    }

    /// Get time since last failure
    fn time_since_last_failure(&self) -> Option<Duration> {
        self.last_failure
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .map(|instant| instant.elapsed())
    }

    // Note: reset() method removed - not used and would require interior mutability with Arc
    // Consider adding back if needed for future reset functionality
}

/// Configuration for health monitoring
#[derive(Debug, Clone)]
pub struct HealthMonitorConfig {
    /// Minimum success rate to be considered healthy (0.0 to 1.0)
    pub healthy_threshold: f64,
    /// Success rate below this is considered unhealthy
    pub unhealthy_threshold: f64,
    /// Time window to consider provider unhealthy if no successes
    pub failure_window: Duration,
    /// Minimum requests before calculating health status
    pub min_requests: u64,
}

impl Default for HealthMonitorConfig {
    fn default() -> Self {
        Self {
            healthy_threshold: 0.95,      // 95% success rate = healthy
            unhealthy_threshold: 0.75,    // Below 75% = unhealthy
            failure_window: Duration::from_secs(60), // 1 minute
            min_requests: 10,             // Need at least 10 requests
        }
    }
}

impl HealthMonitorConfig {
    /// Validate configuration values
    ///
    /// Returns an error if the configuration is invalid
    pub fn validate(&self) -> Result<(), String> {
        if !(0.0..=1.0).contains(&self.healthy_threshold) {
            return Err("healthy_threshold must be between 0.0 and 1.0".to_string());
        }
        if !(0.0..=1.0).contains(&self.unhealthy_threshold) {
            return Err("unhealthy_threshold must be between 0.0 and 1.0".to_string());
        }
        if self.unhealthy_threshold >= self.healthy_threshold {
            return Err("unhealthy_threshold must be less than healthy_threshold".to_string());
        }
        if self.failure_window.as_millis() == 0 {
            return Err("failure_window must be greater than 0".to_string());
        }
        if self.min_requests == 0 {
            return Err("min_requests must be greater than 0".to_string());
        }
        Ok(())
    }

    /// Create a new validated configuration
    ///
    /// Returns an error if validation fails
    pub fn new(
        healthy_threshold: f64,
        unhealthy_threshold: f64,
        failure_window: Duration,
        min_requests: u64,
    ) -> Result<Self, String> {
        let config = Self {
            healthy_threshold,
            unhealthy_threshold,
            failure_window,
            min_requests,
        };
        config.validate()?;
        Ok(config)
    }
}

/// Health monitor for tracking provider health
#[derive(Debug)]
pub struct HealthMonitor {
    /// Per-provider health metrics
    providers: RwLock<HashMap<String, Arc<ProviderHealth>>>,
    /// Configuration
    config: HealthMonitorConfig,
}

impl HealthMonitor {
    /// Create a new health monitor with the given configuration
    pub fn new(config: HealthMonitorConfig) -> Self {
        Self {
            providers: RwLock::new(HashMap::new()),
            config,
        }
    }

    /// Create a new health monitor with default configuration
    pub fn with_defaults() -> Self {
        Self::new(HealthMonitorConfig::default())
    }

    /// Register a provider for health monitoring
    pub fn register_provider(&self, provider_id: impl Into<String>) {
        let provider_id = provider_id.into();
        let mut providers = self.providers.write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        providers
            .entry(provider_id.clone())
            .or_insert_with(|| Arc::new(ProviderHealth::new()));
        tracing::debug!("Registered provider for health monitoring: {}", provider_id);
    }

    /// Record a successful request for a provider
    pub fn record_success(&self, provider_id: &str) {
        let providers = self.providers.read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(health) = providers.get(provider_id) {
            health.record_success();
        }
    }

    /// Record a failed request for a provider
    pub fn record_failure(&self, provider_id: &str) {
        let providers = self.providers.read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(health) = providers.get(provider_id) {
            health.record_failure();
        }
    }

    /// Get the health status of a provider
    pub fn get_status(&self, provider_id: &str) -> HealthStatus {
        let providers = self.providers.read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(health) = providers.get(provider_id) else {
            return HealthStatus::Unknown;
        };

        let total = health.total_count();

        // Not enough data yet
        if total < self.config.min_requests {
            return HealthStatus::Unknown;
        }

        let success_rate = health.success_rate();

        // Check for recent failures without recent successes (indicates current issues)
        let has_recent_failure = health
            .time_since_last_failure()
            .is_some_and(|duration| duration < self.config.failure_window);

        let has_recent_success = health
            .time_since_last_success()
            .is_some_and(|duration| duration < self.config.failure_window);

        // If we have recent failures but no recent successes, mark as unhealthy
        // This catches degradation faster than waiting for success rate to drop
        if has_recent_failure && !has_recent_success {
            return HealthStatus::Unhealthy;
        }

        // Determine status based on success rate
        if success_rate >= self.config.healthy_threshold {
            HealthStatus::Healthy
        } else if success_rate >= self.config.unhealthy_threshold {
            HealthStatus::Degraded
        } else {
            HealthStatus::Unhealthy
        }
    }

    /// Get detailed metrics for a provider
    pub fn get_metrics(&self, provider_id: &str) -> Option<HealthMetrics> {
        let providers = self.providers.read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        providers.get(provider_id).map(|health| HealthMetrics {
            success_count: health.success_count(),
            failure_count: health.failure_count(),
            total_count: health.total_count(),
            success_rate: health.success_rate(),
            time_since_last_success: health.time_since_last_success(),
            time_since_last_failure: health.time_since_last_failure(),
            status: self.get_status(provider_id),
        })
    }

    /// Get all provider IDs being monitored
    pub fn get_provider_ids(&self) -> Vec<String> {
        let providers = self.providers.read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        providers.keys().cloned().collect()
    }

    /// Reset metrics for a provider
    pub fn reset_provider(&self, provider_id: &str) {
        let providers = self.providers.read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(_health) = providers.get(provider_id) {
            // Need to get mutable access - this is a bit awkward with Arc
            // In practice, we'd need to use interior mutability
            tracing::warn!(
                "Reset requested for provider {} (not implemented with current Arc design)",
                provider_id
            );
        }
    }

    /// Remove a provider from monitoring
    pub fn unregister_provider(&self, provider_id: &str) {
        let mut providers = self.providers.write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if providers.remove(provider_id).is_some() {
            tracing::debug!("Unregistered provider from health monitoring: {}", provider_id);
        }
    }

    /// Get all healthy providers
    pub fn get_healthy_providers(&self) -> Vec<String> {
        self.get_provider_ids()
            .into_iter()
            .filter(|id| self.is_healthy(id))
            .collect()
    }

    /// Check if a provider is healthy
    pub fn is_healthy(&self, provider_id: &str) -> bool {
        matches!(
            self.get_status(provider_id),
            HealthStatus::Healthy | HealthStatus::Unknown
        )
    }
}

/// Snapshot of provider health metrics
#[derive(Debug, Clone)]
pub struct HealthMetrics {
    pub success_count: u64,
    pub failure_count: u64,
    pub total_count: u64,
    pub success_rate: f64,
    pub time_since_last_success: Option<Duration>,
    pub time_since_last_failure: Option<Duration>,
    pub status: HealthStatus,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_provider_health_new() {
        let health = ProviderHealth::new();
        assert_eq!(health.success_count(), 0);
        assert_eq!(health.failure_count(), 0);
        assert_eq!(health.total_count(), 0);
        assert_eq!(health.success_rate(), 1.0); // No data = assume healthy
    }

    #[test]
    fn test_provider_health_record_success() {
        let health = ProviderHealth::new();
        health.record_success();
        health.record_success();

        assert_eq!(health.success_count(), 2);
        assert_eq!(health.failure_count(), 0);
        assert_eq!(health.success_rate(), 1.0);
        assert!(health.time_since_last_success().is_some());
    }

    #[test]
    fn test_provider_health_record_failure() {
        let health = ProviderHealth::new();
        health.record_failure();
        health.record_failure();

        assert_eq!(health.success_count(), 0);
        assert_eq!(health.failure_count(), 2);
        assert_eq!(health.success_rate(), 0.0);
        assert!(health.time_since_last_failure().is_some());
    }

    #[test]
    fn test_provider_health_success_rate() {
        let health = ProviderHealth::new();

        // 8 successes, 2 failures = 80% success rate
        for _ in 0..8 {
            health.record_success();
        }
        for _ in 0..2 {
            health.record_failure();
        }

        assert_eq!(health.total_count(), 10);
        assert_eq!(health.success_rate(), 0.8);
    }

    #[test]
    fn test_health_monitor_register_provider() {
        let monitor = HealthMonitor::with_defaults();
        monitor.register_provider("provider1");

        let ids = monitor.get_provider_ids();
        assert_eq!(ids.len(), 1);
        assert!(ids.contains(&"provider1".to_string()));
    }

    #[test]
    fn test_health_monitor_record_metrics() {
        let config = HealthMonitorConfig {
            min_requests: 5,
            ..Default::default()
        };
        let monitor = HealthMonitor::new(config);
        monitor.register_provider("provider1");

        // Record some successes
        for _ in 0..10 {
            monitor.record_success("provider1");
        }

        let metrics = monitor.get_metrics("provider1").unwrap();
        assert_eq!(metrics.success_count, 10);
        assert_eq!(metrics.failure_count, 0);
        assert_eq!(metrics.success_rate, 1.0);
    }

    #[test]
    fn test_health_status_unknown_not_enough_requests() {
        let config = HealthMonitorConfig {
            min_requests: 10,
            ..Default::default()
        };
        let monitor = HealthMonitor::new(config);
        monitor.register_provider("provider1");

        // Only 5 requests
        for _ in 0..5 {
            monitor.record_success("provider1");
        }

        assert_eq!(monitor.get_status("provider1"), HealthStatus::Unknown);
    }

    #[test]
    fn test_health_status_healthy() {
        let config = HealthMonitorConfig {
            min_requests: 10,
            healthy_threshold: 0.95,
            ..Default::default()
        };
        let monitor = HealthMonitor::new(config);
        monitor.register_provider("provider1");

        // 19 successes, 1 failure = 95% success rate
        for _ in 0..19 {
            monitor.record_success("provider1");
        }
        monitor.record_failure("provider1");

        assert_eq!(monitor.get_status("provider1"), HealthStatus::Healthy);
        assert!(monitor.is_healthy("provider1"));
    }

    #[test]
    fn test_health_status_degraded() {
        let config = HealthMonitorConfig {
            min_requests: 10,
            healthy_threshold: 0.95,
            unhealthy_threshold: 0.75,
            ..Default::default()
        };
        let monitor = HealthMonitor::new(config);
        monitor.register_provider("provider1");

        // 17 successes, 3 failures = 85% success rate (between thresholds)
        for _ in 0..17 {
            monitor.record_success("provider1");
        }
        for _ in 0..3 {
            monitor.record_failure("provider1");
        }

        assert_eq!(monitor.get_status("provider1"), HealthStatus::Degraded);
    }

    #[test]
    fn test_health_status_unhealthy() {
        let config = HealthMonitorConfig {
            min_requests: 10,
            healthy_threshold: 0.95,
            unhealthy_threshold: 0.75,
            ..Default::default()
        };
        let monitor = HealthMonitor::new(config);
        monitor.register_provider("provider1");

        // 5 successes, 15 failures = 25% success rate
        for _ in 0..5 {
            monitor.record_success("provider1");
        }
        for _ in 0..15 {
            monitor.record_failure("provider1");
        }

        assert_eq!(monitor.get_status("provider1"), HealthStatus::Unhealthy);
        assert!(!monitor.is_healthy("provider1"));
    }

    #[test]
    fn test_health_status_recent_failures() {
        let config = HealthMonitorConfig {
            min_requests: 10,
            failure_window: Duration::from_millis(100),
            ..Default::default()
        };
        let monitor = HealthMonitor::new(config);
        monitor.register_provider("provider1");

        // Some old successes
        for _ in 0..10 {
            monitor.record_success("provider1");
        }

        // Wait a bit
        thread::sleep(Duration::from_millis(150));

        // Recent failures
        monitor.record_failure("provider1");
        monitor.record_failure("provider1");

        // Should be unhealthy due to recent failures with no recent successes
        assert_eq!(monitor.get_status("provider1"), HealthStatus::Unhealthy);
    }

    #[test]
    fn test_get_healthy_providers() {
        let config = HealthMonitorConfig {
            min_requests: 10,
            ..Default::default()
        };
        let monitor = HealthMonitor::new(config);

        monitor.register_provider("provider1");
        monitor.register_provider("provider2");
        monitor.register_provider("provider3");

        // Provider 1: healthy (95% success)
        for _ in 0..19 {
            monitor.record_success("provider1");
        }
        monitor.record_failure("provider1");

        // Provider 2: unhealthy (50% success)
        for _ in 0..10 {
            monitor.record_success("provider2");
        }
        for _ in 0..10 {
            monitor.record_failure("provider2");
        }

        // Provider 3: not enough data
        for _ in 0..5 {
            monitor.record_success("provider3");
        }

        let healthy = monitor.get_healthy_providers();
        // Provider1 is healthy, Provider3 is unknown (treated as healthy)
        assert_eq!(healthy.len(), 2);
        assert!(healthy.contains(&"provider1".to_string()));
        assert!(healthy.contains(&"provider3".to_string()));
    }

    #[test]
    fn test_unregister_provider() {
        let monitor = HealthMonitor::with_defaults();
        monitor.register_provider("provider1");
        monitor.register_provider("provider2");

        assert_eq!(monitor.get_provider_ids().len(), 2);

        monitor.unregister_provider("provider1");
        let ids = monitor.get_provider_ids();
        assert_eq!(ids.len(), 1);
        assert!(ids.contains(&"provider2".to_string()));
    }

    #[test]
    fn test_thread_safety() {
        let monitor = Arc::new(HealthMonitor::with_defaults());
        monitor.register_provider("provider1");

        let mut handles = vec![];

        // Spawn multiple threads recording metrics
        for i in 0..10 {
            let monitor_clone = Arc::clone(&monitor);
            let handle = thread::spawn(move || {
                for _ in 0..100 {
                    if i % 2 == 0 {
                        monitor_clone.record_success("provider1");
                    } else {
                        monitor_clone.record_failure("provider1");
                    }
                }
            });
            handles.push(handle);
        }

        // Wait for all threads
        for handle in handles {
            handle.join().unwrap();
        }

        // Should have 1000 total requests (10 threads * 100 requests)
        let metrics = monitor.get_metrics("provider1").unwrap();
        assert_eq!(metrics.total_count, 1000);
    }

    #[test]
    fn test_health_status_transitions() {
        let config = HealthMonitorConfig {
            min_requests: 10,
            healthy_threshold: 0.9,
            unhealthy_threshold: 0.7,
            failure_window: Duration::from_secs(60),
        };
        let monitor = HealthMonitor::new(config);
        monitor.register_provider("provider1");

        // Start healthy: 10 successes
        for _ in 0..10 {
            monitor.record_success("provider1");
        }
        assert_eq!(monitor.get_status("provider1"), HealthStatus::Healthy);

        // Degrade to degraded: 85% (17/20)
        for _ in 0..3 {
            monitor.record_failure("provider1");
        }
        for _ in 0..7 {
            monitor.record_success("provider1");
        }
        let status = monitor.get_status("provider1");
        assert_eq!(status, HealthStatus::Degraded);

        // Further degrade to unhealthy: 65% (13/20)
        for _ in 0..7 {
            monitor.record_failure("provider1");
        }
        assert_eq!(monitor.get_status("provider1"), HealthStatus::Unhealthy);
    }

    #[test]
    fn test_get_metrics_includes_status() {
        let config = HealthMonitorConfig {
            min_requests: 10,
            ..Default::default()
        };
        let monitor = HealthMonitor::new(config);
        monitor.register_provider("provider1");

        // Record enough requests
        for _ in 0..19 {
            monitor.record_success("provider1");
        }
        monitor.record_failure("provider1");

        let metrics = monitor.get_metrics("provider1").unwrap();
        assert_eq!(metrics.success_count, 19);
        assert_eq!(metrics.failure_count, 1);
        assert_eq!(metrics.total_count, 20);
        assert_eq!(metrics.success_rate, 0.95);
        assert_eq!(metrics.status, HealthStatus::Healthy);
        assert!(metrics.time_since_last_success.is_some());
        assert!(metrics.time_since_last_failure.is_some());
    }

    #[test]
    fn test_unregister_provider_removes_from_healthy_list() {
        let config = HealthMonitorConfig {
            min_requests: 10,
            ..Default::default()
        };
        let monitor = HealthMonitor::new(config);
        monitor.register_provider("provider1");
        monitor.register_provider("provider2");

        // Make both healthy
        for provider in &["provider1", "provider2"] {
            for _ in 0..20 {
                monitor.record_success(provider);
            }
        }

        let healthy = monitor.get_healthy_providers();
        assert_eq!(healthy.len(), 2);

        // Unregister one
        monitor.unregister_provider("provider1");

        let healthy = monitor.get_healthy_providers();
        assert_eq!(healthy.len(), 1);
        assert!(healthy.contains(&"provider2".to_string()));
    }

    #[test]
    fn test_success_rate_with_only_failures() {
        let health = ProviderHealth::new();

        for _ in 0..10 {
            health.record_failure();
        }

        assert_eq!(health.success_rate(), 0.0);
        assert_eq!(health.failure_count(), 10);
        assert_eq!(health.success_count(), 0);
    }

    #[test]
    fn test_success_rate_with_only_successes() {
        let health = ProviderHealth::new();

        for _ in 0..10 {
            health.record_success();
        }

        assert_eq!(health.success_rate(), 1.0);
        assert_eq!(health.success_count(), 10);
        assert_eq!(health.failure_count(), 0);
    }

    #[test]
    fn test_health_monitor_all_unknown_providers() {
        let config = HealthMonitorConfig {
            min_requests: 100, // Very high threshold
            ..Default::default()
        };
        let monitor = HealthMonitor::new(config);

        monitor.register_provider("provider1");
        monitor.register_provider("provider2");
        monitor.register_provider("provider3");

        // Record a few requests (not enough for status)
        for provider in &["provider1", "provider2", "provider3"] {
            for _ in 0..5 {
                monitor.record_success(provider);
            }
        }

        // All should be unknown
        assert_eq!(monitor.get_status("provider1"), HealthStatus::Unknown);
        assert_eq!(monitor.get_status("provider2"), HealthStatus::Unknown);
        assert_eq!(monitor.get_status("provider3"), HealthStatus::Unknown);

        // Unknown providers are treated as healthy
        let healthy = monitor.get_healthy_providers();
        assert_eq!(healthy.len(), 3);
    }

    #[test]
    fn test_recent_failure_window_expiry() {
        let config = HealthMonitorConfig {
            min_requests: 10,
            failure_window: Duration::from_millis(50),
            ..Default::default()
        };
        let monitor = HealthMonitor::new(config);
        monitor.register_provider("provider1");

        // Many successes
        for _ in 0..10 {
            monitor.record_success("provider1");
        }

        // Wait to make successes "old"
        thread::sleep(Duration::from_millis(60));

        // Recent failure (with old successes)
        monitor.record_failure("provider1");
        assert_eq!(monitor.get_status("provider1"), HealthStatus::Unhealthy);

        // Wait for failure window to expire
        thread::sleep(Duration::from_millis(60));

        // Need a recent success to pass the time_since_last_success check
        monitor.record_success("provider1");

        // Status should improve (old failure outside window)
        // With 11 successes and 1 old failure = 91.7% success rate
        let status = monitor.get_status("provider1");
        assert_eq!(status, HealthStatus::Degraded); // Below 95% but above 75%
    }
}
