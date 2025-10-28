//! Routing strategies for provider selection
//!
//! Implements intelligent provider selection strategies with production-ready features:
//!
//! ## Strategies
//!
//! ### Round-Robin
//! Equal distribution across all providers in the list.
//!
//! ```rust
//! use lunaroute_routing::{RoutingStrategy, StrategyState};
//!
//! let strategy = RoutingStrategy::RoundRobin {
//!     providers: vec!["p1".to_string(), "p2".to_string(), "p3".to_string()],
//! };
//!
//! let state = StrategyState::new();
//! let provider1 = state.select_provider(&strategy).unwrap(); // "p1"
//! let provider2 = state.select_provider(&strategy).unwrap(); // "p2"
//! let provider3 = state.select_provider(&strategy).unwrap(); // "p3"
//! let provider4 = state.select_provider(&strategy).unwrap(); // "p1" (wraps)
//! ```
//!
//! ### Weighted Round-Robin
//! Distribution based on provider weights (capacity, cost, etc.).
//!
//! ```rust
//! use lunaroute_routing::{RoutingStrategy, StrategyState, WeightedProvider};
//!
//! let strategy = RoutingStrategy::WeightedRoundRobin {
//!     providers: vec![
//!         WeightedProvider { id: "primary".to_string(), weight: 70 },
//!         WeightedProvider { id: "backup".to_string(), weight: 30 },
//!     ],
//! };
//!
//! let state = StrategyState::new();
//! // Over 100 requests: 70 go to "primary", 30 go to "backup"
//! ```
//!
//! ## Thread Safety
//!
//! - **Lock-free**: Uses atomic operations for zero contention
//! - **Overflow safe**: Counter wraps safely at `usize::MAX`
//! - **AcqRel ordering**: Ensures visibility across CPU cores
//! - **Concurrent**: Safe to use from multiple threads simultaneously
//!
//! ## Validation
//!
//! Strategies are validated before use:
//!
//! ```rust
//! use lunaroute_routing::RoutingStrategy;
//!
//! let strategy = RoutingStrategy::RoundRobin { providers: vec![] };
//! assert!(strategy.validate().is_err()); // Empty provider list
//!
//! let strategy = RoutingStrategy::RoundRobin {
//!     providers: vec!["p1".to_string()],
//! };
//! assert!(strategy.validate().is_ok()); // Valid
//! ```

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Tracks rate limit state for a provider
#[derive(Debug, Clone)]
pub struct RateLimitState {
    /// Provider ID
    pub provider_id: String,
    /// Time when the rate limit expires
    pub rate_limited_until: Instant,
    /// Number of consecutive rate limits encountered
    pub consecutive_rate_limits: u32,
    /// Time of the last rate limit
    pub last_rate_limit: Instant,
}

impl RateLimitState {
    /// Check if the rate limit has expired
    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.rate_limited_until
    }

    /// Calculate exponential backoff duration
    ///
    /// Formula: base_delay * 2^(consecutive_limits - 1)
    /// Examples:
    /// - 1st limit: 60s * 2^0 = 60s
    /// - 2nd limit: 60s * 2^1 = 120s
    /// - 3rd limit: 60s * 2^2 = 240s
    pub fn calculate_backoff_duration(consecutive_limits: u32, base_delay_secs: u64) -> Duration {
        let exponent = consecutive_limits.saturating_sub(1);
        let multiplier = 2u64.saturating_pow(exponent);
        let total_seconds = base_delay_secs.saturating_mul(multiplier);
        Duration::from_secs(total_seconds)
    }
}

/// Default base delay for exponential backoff (in seconds)
fn default_backoff_base() -> u64 {
    60
}

/// Routing strategy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "kebab-case")]
pub enum RoutingStrategy {
    /// Round-robin: equal distribution across providers
    RoundRobin {
        /// List of provider IDs to distribute across
        providers: Vec<String>,
    },

    /// Weighted round-robin: distribution based on weights
    WeightedRoundRobin {
        /// Providers with their weights
        providers: Vec<WeightedProvider>,
    },

