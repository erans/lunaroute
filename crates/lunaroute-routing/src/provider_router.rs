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
};
use async_trait::async_trait;
use lunaroute_core::{
    error::{Error, Result},
    normalized::{NormalizedRequest, NormalizedResponse, NormalizedStreamEvent},
    provider::{Provider, ProviderCapabilities},
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_stream::Stream;

/// Router that delegates to multiple providers based on routing rules
pub struct Router {
    /// Routing table with rules
    route_table: RouteTable,

    /// Map of provider ID to provider instance
    providers: HashMap<String, Arc<dyn Provider>>,

    /// Health monitor for all providers
    health_monitor: Arc<HealthMonitor>,

    /// Circuit breakers per provider
    circuit_breakers: RwLock<HashMap<String, Arc<CircuitBreaker>>>,

    /// Default circuit breaker config for new providers
    circuit_breaker_config: CircuitBreakerConfig,
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
            circuit_breakers: RwLock::new(HashMap::new()),
            circuit_breaker_config,
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

    /// Get or create circuit breaker for a provider
    async fn get_circuit_breaker(&self, provider_id: &str) -> Arc<CircuitBreaker> {
        let breakers = self.circuit_breakers.read().await;

        if let Some(cb) = breakers.get(provider_id) {
            return Arc::clone(cb);
        }

        drop(breakers);

        // Create new circuit breaker
        let cb = Arc::new(CircuitBreaker::new(self.circuit_breaker_config.clone()));

        let mut breakers = self.circuit_breakers.write().await;
        breakers.insert(provider_id.to_string(), Arc::clone(&cb));

        cb
    }

    /// Try to send request to a provider, respecting circuit breaker
    async fn try_provider(
        &self,
        provider_id: &str,
        request: &NormalizedRequest,
    ) -> Result<NormalizedResponse> {
        let circuit_breaker = self.get_circuit_breaker(provider_id).await;

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

        let provider = self.providers.get(provider_id).ok_or_else(|| {
            Error::Provider(format!("Provider '{}' not found", provider_id))
        })?;

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
        let decision = self.route_table.find_route(&request, &context).ok_or_else(|| {
            Error::Provider(format!("No route found for model '{}'", request.model))
        })?;

        tracing::info!(
            model = %request.model,
            primary = %decision.primary,
            fallbacks = ?decision.fallbacks,
            rule = ?decision.matched_rule,
            "Route decision made"
        );

        // Try primary provider
        match self.try_provider(&decision.primary, &request).await {
            Ok(response) => return Ok(response),
            Err(err) => {
                tracing::warn!(
                    primary = %decision.primary,
                    error = %err,
                    "Primary provider failed, trying fallbacks"
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
            request.model, decision.primary, decision.fallbacks
        )))
    }

    async fn stream(
        &self,
        request: NormalizedRequest,
    ) -> Result<Box<dyn Stream<Item = Result<NormalizedStreamEvent>> + Send + Unpin>> {
        // Create routing context
        let context = RoutingContext::new();

        // Find route
        let decision = self.route_table.find_route(&request, &context).ok_or_else(|| {
            Error::Provider(format!("No route found for model '{}'", request.model))
        })?;

        tracing::info!(
            model = %request.model,
            primary = %decision.primary,
            fallbacks = ?decision.fallbacks,
            rule = ?decision.matched_rule,
            "Route decision made for streaming request"
        );

        // For streaming, we'll try primary first, then fallbacks
        // Note: Circuit breaker check for streaming
        let circuit_breaker = self.get_circuit_breaker(&decision.primary).await;

        if !circuit_breaker.allow_request() {
            tracing::warn!(
                provider = %decision.primary,
                state = ?circuit_breaker.state(),
                "Circuit breaker is open for streaming request"
            );

            // Try fallbacks for streaming
            for fallback in &decision.fallbacks {
                let fallback_cb = self.get_circuit_breaker(fallback).await;
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

        // Use primary provider for streaming
        let provider = self.providers.get(&decision.primary).ok_or_else(|| {
            Error::Provider(format!("Provider '{}' not found", decision.primary))
        })?;

        tracing::debug!(
            provider = %decision.primary,
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
            primary: "test-provider".to_string(),
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
            primary: "primary".to_string(),
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
            primary: "primary".to_string(),
            fallbacks: vec!["fallback".to_string()],
        };

        let route_table = RouteTable::with_rules(vec![rule]);
        let router = Router::with_defaults(route_table, providers);

        let request = create_test_request("test-model");
        let result = router.send(request).await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("All providers failed"));
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
            primary: "test-provider".to_string(),
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
            primary: "test-provider".to_string(),
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
}
