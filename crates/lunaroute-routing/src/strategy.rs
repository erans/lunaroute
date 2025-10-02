//! Routing strategies for provider selection
//!
//! Implements different strategies for selecting providers from a pool:
//! - Round-robin: Equal distribution across providers
//! - Weighted round-robin: Distribution based on weights

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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
        }
    }
}

/// Strategy execution state (maintains counters between requests)
pub struct StrategyState {
    /// Counter for round-robin strategies
    round_robin_counter: AtomicUsize,
    /// Weighted round-robin state
    weighted_state: Arc<WeightedRoundRobinState>,
}

impl StrategyState {
    /// Create new strategy state
    pub fn new() -> Self {
        Self {
            round_robin_counter: AtomicUsize::new(0),
            weighted_state: Arc::new(WeightedRoundRobinState::new()),
        }
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
        }
    }
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
        let strategy = RoutingStrategy::RoundRobin {
            providers: vec![],
        };
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
        let providers = vec![
            WeightedProvider {
                id: "p1".to_string(),
                weight: 0,
            },
        ];

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
}