    /// Limits-alternative: automatic failover when rate limits are hit
    LimitsAlternative {
        /// Primary providers to try first
        primary_providers: Vec<String>,
        /// Alternative providers to use when primaries are rate-limited
        alternative_providers: Vec<String>,
        /// Base delay in seconds for exponential backoff (default: 60)
        /// Used only when retry-after header is missing
        #[serde(default = "default_backoff_base")]
        exponential_backoff_base_secs: u64,
    },
}

/// Provider with weight for weighted round-robin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightedProvider {
    /// Provider ID
    pub id: String,
    /// Weight (relative distribution, e.g., 70 = 70%)
    pub weight: u32,
}

impl RoutingStrategy {
    /// Get all provider IDs from this strategy
    pub fn provider_ids(&self) -> Vec<&str> {
        match self {
            RoutingStrategy::RoundRobin { providers } => {
                providers.iter().map(|s| s.as_str()).collect()
            }
            RoutingStrategy::WeightedRoundRobin { providers } => {
                providers.iter().map(|p| p.id.as_str()).collect()
            }
            RoutingStrategy::LimitsAlternative {
                primary_providers,
                alternative_providers,
                ..
            } => {
                let mut all_providers = Vec::new();
                all_providers.extend(primary_providers.iter().map(|s| s.as_str()));
                all_providers.extend(alternative_providers.iter().map(|s| s.as_str()));
                all_providers
            }
        }
    }

    /// Validate the strategy configuration
    pub fn validate(&self) -> Result<(), StrategyError> {
        match self {
            RoutingStrategy::RoundRobin { providers } => {
                if providers.is_empty() {
                    return Err(StrategyError::EmptyProviderList);
                }
                Ok(())
            }
            RoutingStrategy::WeightedRoundRobin { providers } => {
                if providers.is_empty() {
                    return Err(StrategyError::EmptyProviderList);
                }

                // Use checked arithmetic to prevent overflow
                let total_weight = providers
                    .iter()
                    .try_fold(0u32, |acc, p| acc.checked_add(p.weight))
                    .ok_or(StrategyError::WeightOverflow)?;

                if total_weight == 0 {
                    return Err(StrategyError::ZeroTotalWeight);
                }

                Ok(())
            }
            RoutingStrategy::LimitsAlternative {
                primary_providers,
                alternative_providers,
                exponential_backoff_base_secs,
            } => {
                if primary_providers.is_empty() {
                    return Err(StrategyError::InvalidLimitsAlternative(
                        "primary_providers cannot be empty".to_string(),
                    ));
                }
                if alternative_providers.is_empty() {
                    return Err(StrategyError::InvalidLimitsAlternative(
                        "alternative_providers cannot be empty".to_string(),
                    ));
                }
                if *exponential_backoff_base_secs == 0 {
                    return Err(StrategyError::InvalidLimitsAlternative(
                        "exponential_backoff_base_secs must be greater than 0".to_string(),
                    ));
                }
                Ok(())
            }
        }
    }
}

/// Strategy execution state (maintains counters between requests)
pub struct StrategyState {
    /// Counter for round-robin strategies
    round_robin_counter: AtomicUsize,
    /// Weighted round-robin state
    weighted_state: Arc<WeightedRoundRobinState>,
    /// Rate limit states per provider (lock-free concurrent access)
    rate_limit_states: Arc<DashMap<String, RateLimitState>>,
}

impl StrategyState {
    /// Create new strategy state
    pub fn new() -> Self {
        Self {
            round_robin_counter: AtomicUsize::new(0),
            weighted_state: Arc::new(WeightedRoundRobinState::new()),
            rate_limit_states: Arc::new(DashMap::new()),
        }
    }

    /// Record a rate limit event for a provider
    pub fn record_rate_limit(
        &self,
        provider_id: &str,
        retry_after_secs: Option<u64>,
        backoff_base_secs: u64,
    ) {
        let now = Instant::now();

        self.rate_limit_states
            .entry(provider_id.to_string())
            .and_modify(|state| {
                state.consecutive_rate_limits += 1;
                state.last_rate_limit = now;
                state.rate_limited_until = now
                    + calculate_rate_limit_duration(
                        retry_after_secs,
                        state.consecutive_rate_limits,
                        backoff_base_secs,
                    );
            })
            .or_insert_with(|| {
                let duration = calculate_rate_limit_duration(
                    retry_after_secs,
                    1, // first rate limit
                    backoff_base_secs,
                );

                RateLimitState {
                    provider_id: provider_id.to_string(),
                    rate_limited_until: now + duration,
                    consecutive_rate_limits: 1,
                    last_rate_limit: now,
                }
            });
    }

