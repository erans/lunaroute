//! Router that implements the Provider trait
//!
//! The Router coordinates multiple providers using:
//! - RouteTable for intelligent routing decisions
//! - HealthMonitor for provider health tracking
//! - CircuitBreakers for automatic failover
//! - Fallback chains for resilience

use crate::{
    circuit_breaker::{CircuitBreaker, CircuitBreakerConfig},
    health::{HealthMonitor, HealthMonitorConfig},
    router::{RouteTable, RoutingContext},
    strategy::StrategyState,
};
use async_trait::async_trait;
use dashmap::DashMap;
use lunaroute_core::{
    error::{Error, Result},
    normalized::{NormalizedRequest, NormalizedResponse, NormalizedStreamEvent},
    provider::{Provider, ProviderCapabilities},
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio_stream::Stream;

/// Router that delegates to multiple providers based on routing rules
pub struct Router {
    /// Routing table with rules
    route_table: RouteTable,

    /// Map of provider ID to provider instance
    providers: HashMap<String, Arc<dyn Provider>>,

    /// Health monitor for all providers
    health_monitor: Arc<HealthMonitor>,

    /// Circuit breakers per provider (uses DashMap for lock-free concurrent access)
    circuit_breakers: DashMap<String, Arc<CircuitBreaker>>,

    /// Default circuit breaker config for new providers
    circuit_breaker_config: CircuitBreakerConfig,

    /// Strategy state for routing strategies (uses DashMap for lock-free concurrent access)
    strategy_states: DashMap<String, Arc<StrategyState>>,
}

impl Router {
    /// Create a new router
    pub fn new(
        route_table: RouteTable,
        providers: HashMap<String, Arc<dyn Provider>>,
        health_config: HealthMonitorConfig,
        circuit_breaker_config: CircuitBreakerConfig,
    ) -> Self {
        let health_monitor = Arc::new(HealthMonitor::new(health_config));

        // Register all providers with health monitor
        for provider_id in providers.keys() {
            health_monitor.register_provider(provider_id);
        }

        Self {
            route_table,
            providers,
            health_monitor,
            circuit_breakers: DashMap::new(),
            circuit_breaker_config,
            strategy_states: DashMap::new(),
        }
    }

    /// Create a router with default configurations
    pub fn with_defaults(
        route_table: RouteTable,
        providers: HashMap<String, Arc<dyn Provider>>,
    ) -> Self {
        Self::new(
            route_table,
            providers,
            HealthMonitorConfig::default(),
            CircuitBreakerConfig::default(),
        )
    }

    /// Get health metrics for a provider
    pub fn get_health_metrics(&self, provider_id: &str) -> Option<crate::health::HealthMetrics> {
        self.health_monitor.get_metrics(provider_id)
    }

    /// Get health status for a provider
    pub fn get_health_status(&self, provider_id: &str) -> crate::health::HealthStatus {
        self.health_monitor.get_status(provider_id)
    }

    /// Get or create circuit breaker for a provider (lock-free with DashMap)
    fn get_circuit_breaker(&self, provider_id: &str) -> Arc<CircuitBreaker> {
        self.circuit_breakers
            .entry(provider_id.to_string())
            .or_insert_with(|| Arc::new(CircuitBreaker::new(self.circuit_breaker_config.clone())))
            .clone()
    }

    /// Get or create strategy state for a rule (lock-free with DashMap)
    fn get_strategy_state(&self, rule_name: &str) -> Arc<StrategyState> {
        self.strategy_states
            .entry(rule_name.to_string())
            .or_insert_with(|| Arc::new(StrategyState::new()))
            .clone()
    }

    /// Select provider using strategy
    fn select_provider_from_strategy(
        &self,
        strategy: &crate::strategy::RoutingStrategy,
        rule_name: &str,
    ) -> Result<String> {
        let state = self.get_strategy_state(rule_name);
        state
            .select_provider(strategy)
            .map_err(|e| Error::Provider(format!("Strategy selection failed: {}", e)))
    }

    /// Try to send request to a provider, respecting circuit breaker
    async fn try_provider(
        &self,
        provider_id: &str,
        request: &NormalizedRequest,
    ) -> Result<NormalizedResponse> {
        let circuit_breaker = self.get_circuit_breaker(provider_id);

        // Check circuit breaker
        if !circuit_breaker.allow_request() {
            tracing::warn!(
                provider = provider_id,
                state = ?circuit_breaker.state(),
                "Circuit breaker is open, skipping provider"
            );
            return Err(Error::Provider(format!(
                "Circuit breaker open for provider '{}'",
                provider_id
            )));
        }

        let provider = self
            .providers
            .get(provider_id)
            .ok_or_else(|| Error::Provider(format!("Provider '{}' not found", provider_id)))?;

        tracing::debug!(
            provider = provider_id,
            model = %request.model,
            "Attempting request to provider"
        );

        match provider.send(request.clone()).await {
            Ok(response) => {
                // Record success
                circuit_breaker.record_success();
                self.health_monitor.record_success(provider_id);

                tracing::info!(
                    provider = provider_id,
                    model = %request.model,
                    tokens = response.usage.total_tokens,
                    "Request succeeded"
                );

                Ok(response)
            }
            Err(err) => {
                // Record failure
                circuit_breaker.record_failure();
                self.health_monitor.record_failure(provider_id);

                tracing::warn!(
                    provider = provider_id,
                    model = %request.model,
                    error = %err,
                    "Request failed"
                );

                Err(err)
            }
        }
    }
}

#[async_trait]
impl Provider for Router {
    async fn send(&self, request: NormalizedRequest) -> Result<NormalizedResponse> {
        // Create routing context (simplified - can be extended with headers, etc.)
        let context = RoutingContext::new();

        // Find route
        let decision = self
            .route_table
            .find_route(&request, &context)
            .ok_or_else(|| {
                Error::Provider(format!("No route found for model '{}'", request.model))
            })?;

        // Determine primary provider (from strategy or direct)
        let primary_provider = if let Some(strategy) = &decision.strategy {
            let rule_name = decision.matched_rule.as_deref().unwrap_or("unknown");
            let selected = self.select_provider_from_strategy(strategy, rule_name)?;

            tracing::info!(
                model = %request.model,
                selected_provider = %selected,
                strategy = "round-robin/weighted",
                rule = ?decision.matched_rule,
                "Route decision made (strategy)"
            );

            selected
        } else if let Some(primary) = &decision.primary {
            tracing::info!(
                model = %request.model,
                primary = %primary,
                fallbacks = ?decision.fallbacks,
                rule = ?decision.matched_rule,
                "Route decision made (primary)"
            );

            primary.clone()
        } else {
            return Err(Error::Provider(
                "No primary provider or strategy specified".to_string(),
            ));
        };

        // Try primary/selected provider
        match self.try_provider(&primary_provider, &request).await {
            Ok(response) => return Ok(response),
            Err(err) => {
                tracing::warn!(
                    provider = %primary_provider,
                    error = %err,
                    "Primary/selected provider failed, trying fallbacks"
                );
            }
        }

        // Try fallback providers
        for fallback in &decision.fallbacks {
            match self.try_provider(fallback, &request).await {
                Ok(response) => {
                    tracing::info!(
                        fallback = %fallback,
                        "Fallback provider succeeded"
                    );
                    return Ok(response);
                }
                Err(err) => {
                    tracing::warn!(
                        fallback = %fallback,
                        error = %err,
                        "Fallback provider failed"
                    );
                }
            }
        }

        // All providers failed
        Err(Error::Provider(format!(
            "All providers failed for model '{}' (primary: {}, fallbacks: {:?})",
            request.model, primary_provider, decision.fallbacks
        )))
    }

    async fn stream(
        &self,
        request: NormalizedRequest,
    ) -> Result<Box<dyn Stream<Item = Result<NormalizedStreamEvent>> + Send + Unpin>> {
        // Create routing context
        let context = RoutingContext::new();

        // Find route
        let decision = self
            .route_table
            .find_route(&request, &context)
            .ok_or_else(|| {
                Error::Provider(format!("No route found for model '{}'", request.model))
            })?;

        // Determine primary provider (from strategy or direct)
        let primary_provider = if let Some(strategy) = &decision.strategy {
            let rule_name = decision.matched_rule.as_deref().unwrap_or("unknown");
            let selected = self.select_provider_from_strategy(strategy, rule_name)?;

            tracing::info!(
                model = %request.model,
                selected_provider = %selected,
                strategy = "round-robin/weighted",
                rule = ?decision.matched_rule,
                "Route decision made for streaming request (strategy)"
            );

            selected
        } else if let Some(primary) = &decision.primary {
            tracing::info!(
                model = %request.model,
                primary = %primary,
                fallbacks = ?decision.fallbacks,
                rule = ?decision.matched_rule,
                "Route decision made for streaming request (primary)"
            );

            primary.clone()
        } else {
            return Err(Error::Provider(
                "No primary provider or strategy specified".to_string(),
            ));
        };

        // For streaming, we'll try primary/selected first, then fallbacks
        // Note: Circuit breaker check for streaming
        let circuit_breaker = self.get_circuit_breaker(&primary_provider);

        if !circuit_breaker.allow_request() {
            tracing::warn!(
                provider = %primary_provider,
                state = ?circuit_breaker.state(),
                "Circuit breaker is open for streaming request"
            );

            // Try fallbacks for streaming
            for fallback in &decision.fallbacks {
                let fallback_cb = self.get_circuit_breaker(fallback);
                if fallback_cb.allow_request() {
                    let provider = self.providers.get(fallback).ok_or_else(|| {
                        Error::Provider(format!("Fallback provider '{}' not found", fallback))
                    })?;

                    tracing::info!(
                        fallback = %fallback,
                        "Using fallback provider for streaming due to circuit breaker"
                    );

                    // TODO: Wrap stream to track success/failure
                    return provider.stream(request).await;
                }
            }

            return Err(Error::Provider(format!(
                "Circuit breaker open and no healthy fallbacks for model '{}'",
                request.model
            )));
        }

        // Use primary/selected provider for streaming
        let provider = self
            .providers
            .get(&primary_provider)
            .ok_or_else(|| Error::Provider(format!("Provider '{}' not found", primary_provider)))?;

        tracing::debug!(
            provider = %primary_provider,
            model = %request.model,
            "Starting streaming request"
        );

        // TODO: Wrap stream to track success/failure and update circuit breaker
        provider.stream(request).await
    }

    fn capabilities(&self) -> ProviderCapabilities {
        // Router supports what all providers support
        // For simplicity, we'll return a union of capabilities
        ProviderCapabilities {
            supports_streaming: true, // If any provider supports it
            supports_tools: true,     // If any provider supports it
            supports_vision: false,   // Conservative default
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunaroute_core::{
        normalized::{Message, MessageContent, Role, Usage},
        provider::ProviderCapabilities,
    };
    use mockall::mock;
    use mockall::predicate::*;

    mock! {
        pub TestProvider {}

        #[async_trait]
        impl Provider for TestProvider {
            async fn send(&self, request: NormalizedRequest) -> Result<NormalizedResponse>;
            async fn stream(
                &self,
                request: NormalizedRequest,
            ) -> Result<Box<dyn Stream<Item = Result<NormalizedStreamEvent>> + Send + Unpin>>;
            fn capabilities(&self) -> ProviderCapabilities;
        }
    }

    fn create_test_request(model: &str) -> NormalizedRequest {
        NormalizedRequest {
            model: model.to_string(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text("test".to_string()),
                name: None,
                tool_calls: vec![],
                tool_call_id: None,
            }],
            system: None,
            max_tokens: Some(100),
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: vec![],
            stream: false,
            tools: vec![],
            tool_choice: None,
            metadata: HashMap::new(),
        }
    }

    fn create_test_response() -> NormalizedResponse {
        NormalizedResponse {
            id: "test-id".to_string(),
            model: "test-model".to_string(),
            choices: vec![],
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 20,
                total_tokens: 30,
            },
            created: 1234567890,
            metadata: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn test_router_basic_routing() {
        use crate::router::{RoutingRule, RuleMatcher};

        // Create mock provider
        let mut mock_provider = MockTestProvider::new();
        mock_provider
            .expect_send()
            .returning(|_| Ok(create_test_response()));

        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        providers.insert("test-provider".to_string(), Arc::new(mock_provider));

        // Create routing rule
        let rule = RoutingRule {
            priority: 10,
            name: Some("test-rule".to_string()),
            matcher: RuleMatcher::Always,
            strategy: None,
            primary: Some("test-provider".to_string()),
            fallbacks: vec![],
        };

        let route_table = RouteTable::with_rules(vec![rule]);
        let router = Router::with_defaults(route_table, providers);

        // Send request
        let request = create_test_request("test-model");
        let response = router.send(request).await.unwrap();

        assert_eq!(response.model, "test-model");
    }

    #[tokio::test]
    async fn test_router_no_route_found() {
        let providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        let route_table = RouteTable::new(); // Empty route table
        let router = Router::with_defaults(route_table, providers);

        let request = create_test_request("test-model");
        let result = router.send(request).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No route found"));
    }

    #[tokio::test]
    async fn test_router_fallback_on_primary_failure() {
        use crate::router::{RoutingRule, RuleMatcher};

        // Create mock providers
        let mut mock_primary = MockTestProvider::new();
        mock_primary
            .expect_send()
            .returning(|_| Err(Error::Provider("Primary failed".to_string())));

        let mut mock_fallback = MockTestProvider::new();
        mock_fallback
            .expect_send()
            .returning(|_| Ok(create_test_response()));

        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        providers.insert("primary".to_string(), Arc::new(mock_primary));
        providers.insert("fallback".to_string(), Arc::new(mock_fallback));

        // Create routing rule with fallback
        let rule = RoutingRule {
            priority: 10,
            name: Some("test-rule".to_string()),
            matcher: RuleMatcher::Always,
            strategy: None,
            primary: Some("primary".to_string()),
            fallbacks: vec!["fallback".to_string()],
        };

        let route_table = RouteTable::with_rules(vec![rule]);
        let router = Router::with_defaults(route_table, providers);

        // Send request - should succeed via fallback
        let request = create_test_request("test-model");
        let response = router.send(request).await.unwrap();

        assert_eq!(response.model, "test-model");
    }

    #[tokio::test]
    async fn test_router_all_providers_fail() {
        use crate::router::{RoutingRule, RuleMatcher};

        // Create mock providers that all fail
        let mut mock_primary = MockTestProvider::new();
        mock_primary
            .expect_send()
            .returning(|_| Err(Error::Provider("Primary failed".to_string())));

        let mut mock_fallback = MockTestProvider::new();
        mock_fallback
            .expect_send()
            .returning(|_| Err(Error::Provider("Fallback failed".to_string())));

        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        providers.insert("primary".to_string(), Arc::new(mock_primary));
        providers.insert("fallback".to_string(), Arc::new(mock_fallback));

        let rule = RoutingRule {
            priority: 10,
            name: Some("test-rule".to_string()),
            matcher: RuleMatcher::Always,
            strategy: None,
            primary: Some("primary".to_string()),
            fallbacks: vec!["fallback".to_string()],
        };

        let route_table = RouteTable::with_rules(vec![rule]);
        let router = Router::with_defaults(route_table, providers);

        let request = create_test_request("test-model");
        let result = router.send(request).await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("All providers failed")
        );
    }

    #[tokio::test]
    async fn test_router_circuit_breaker_integration() {
        use crate::router::{RoutingRule, RuleMatcher};
        use std::time::Duration;

        // Create mock provider that fails
        let mut mock_provider = MockTestProvider::new();
        mock_provider
            .expect_send()
            .returning(|_| Err(Error::Provider("Always fails".to_string())));

        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        providers.insert("test-provider".to_string(), Arc::new(mock_provider));

        let rule = RoutingRule {
            priority: 10,
            name: Some("test-rule".to_string()),
            matcher: RuleMatcher::Always,
            strategy: None,
            primary: Some("test-provider".to_string()),
            fallbacks: vec![],
        };

        let route_table = RouteTable::with_rules(vec![rule]);

        // Create router with low thresholds for testing
        let circuit_breaker_config = CircuitBreakerConfig {
            failure_threshold: 2,
            success_threshold: 1,
            timeout: Duration::from_millis(100),
        };

        let router = Router::new(
            route_table,
            providers,
            HealthMonitorConfig::default(),
            circuit_breaker_config,
        );

        // First two requests should fail normally
        let request = create_test_request("test-model");
        assert!(router.send(request.clone()).await.is_err());
        assert!(router.send(request.clone()).await.is_err());

        // Third request should still fail (circuit breaker blocks or provider fails)
        let result = router.send(request).await;
        assert!(result.is_err());
        // Circuit breaker should be open after 2 failures
        // Error message will be "All providers failed" since circuit breaker blocked the request
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("All providers failed"));
    }

    #[tokio::test]
    async fn test_router_health_monitoring() {
        use crate::router::{RoutingRule, RuleMatcher};
        use std::time::Duration;

        // Create mock provider that succeeds
        let mut mock_provider = MockTestProvider::new();
        mock_provider
            .expect_send()
            .times(10) // Need enough requests for health status
            .returning(|_| Ok(create_test_response()));

        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        providers.insert("test-provider".to_string(), Arc::new(mock_provider));

        let rule = RoutingRule {
            priority: 10,
            name: Some("test-rule".to_string()),
            matcher: RuleMatcher::Always,
            strategy: None,
            primary: Some("test-provider".to_string()),
            fallbacks: vec![],
        };

        let route_table = RouteTable::with_rules(vec![rule]);

        // Use custom health config with lower min_requests for testing
        let health_config = HealthMonitorConfig {
            healthy_threshold: 0.95,
            unhealthy_threshold: 0.5,
            failure_window: Duration::from_secs(60),
            min_requests: 5, // Lower for testing
        };

        let router = Router::new(
            route_table,
            providers,
            health_config,
            CircuitBreakerConfig::default(),
        );

        // Send enough successful requests
        let request = create_test_request("test-model");
        for _ in 0..10 {
            router.send(request.clone()).await.unwrap();
        }

        // Check health status
        let health_status = router.health_monitor.get_status("test-provider");
        use crate::health::HealthStatus;
        assert_eq!(health_status, HealthStatus::Healthy);
    }

    // ========== STRATEGY INTEGRATION TESTS ==========

    #[tokio::test]
    async fn test_router_round_robin_strategy() {
        use crate::router::{RoutingRule, RuleMatcher};
        use crate::strategy::RoutingStrategy;
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        // Track which provider was called
        let p1_calls = StdArc::new(AtomicUsize::new(0));
        let p2_calls = StdArc::new(AtomicUsize::new(0));
        let p3_calls = StdArc::new(AtomicUsize::new(0));

        // Create mock providers that track calls
        let p1_calls_clone = p1_calls.clone();
        let mut mock_p1 = MockTestProvider::new();
        mock_p1.expect_send().returning(move |_| {
            p1_calls_clone.fetch_add(1, Ordering::SeqCst);
            Ok(create_test_response())
        });

        let p2_calls_clone = p2_calls.clone();
        let mut mock_p2 = MockTestProvider::new();
        mock_p2.expect_send().returning(move |_| {
            p2_calls_clone.fetch_add(1, Ordering::SeqCst);
            Ok(create_test_response())
        });

        let p3_calls_clone = p3_calls.clone();
        let mut mock_p3 = MockTestProvider::new();
        mock_p3.expect_send().returning(move |_| {
            p3_calls_clone.fetch_add(1, Ordering::SeqCst);
            Ok(create_test_response())
        });

        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        providers.insert("p1".to_string(), Arc::new(mock_p1));
        providers.insert("p2".to_string(), Arc::new(mock_p2));
        providers.insert("p3".to_string(), Arc::new(mock_p3));

        // Create routing rule with round-robin strategy
        let rule = RoutingRule {
            priority: 10,
            name: Some("round-robin-rule".to_string()),
            matcher: RuleMatcher::Always,
            strategy: Some(RoutingStrategy::RoundRobin {
                providers: vec!["p1".to_string(), "p2".to_string(), "p3".to_string()],
            }),
            primary: None,
            fallbacks: vec![],
        };

        let route_table = RouteTable::with_rules(vec![rule]);
        let router = Router::with_defaults(route_table, providers);

        // Send 9 requests - should distribute evenly (3 each)
        for _ in 0..9 {
            let request = create_test_request("test-model");
            router.send(request).await.unwrap();
        }

        // Verify round-robin distribution
        assert_eq!(p1_calls.load(Ordering::SeqCst), 3);
        assert_eq!(p2_calls.load(Ordering::SeqCst), 3);
        assert_eq!(p3_calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_router_weighted_round_robin_strategy() {
        use crate::router::{RoutingRule, RuleMatcher};
        use crate::strategy::{RoutingStrategy, WeightedProvider};
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        // Track which provider was called
        let p1_calls = StdArc::new(AtomicUsize::new(0));
        let p2_calls = StdArc::new(AtomicUsize::new(0));

        let p1_calls_clone = p1_calls.clone();
        let mut mock_p1 = MockTestProvider::new();
        mock_p1.expect_send().returning(move |_| {
            p1_calls_clone.fetch_add(1, Ordering::SeqCst);
            Ok(create_test_response())
        });

        let p2_calls_clone = p2_calls.clone();
        let mut mock_p2 = MockTestProvider::new();
        mock_p2.expect_send().returning(move |_| {
            p2_calls_clone.fetch_add(1, Ordering::SeqCst);
            Ok(create_test_response())
        });

        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        providers.insert("p1".to_string(), Arc::new(mock_p1));
        providers.insert("p2".to_string(), Arc::new(mock_p2));

        // Create routing rule with weighted round-robin (70/30)
        let rule = RoutingRule {
            priority: 10,
            name: Some("weighted-rule".to_string()),
            matcher: RuleMatcher::Always,
            strategy: Some(RoutingStrategy::WeightedRoundRobin {
                providers: vec![
                    WeightedProvider {
                        id: "p1".to_string(),
                        weight: 70,
                    },
                    WeightedProvider {
                        id: "p2".to_string(),
                        weight: 30,
                    },
                ],
            }),
            primary: None,
            fallbacks: vec![],
        };

        let route_table = RouteTable::with_rules(vec![rule]);
        let router = Router::with_defaults(route_table, providers);

        // Send 100 requests - should distribute 70/30
        for _ in 0..100 {
            let request = create_test_request("test-model");
            router.send(request).await.unwrap();
        }

        // Verify weighted distribution
        assert_eq!(p1_calls.load(Ordering::SeqCst), 70);
        assert_eq!(p2_calls.load(Ordering::SeqCst), 30);
    }

    #[tokio::test]
    async fn test_router_strategy_with_fallbacks() {
        use crate::router::{RoutingRule, RuleMatcher};
        use crate::strategy::RoutingStrategy;

        // Create mock providers - p1 and p2 fail, p3 succeeds
        let mut mock_p1 = MockTestProvider::new();
        mock_p1
            .expect_send()
            .returning(|_| Err(Error::Provider("p1 failed".to_string())));

        let mut mock_p2 = MockTestProvider::new();
        mock_p2
            .expect_send()
            .returning(|_| Err(Error::Provider("p2 failed".to_string())));

        let mut mock_p3 = MockTestProvider::new();
        mock_p3
            .expect_send()
            .returning(|_| Ok(create_test_response()));

        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        providers.insert("p1".to_string(), Arc::new(mock_p1));
        providers.insert("p2".to_string(), Arc::new(mock_p2));
        providers.insert("p3".to_string(), Arc::new(mock_p3));

        // Strategy selects p1/p2, but p3 is fallback
        let rule = RoutingRule {
            priority: 10,
            name: Some("strategy-with-fallback".to_string()),
            matcher: RuleMatcher::Always,
            strategy: Some(RoutingStrategy::RoundRobin {
                providers: vec!["p1".to_string(), "p2".to_string()],
            }),
            primary: None,
            fallbacks: vec!["p3".to_string()],
        };

        let route_table = RouteTable::with_rules(vec![rule]);
        let router = Router::with_defaults(route_table, providers);

        // First request: strategy picks p1, fails, should fallback to p3
        let request = create_test_request("test-model");
        let response = router.send(request).await.unwrap();
        assert_eq!(response.model, "test-model");

        // Second request: strategy picks p2, fails, should fallback to p3
        let request = create_test_request("test-model");
        let response = router.send(request).await.unwrap();
        assert_eq!(response.model, "test-model");
    }

    #[tokio::test]
    async fn test_router_strategy_state_persistence() {
        use crate::router::{RoutingRule, RuleMatcher};
        use crate::strategy::RoutingStrategy;
        use std::sync::Arc as StdArc;

        let call_sequence = StdArc::new(std::sync::Mutex::new(Vec::new()));

        // Track call order
        let seq1 = call_sequence.clone();
        let mut mock_p1 = MockTestProvider::new();
        mock_p1.expect_send().returning(move |_| {
            seq1.lock().unwrap().push("p1");
            Ok(create_test_response())
        });

        let seq2 = call_sequence.clone();
        let mut mock_p2 = MockTestProvider::new();
        mock_p2.expect_send().returning(move |_| {
            seq2.lock().unwrap().push("p2");
            Ok(create_test_response())
        });

        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        providers.insert("p1".to_string(), Arc::new(mock_p1));
        providers.insert("p2".to_string(), Arc::new(mock_p2));

        let rule = RoutingRule {
            priority: 10,
            name: Some("persistent-state".to_string()),
            matcher: RuleMatcher::Always,
            strategy: Some(RoutingStrategy::RoundRobin {
                providers: vec!["p1".to_string(), "p2".to_string()],
            }),
            primary: None,
            fallbacks: vec![],
        };

        let route_table = RouteTable::with_rules(vec![rule]);
        let router = Router::with_defaults(route_table, providers);

        // Send 4 requests
        for _ in 0..4 {
            let request = create_test_request("test-model");
            router.send(request).await.unwrap();
        }

        // Verify state was maintained: p1, p2, p1, p2
        let sequence = call_sequence.lock().unwrap();
        assert_eq!(*sequence, vec!["p1", "p2", "p1", "p2"]);
    }

    #[tokio::test]
    async fn test_router_strategy_concurrent_requests() {
        use crate::router::{RoutingRule, RuleMatcher};
        use crate::strategy::RoutingStrategy;
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let p1_calls = StdArc::new(AtomicUsize::new(0));
        let p2_calls = StdArc::new(AtomicUsize::new(0));

        let p1_clone = p1_calls.clone();
        let mut mock_p1 = MockTestProvider::new();
        mock_p1.expect_send().returning(move |_| {
            p1_clone.fetch_add(1, Ordering::SeqCst);
            Ok(create_test_response())
        });

        let p2_clone = p2_calls.clone();
        let mut mock_p2 = MockTestProvider::new();
        mock_p2.expect_send().returning(move |_| {
            p2_clone.fetch_add(1, Ordering::SeqCst);
            Ok(create_test_response())
        });

        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        providers.insert("p1".to_string(), Arc::new(mock_p1));
        providers.insert("p2".to_string(), Arc::new(mock_p2));

        let rule = RoutingRule {
            priority: 10,
            name: Some("concurrent-rule".to_string()),
            matcher: RuleMatcher::Always,
            strategy: Some(RoutingStrategy::RoundRobin {
                providers: vec!["p1".to_string(), "p2".to_string()],
            }),
            primary: None,
            fallbacks: vec![],
        };

        let route_table = RouteTable::with_rules(vec![rule]);
        let router = Arc::new(Router::with_defaults(route_table, providers));

        // Send 20 concurrent requests
        let mut handles = vec![];
        for _ in 0..20 {
            let router_clone = router.clone();
            let handle = tokio::spawn(async move {
                let request = create_test_request("test-model");
                router_clone.send(request).await.unwrap();
            });
            handles.push(handle);
        }

        // Wait for all requests to complete
        for handle in handles {
            handle.await.unwrap();
        }

        // Should be distributed roughly evenly (10 each)
        let p1_count = p1_calls.load(Ordering::SeqCst);
        let p2_count = p2_calls.load(Ordering::SeqCst);

        assert_eq!(p1_count + p2_count, 20);
        assert_eq!(p1_count, 10);
        assert_eq!(p2_count, 10);
    }

    #[tokio::test]
    async fn test_router_streaming_with_strategy() {
        use crate::router::{RoutingRule, RuleMatcher};
        use crate::strategy::RoutingStrategy;
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let p1_calls = StdArc::new(AtomicUsize::new(0));
        let p2_calls = StdArc::new(AtomicUsize::new(0));

        let p1_clone = p1_calls.clone();
        let mut mock_p1 = MockTestProvider::new();
        mock_p1.expect_stream().returning(move |_| {
            p1_clone.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(tokio_stream::empty()))
        });

        let p2_clone = p2_calls.clone();
        let mut mock_p2 = MockTestProvider::new();
        mock_p2.expect_stream().returning(move |_| {
            p2_clone.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(tokio_stream::empty()))
        });

        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        providers.insert("p1".to_string(), Arc::new(mock_p1));
        providers.insert("p2".to_string(), Arc::new(mock_p2));

        let rule = RoutingRule {
            priority: 10,
            name: Some("streaming-strategy".to_string()),
            matcher: RuleMatcher::Always,
            strategy: Some(RoutingStrategy::RoundRobin {
                providers: vec!["p1".to_string(), "p2".to_string()],
            }),
            primary: None,
            fallbacks: vec![],
        };

        let route_table = RouteTable::with_rules(vec![rule]);
        let router = Router::with_defaults(route_table, providers);

        // Send 4 streaming requests
        for _ in 0..4 {
            let request = create_test_request("test-model");
            let _ = router.stream(request).await.unwrap();
        }

        // Verify round-robin for streaming
        assert_eq!(p1_calls.load(Ordering::SeqCst), 2);
        assert_eq!(p2_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_router_strategy_validation_no_primary_or_strategy() {
        use crate::router::{RoutingRule, RuleMatcher};

        // Rule with neither strategy nor primary should fail validation
        let rule = RoutingRule {
            priority: 10,
            name: Some("invalid-rule".to_string()),
            matcher: RuleMatcher::Always,
            strategy: None,
            primary: None,
            fallbacks: vec![],
        };

        // Validation should fail
        assert!(rule.validate().is_err());
        let err = rule.validate().unwrap_err();
        assert!(err.contains("strategy") || err.contains("primary"));
    }

    #[tokio::test]
    async fn test_router_multiple_rules_with_different_strategies() {
        use crate::router::{RoutingRule, RuleMatcher};
        use crate::strategy::RoutingStrategy;
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let p1_calls = StdArc::new(AtomicUsize::new(0));
        let p2_calls = StdArc::new(AtomicUsize::new(0));
        let p3_calls = StdArc::new(AtomicUsize::new(0));

        let p1_clone = p1_calls.clone();
        let mut mock_p1 = MockTestProvider::new();
        mock_p1.expect_send().returning(move |_| {
            p1_clone.fetch_add(1, Ordering::SeqCst);
            Ok(create_test_response())
        });

        let p2_clone = p2_calls.clone();
        let mut mock_p2 = MockTestProvider::new();
        mock_p2.expect_send().returning(move |_| {
            p2_clone.fetch_add(1, Ordering::SeqCst);
            Ok(create_test_response())
        });

        let p3_clone = p3_calls.clone();
        let mut mock_p3 = MockTestProvider::new();
        mock_p3.expect_send().returning(move |_| {
            p3_clone.fetch_add(1, Ordering::SeqCst);
            Ok(create_test_response())
        });

        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        providers.insert("p1".to_string(), Arc::new(mock_p1));
        providers.insert("p2".to_string(), Arc::new(mock_p2));
        providers.insert("p3".to_string(), Arc::new(mock_p3));

        // Rule 1: gpt models use round-robin between p1 and p2
        let rule1 = RoutingRule {
            priority: 20,
            name: Some("gpt-rule".to_string()),
            matcher: RuleMatcher::model_pattern("^gpt-.*"),
            strategy: Some(RoutingStrategy::RoundRobin {
                providers: vec!["p1".to_string(), "p2".to_string()],
            }),
            primary: None,
            fallbacks: vec![],
        };

        // Rule 2: claude models go to p3 only
        let rule2 = RoutingRule {
            priority: 20,
            name: Some("claude-rule".to_string()),
            matcher: RuleMatcher::model_pattern("^claude-.*"),
            strategy: None,
            primary: Some("p3".to_string()),
            fallbacks: vec![],
        };

        let route_table = RouteTable::with_rules(vec![rule1, rule2]);
        let router = Router::with_defaults(route_table, providers);

        // Send 4 gpt requests (should round-robin p1/p2)
        for _ in 0..4 {
            let request = create_test_request("gpt-4");
            router.send(request).await.unwrap();
        }

        // Send 4 claude requests (should all go to p3)
        for _ in 0..4 {
            let request = create_test_request("claude-3");
            router.send(request).await.unwrap();
        }

        // Verify distribution
        assert_eq!(p1_calls.load(Ordering::SeqCst), 2); // gpt requests
        assert_eq!(p2_calls.load(Ordering::SeqCst), 2); // gpt requests
        assert_eq!(p3_calls.load(Ordering::SeqCst), 4); // claude requests
    }
}