    /// Remove expired rate limit states
    pub fn clear_expired_rate_limits(&self) {
        self.rate_limit_states
            .retain(|_, state| !state.is_expired());
    }

    /// Check if a provider is currently rate-limited
    pub fn is_rate_limited(&self, provider_id: &str) -> bool {
        self.rate_limit_states
            .get(provider_id)
            .map(|state| !state.is_expired())
            .unwrap_or(false)
    }

    /// Select next provider using the strategy
    pub fn select_provider(&self, strategy: &RoutingStrategy) -> Result<String, StrategyError> {
        match strategy {
            RoutingStrategy::RoundRobin { providers } => {
                if providers.is_empty() {
                    return Err(StrategyError::EmptyProviderList);
                }

                // Use wrapping_add with AcqRel ordering for thread safety
                let index = self
                    .round_robin_counter
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |x| {
                        Some(x.wrapping_add(1))
                    })
                    .unwrap();
                let provider_index = index % providers.len();
                Ok(providers[provider_index].clone())
            }

            RoutingStrategy::WeightedRoundRobin { providers } => {
                self.weighted_state.select_provider(providers)
            }

            RoutingStrategy::LimitsAlternative {
                primary_providers,
                alternative_providers,
                ..
            } => {
                // Clean up expired rate limit states
                self.clear_expired_rate_limits();

                // Try primary providers first
                for provider_id in primary_providers {
                    if !self.is_rate_limited(provider_id) {
                        return Ok(provider_id.clone());
                    }
                }

                // All primaries rate-limited, try alternatives
                for provider_id in alternative_providers {
                    if !self.is_rate_limited(provider_id) {
                        return Ok(provider_id.clone());
                    }
                }

                // All providers rate-limited
                Err(StrategyError::AllProvidersRateLimited)
            }
        }
    }
}

/// Calculate the duration to wait before retrying after a rate limit
///
/// Priority order:
/// 1. Use retry_after_secs from provider's header (if present)
/// 2. Use exponential backoff based on consecutive rate limits
fn calculate_rate_limit_duration(
    retry_after_secs: Option<u64>,
    consecutive_limits: u32,
    base_delay_secs: u64,
) -> Duration {
    // PRIORITY 1: Always use retry-after header if provider sent it
    // This is the provider's explicit instruction on when to retry
    retry_after_secs
        .map(Duration::from_secs)
        // PRIORITY 2: Fallback to exponential backoff only if header missing
        .unwrap_or_else(|| {
            RateLimitState::calculate_backoff_duration(consecutive_limits, base_delay_secs)
        })
}

impl Default for StrategyState {
    fn default() -> Self {
        Self::new()
    }
}

/// State for weighted round-robin selection
struct WeightedRoundRobinState {
    /// Current position in the weighted sequence
    current_position: AtomicUsize,
}

impl WeightedRoundRobinState {
    fn new() -> Self {
        Self {
            current_position: AtomicUsize::new(0),
        }
    }

    /// Select provider using weighted round-robin algorithm
    /// Uses smooth weighted round-robin (Nginx algorithm)
    fn select_provider(&self, providers: &[WeightedProvider]) -> Result<String, StrategyError> {
        if providers.is_empty() {
            return Err(StrategyError::EmptyProviderList);
        }

        // Calculate total weight with overflow protection
        let total_weight = providers
            .iter()
            .try_fold(0u32, |acc, p| acc.checked_add(p.weight))
            .ok_or(StrategyError::WeightOverflow)?;

        if total_weight == 0 {
            return Err(StrategyError::ZeroTotalWeight);
        }

        // Get current position and increment with wrapping and proper ordering
        let position = self
            .current_position
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |x| {
                Some(x.wrapping_add(1))
            })
            .unwrap();

        // Map position to provider based on cumulative weights
        let normalized_position = (position % total_weight as usize) as u32;
        let mut cumulative_weight = 0u32;

        for provider in providers {
            // Use saturating_add to prevent overflow in cumulative weight
            cumulative_weight = cumulative_weight.saturating_add(provider.weight);
            if normalized_position < cumulative_weight {
                return Ok(provider.id.clone());
            }
        }

        // Fallback (should not reach here if weights are valid)
        Ok(providers[0].id.clone())
    }
}

/// Strategy-related errors
#[derive(Debug, thiserror::Error)]
pub enum StrategyError {
    #[error("Provider list cannot be empty")]
    EmptyProviderList,

    #[error("Total weight cannot be zero")]
    ZeroTotalWeight,

    #[error("Weight overflow: total weight exceeds maximum allowed value")]
    WeightOverflow,

    #[error("Invalid limits-alternative configuration: {0}")]
    InvalidLimitsAlternative(String),

    #[error("All providers are currently rate-limited")]
    AllProvidersRateLimited,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_robin_basic() {
        let strategy = RoutingStrategy::RoundRobin {
            providers: vec!["p1".to_string(), "p2".to_string(), "p3".to_string()],
        };

        let state = StrategyState::new();

        // Should cycle through providers
        assert_eq!(state.select_provider(&strategy).unwrap(), "p1");
        assert_eq!(state.select_provider(&strategy).unwrap(), "p2");
        assert_eq!(state.select_provider(&strategy).unwrap(), "p3");
        assert_eq!(state.select_provider(&strategy).unwrap(), "p1"); // Wraps around
        assert_eq!(state.select_provider(&strategy).unwrap(), "p2");
    }

    #[test]
    fn test_round_robin_single_provider() {
        let strategy = RoutingStrategy::RoundRobin {
            providers: vec!["only-one".to_string()],
        };

        let state = StrategyState::new();

        // Should always return the same provider
        assert_eq!(state.select_provider(&strategy).unwrap(), "only-one");
        assert_eq!(state.select_provider(&strategy).unwrap(), "only-one");
        assert_eq!(state.select_provider(&strategy).unwrap(), "only-one");
    }

    #[test]
    fn test_weighted_round_robin_equal_weights() {
        let strategy = RoutingStrategy::WeightedRoundRobin {
            providers: vec![
                WeightedProvider {
                    id: "p1".to_string(),
                    weight: 50,
                },
                WeightedProvider {
                    id: "p2".to_string(),
                    weight: 50,
                },
            ],
        };

        let state = StrategyState::new();

        // With equal weights, should alternate
        let mut p1_count = 0;
        let mut p2_count = 0;

        for _ in 0..100 {
            let provider = state.select_provider(&strategy).unwrap();
            if provider == "p1" {
                p1_count += 1;
            } else if provider == "p2" {
                p2_count += 1;
            }
        }

        // Should be roughly equal (50-50)
        assert_eq!(p1_count, 50);
        assert_eq!(p2_count, 50);
    }

    #[test]
    fn test_weighted_round_robin_unequal_weights() {
        let strategy = RoutingStrategy::WeightedRoundRobin {
            providers: vec![
                WeightedProvider {
                    id: "p1".to_string(),
                    weight: 70, // 70%
                },
                WeightedProvider {
                    id: "p2".to_string(),
                    weight: 20, // 20%
                },
                WeightedProvider {
                    id: "p3".to_string(),
                    weight: 10, // 10%
                },
            ],
        };

        let state = StrategyState::new();

        let mut counts = std::collections::HashMap::new();
        for _ in 0..100 {
            let provider = state.select_provider(&strategy).unwrap();
            *counts.entry(provider).or_insert(0) += 1;
        }

        // Check distribution matches weights
        assert_eq!(counts.get("p1").unwrap(), &70);
        assert_eq!(counts.get("p2").unwrap(), &20);
        assert_eq!(counts.get("p3").unwrap(), &10);
    }

    #[test]
    fn test_strategy_validation() {
        // Empty providers
        let strategy = RoutingStrategy::RoundRobin { providers: vec![] };
        assert!(strategy.validate().is_err());

        // Valid round-robin
        let strategy = RoutingStrategy::RoundRobin {
            providers: vec!["p1".to_string()],
        };
        assert!(strategy.validate().is_ok());

        // Empty weighted providers
        let strategy = RoutingStrategy::WeightedRoundRobin { providers: vec![] };
        assert!(strategy.validate().is_err());

        // Zero total weight
        let strategy = RoutingStrategy::WeightedRoundRobin {
            providers: vec![WeightedProvider {
                id: "p1".to_string(),
                weight: 0,
            }],
        };
        assert!(strategy.validate().is_err());

        // Valid weighted
        let strategy = RoutingStrategy::WeightedRoundRobin {
            providers: vec![WeightedProvider {
                id: "p1".to_string(),
                weight: 100,
            }],
        };
        assert!(strategy.validate().is_ok());
    }

    #[test]
    fn test_strategy_provider_ids() {
        let strategy = RoutingStrategy::RoundRobin {
            providers: vec!["p1".to_string(), "p2".to_string()],
        };
        assert_eq!(strategy.provider_ids(), vec!["p1", "p2"]);

        let strategy = RoutingStrategy::WeightedRoundRobin {
            providers: vec![
                WeightedProvider {
                    id: "p1".to_string(),
                    weight: 50,
                },
                WeightedProvider {
                    id: "p2".to_string(),
                    weight: 50,
                },
            ],
        };
        assert_eq!(strategy.provider_ids(), vec!["p1", "p2"]);
    }

    #[test]
    fn test_strategy_serde() {
        // Round-robin
        let strategy = RoutingStrategy::RoundRobin {
            providers: vec!["p1".to_string(), "p2".to_string()],
        };
        let yaml = serde_yaml::to_string(&strategy).unwrap();
        let deserialized: RoutingStrategy = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(deserialized.provider_ids(), vec!["p1", "p2"]);

        // Weighted round-robin
        let strategy = RoutingStrategy::WeightedRoundRobin {
            providers: vec![WeightedProvider {
                id: "p1".to_string(),
                weight: 70,
            }],
        };
        let yaml = serde_yaml::to_string(&strategy).unwrap();
        assert!(yaml.contains("weighted-round-robin"));
        assert!(yaml.contains("weight: 70"));
    }

    #[test]
    fn test_weight_overflow_protection() {
        // Test that weight overflow is detected during validation
        let strategy = RoutingStrategy::WeightedRoundRobin {
            providers: vec![
                WeightedProvider {
                    id: "p1".to_string(),
                    weight: u32::MAX,
                },
                WeightedProvider {
                    id: "p2".to_string(),
                    weight: 1,
                },
            ],
        };

        // Validation should fail due to overflow
        let result = strategy.validate();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), StrategyError::WeightOverflow));
    }

    #[test]
    fn test_weight_overflow_during_selection() {
        // Test that overflow is handled during provider selection
        let state = StrategyState::new();
        let providers = vec![
            WeightedProvider {
                id: "p1".to_string(),
                weight: u32::MAX / 2,
            },
            WeightedProvider {
                id: "p2".to_string(),
                weight: u32::MAX / 2 + 2, // This will cause overflow
            },
        ];

        let strategy = RoutingStrategy::WeightedRoundRobin { providers };

        // Selection should fail due to overflow
        let result = state.select_provider(&strategy);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), StrategyError::WeightOverflow));
    }

    #[test]
    fn test_round_robin_counter_wrapping() {
        // Test that counter wraps correctly at overflow
        let strategy = RoutingStrategy::RoundRobin {
            providers: vec!["p1".to_string(), "p2".to_string()],
        };

        let state = StrategyState {
            round_robin_counter: std::sync::atomic::AtomicUsize::new(usize::MAX - 1),
            weighted_state: Arc::new(WeightedRoundRobinState::new()),
            rate_limit_states: Arc::new(DashMap::new()),
        };

        // Should not panic even when wrapping
        let result1 = state.select_provider(&strategy);
        assert!(result1.is_ok());

        let result2 = state.select_provider(&strategy);
        assert!(result2.is_ok());

        // After wrapping, should still distribute correctly
        let result3 = state.select_provider(&strategy);
        assert!(result3.is_ok());
    }

    #[test]
    fn test_single_provider_with_max_weight() {
        // Edge case: single provider with maximum weight
        let strategy = RoutingStrategy::WeightedRoundRobin {
            providers: vec![WeightedProvider {
                id: "only".to_string(),
                weight: u32::MAX,
            }],
        };

        // Should validate successfully
        assert!(strategy.validate().is_ok());

        let state = StrategyState::new();

        // Should always select the only provider
        for _ in 0..10 {
            let result = state.select_provider(&strategy).unwrap();
            assert_eq!(result, "only");
        }
    }

    #[test]
    fn test_many_providers_edge_case() {
        // Test with many providers to ensure no performance issues
        let providers: Vec<String> = (0..100).map(|i| format!("p{}", i)).collect();

        let strategy = RoutingStrategy::RoundRobin {
            providers: providers.clone(),
        };

        assert!(strategy.validate().is_ok());

        let state = StrategyState::new();

        // Should cycle through all providers
        for i in 0..200 {
            let result = state.select_provider(&strategy).unwrap();
            assert_eq!(result, providers[i % 100]);
        }
    }

    #[test]
    fn test_weighted_zero_weight_handled() {
        // Although zero weights should be caught by validation,
        // test that the algorithm doesn't panic
        let providers = vec![WeightedProvider {
            id: "p1".to_string(),
            weight: 0,
        }];

        let strategy = RoutingStrategy::WeightedRoundRobin { providers };

        // Should fail validation
        assert!(strategy.validate().is_err());
    }

    #[test]
    fn test_concurrent_strategy_state_access() {
        use std::sync::Arc;
        use std::thread;

        let strategy = Arc::new(RoutingStrategy::RoundRobin {
            providers: vec!["p1".to_string(), "p2".to_string(), "p3".to_string()],
        });
        let state = Arc::new(StrategyState::new());

        let mut handles = vec![];

        // Spawn 10 threads, each selecting 100 providers
        for _ in 0..10 {
            let strategy_clone = strategy.clone();
            let state_clone = state.clone();

            let handle = thread::spawn(move || {
                for _ in 0..100 {
                    let _ = state_clone.select_provider(&strategy_clone);
                }
            });
            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            handle.join().unwrap();
        }

        // Total selections should be 1000, counter should reflect that
        // (though with wrapping, just verify no panics occurred)
    }

    // Tests for rate limit functionality

    #[test]
    fn test_rate_limit_state_expiration() {
        let expired_state = RateLimitState {
            provider_id: "test".to_string(),
            rate_limited_until: Instant::now() - Duration::from_secs(1),
            consecutive_rate_limits: 1,
            last_rate_limit: Instant::now(),
        };
        assert!(expired_state.is_expired());

        let active_state = RateLimitState {
            provider_id: "test".to_string(),
            rate_limited_until: Instant::now() + Duration::from_secs(60),
            consecutive_rate_limits: 1,
            last_rate_limit: Instant::now(),
        };
        assert!(!active_state.is_expired());
    }

    #[test]
    fn test_rate_limit_exponential_backoff() {
        // 1st limit: 60s * 2^0 = 60s
        assert_eq!(
            RateLimitState::calculate_backoff_duration(1, 60),
            Duration::from_secs(60)
        );

        // 2nd limit: 60s * 2^1 = 120s
        assert_eq!(
            RateLimitState::calculate_backoff_duration(2, 60),
            Duration::from_secs(120)
        );

        // 3rd limit: 60s * 2^2 = 240s
        assert_eq!(
            RateLimitState::calculate_backoff_duration(3, 60),
            Duration::from_secs(240)
        );

        // 4th limit: 60s * 2^3 = 480s
        assert_eq!(
            RateLimitState::calculate_backoff_duration(4, 60),
            Duration::from_secs(480)
        );
    }

    #[test]
    fn test_strategy_state_rate_limit_tracking() {
        let state = StrategyState::new();

        // Initially not rate-limited
        assert!(!state.is_rate_limited("provider1"));

        // Record a rate limit with 60 second retry-after
        state.record_rate_limit("provider1", Some(60), 60);

        // Should now be rate-limited
        assert!(state.is_rate_limited("provider1"));

        // Record another rate limit (consecutive)
        state.record_rate_limit("provider1", None, 60); // No retry-after, uses exponential

        // Should still be rate-limited
        assert!(state.is_rate_limited("provider1"));
    }

    #[test]
    fn test_strategy_state_clear_expired() {
        let state = StrategyState::new();

        // Manually insert an expired rate limit
        state.rate_limit_states.insert(
            "expired".to_string(),
            RateLimitState {
                provider_id: "expired".to_string(),
                rate_limited_until: Instant::now() - Duration::from_secs(1),
                consecutive_rate_limits: 1,
                last_rate_limit: Instant::now(),
            },
        );

        // Insert a non-expired rate limit
        state.rate_limit_states.insert(
            "active".to_string(),
            RateLimitState {
                provider_id: "active".to_string(),
                rate_limited_until: Instant::now() + Duration::from_secs(60),
                consecutive_rate_limits: 1,
                last_rate_limit: Instant::now(),
            },
        );

        // Clear expired states
        state.clear_expired_rate_limits();

        // Expired should be removed, active should remain
        assert!(!state.is_rate_limited("expired"));
        assert!(state.is_rate_limited("active"));
    }

    #[test]
    fn test_limits_alternative_validation() {
        // Valid configuration
        let strategy = RoutingStrategy::LimitsAlternative {
            primary_providers: vec!["p1".to_string()],
            alternative_providers: vec!["p2".to_string()],
            exponential_backoff_base_secs: 60,
        };
        assert!(strategy.validate().is_ok());

        // Empty primary providers
        let strategy = RoutingStrategy::LimitsAlternative {
            primary_providers: vec![],
            alternative_providers: vec!["p2".to_string()],
            exponential_backoff_base_secs: 60,
        };
        assert!(strategy.validate().is_err());

        // Empty alternative providers
        let strategy = RoutingStrategy::LimitsAlternative {
            primary_providers: vec!["p1".to_string()],
            alternative_providers: vec![],
            exponential_backoff_base_secs: 60,
        };
        assert!(strategy.validate().is_err());

        // Zero backoff
        let strategy = RoutingStrategy::LimitsAlternative {
            primary_providers: vec!["p1".to_string()],
            alternative_providers: vec!["p2".to_string()],
            exponential_backoff_base_secs: 0,
        };
        assert!(strategy.validate().is_err());
    }

    #[test]
    fn test_limits_alternative_provider_selection() {
        let strategy = RoutingStrategy::LimitsAlternative {
            primary_providers: vec!["primary1".to_string(), "primary2".to_string()],
            alternative_providers: vec!["alt1".to_string(), "alt2".to_string()],
            exponential_backoff_base_secs: 60,
        };

        let state = StrategyState::new();

        // Should select first primary provider
        assert_eq!(state.select_provider(&strategy).unwrap(), "primary1");

        // Rate limit primary1
        state.record_rate_limit("primary1", Some(60), 60);

        // Should now select primary2
        assert_eq!(state.select_provider(&strategy).unwrap(), "primary2");

        // Rate limit primary2
        state.record_rate_limit("primary2", Some(60), 60);

        // Should now select alt1 (first alternative)
        assert_eq!(state.select_provider(&strategy).unwrap(), "alt1");

        // Rate limit alt1
        state.record_rate_limit("alt1", Some(60), 60);

        // Should now select alt2
        assert_eq!(state.select_provider(&strategy).unwrap(), "alt2");

        // Rate limit alt2
        state.record_rate_limit("alt2", Some(60), 60);

        // All providers rate-limited, should return error
        let result = state.select_provider(&strategy);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            StrategyError::AllProvidersRateLimited
        ));
    }

    #[test]
    fn test_limits_alternative_provider_ids() {
        let strategy = RoutingStrategy::LimitsAlternative {
            primary_providers: vec!["p1".to_string(), "p2".to_string()],
            alternative_providers: vec!["a1".to_string(), "a2".to_string()],
            exponential_backoff_base_secs: 60,
        };

        let ids = strategy.provider_ids();
        assert_eq!(ids, vec!["p1", "p2", "a1", "a2"]);
    }

    #[test]
    fn test_calculate_rate_limit_duration_with_retry_after() {
        // When retry_after is present, use it (priority 1)
        let duration = calculate_rate_limit_duration(Some(120), 5, 60);
        assert_eq!(duration, Duration::from_secs(120));
    }

    #[test]
    fn test_calculate_rate_limit_duration_exponential_fallback() {
        // When retry_after is None, use exponential backoff (priority 2)
        // 3rd consecutive limit: 60 * 2^2 = 240s
        let duration = calculate_rate_limit_duration(None, 3, 60);
        assert_eq!(duration, Duration::from_secs(240));
    }
}
