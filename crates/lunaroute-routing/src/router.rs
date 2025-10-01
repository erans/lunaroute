//! Route table and routing logic
//!
//! Implements intelligent routing of requests to providers based on:
//! - Model name patterns (e.g., gpt-.* → OpenAI)
//! - Listener type (OpenAI endpoint → OpenAI provider)
//! - Header overrides (X-Luna-Provider)
//! - Fallback chains for automatic failover

use lunaroute_core::normalized::NormalizedRequest;
use once_cell::sync::OnceCell;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Which ingress listener received the request
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListenerType {
    /// OpenAI-compatible endpoint (/v1/chat/completions)
    OpenAI,
    /// Anthropic-compatible endpoint (/v1/messages)
    Anthropic,
}

/// Additional context for routing decisions beyond the normalized request
#[derive(Debug, Clone)]
pub struct RoutingContext {
    /// Which listener received the request
    pub listener: Option<ListenerType>,
    /// Provider override from headers (X-Luna-Provider)
    pub provider_override: Option<String>,
    /// Additional headers that might influence routing
    pub headers: HashMap<String, String>,
}

impl RoutingContext {
    /// Create a new routing context
    pub fn new() -> Self {
        Self {
            listener: None,
            provider_override: None,
            headers: HashMap::new(),
        }
    }

    /// Set the listener type
    pub fn with_listener(mut self, listener: ListenerType) -> Self {
        self.listener = Some(listener);
        self
    }

    /// Set provider override
    pub fn with_provider_override(mut self, provider: impl Into<String>) -> Self {
        self.provider_override = Some(provider.into());
        self
    }

    /// Add a header
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }
}

impl Default for RoutingContext {
    fn default() -> Self {
        Self::new()
    }
}

/// A routing rule that matches requests and specifies target providers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRule {
    /// Priority of this rule (higher = evaluated first)
    #[serde(default)]
    pub priority: i32,
    /// Optional name for the rule (for debugging/logging)
    #[serde(default)]
    pub name: Option<String>,
    /// Matcher for this rule
    pub matcher: RuleMatcher,
    /// Primary provider to route to
    pub primary: String,
    /// Fallback providers (tried in order if primary fails)
    #[serde(default)]
    pub fallbacks: Vec<String>,
}

/// Matcher for routing rules
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RuleMatcher {
    /// Match based on model name pattern (regex)
    #[serde(rename = "model")]
    ModelPattern {
        pattern: String,
        /// Compiled regex (lazily initialized, not serialized)
        #[serde(skip)]
        compiled: OnceCell<Option<Regex>>,
    },
    /// Match based on listener type
    #[serde(rename = "listener")]
    Listener { listener: ListenerType },
    /// Match if provider override header is present
    #[serde(rename = "override")]
    ProviderOverride,
    /// Always matches (catch-all/default rule)
    #[serde(rename = "always")]
    Always,
}

// Implement Clone manually because OnceCell doesn't implement Clone
impl Clone for RuleMatcher {
    fn clone(&self) -> Self {
        match self {
            RuleMatcher::ModelPattern { pattern, .. } => RuleMatcher::ModelPattern {
                pattern: pattern.clone(),
                compiled: OnceCell::new(), // New cell for the clone
            },
            RuleMatcher::Listener { listener } => RuleMatcher::Listener {
                listener: *listener,
            },
            RuleMatcher::ProviderOverride => RuleMatcher::ProviderOverride,
            RuleMatcher::Always => RuleMatcher::Always,
        }
    }
}

impl RuleMatcher {
    /// Create a new ModelPattern matcher with the given regex pattern
    pub fn model_pattern(pattern: impl Into<String>) -> Self {
        RuleMatcher::ModelPattern {
            pattern: pattern.into(),
            compiled: OnceCell::new(),
        }
    }

    /// Check if this matcher matches the given request and context
    fn matches(&self, request: &NormalizedRequest, context: &RoutingContext) -> bool {
        match self {
            RuleMatcher::ModelPattern { pattern, compiled } => {
                // Get or compile regex (cached for performance)
                let regex_opt = compiled.get_or_init(|| {
                    match Regex::new(pattern) {
                        Ok(regex) => Some(regex),
                        Err(e) => {
                            tracing::warn!(
                                "Invalid regex pattern '{}' in routing rule: {}",
                                pattern,
                                e
                            );
                            None
                        }
                    }
                });

                // Match against model name if regex compiled successfully
                regex_opt.as_ref().is_some_and(|regex| regex.is_match(&request.model))
            }
            RuleMatcher::Listener { listener } => {
                // Match listener type
                context.listener == Some(*listener)
            }
            RuleMatcher::ProviderOverride => {
                // Match if override is present
                context.provider_override.is_some()
            }
            RuleMatcher::Always => {
                // Always matches
                true
            }
        }
    }
}

/// Routing decision containing target provider and fallbacks
#[derive(Debug, Clone)]
pub struct RoutingDecision {
    /// Primary provider to use
    pub primary: String,
    /// Fallback providers to try if primary fails
    pub fallbacks: Vec<String>,
    /// The rule that matched (for logging/debugging)
    pub matched_rule: Option<String>,
}

/// Route table for managing routing rules
#[derive(Debug, Clone)]
pub struct RouteTable {
    /// List of routing rules (sorted by priority, highest first)
    rules: Vec<RoutingRule>,
}

impl RouteTable {
    /// Create a new empty route table
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// Create a route table with the given rules
    pub fn with_rules(mut rules: Vec<RoutingRule>) -> Self {
        // Sort rules by priority (highest first)
        rules.sort_by(|a, b| b.priority.cmp(&a.priority));
        Self { rules }
    }

    /// Add a rule to the route table
    pub fn add_rule(&mut self, rule: RoutingRule) {
        self.rules.push(rule);
        // Re-sort after adding
        self.rules
            .sort_by(|a, b| b.priority.cmp(&a.priority));
    }

    /// Find a matching route for the given request and context
    pub fn find_route(
        &self,
        request: &NormalizedRequest,
        context: &RoutingContext,
    ) -> Option<RoutingDecision> {
        // Priority 1: Provider override from context/header
        if let Some(provider) = &context.provider_override {
            tracing::debug!("Using provider override: {}", provider);
            return Some(RoutingDecision {
                primary: provider.clone(),
                fallbacks: vec![],
                matched_rule: Some("provider_override".to_string()),
            });
        }

        // Priority 2: Find first matching rule
        for rule in &self.rules {
            if rule.matcher.matches(request, context) {
                let rule_name = rule
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("rule_priority_{}", rule.priority));

                tracing::debug!(
                    "Matched routing rule '{}': {} → {}",
                    rule_name,
                    request.model,
                    rule.primary
                );

                return Some(RoutingDecision {
                    primary: rule.primary.clone(),
                    fallbacks: rule.fallbacks.clone(),
                    matched_rule: Some(rule_name),
                });
            }
        }

        // No match found
        tracing::warn!(
            "No routing rule matched for model '{}' and listener {:?}",
            request.model,
            context.listener
        );
        None
    }

    /// Get all rules (sorted by priority)
    pub fn rules(&self) -> &[RoutingRule] {
        &self.rules
    }

    /// Get the number of rules
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Check if the route table is empty
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

impl Default for RouteTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_request(model: &str) -> NormalizedRequest {
        NormalizedRequest {
            model: model.to_string(),
            messages: vec![],
            system: None,
            max_tokens: None,
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

    #[test]
    fn test_rule_matcher_model_pattern() {
        let matcher = RuleMatcher::model_pattern("^gpt-.*");
        let request = create_test_request("gpt-4");
        let context = RoutingContext::new();

        assert!(matcher.matches(&request, &context));

        let request2 = create_test_request("claude-3");
        assert!(!matcher.matches(&request2, &context));
    }

    #[test]
    fn test_rule_matcher_listener() {
        let matcher = RuleMatcher::Listener {
            listener: ListenerType::OpenAI,
        };
        let request = create_test_request("any-model");
        let context = RoutingContext::new().with_listener(ListenerType::OpenAI);

        assert!(matcher.matches(&request, &context));

        let context2 = RoutingContext::new().with_listener(ListenerType::Anthropic);
        assert!(!matcher.matches(&request, &context2));
    }

    #[test]
    fn test_rule_matcher_provider_override() {
        let matcher = RuleMatcher::ProviderOverride;
        let request = create_test_request("any-model");
        let context = RoutingContext::new().with_provider_override("custom");

        assert!(matcher.matches(&request, &context));

        let context2 = RoutingContext::new();
        assert!(!matcher.matches(&request, &context2));
    }

    #[test]
    fn test_rule_matcher_always() {
        let matcher = RuleMatcher::Always;
        let request = create_test_request("any-model");
        let context = RoutingContext::new();

        assert!(matcher.matches(&request, &context));
    }

    #[test]
    fn test_route_table_add_rule() {
        let mut table = RouteTable::new();
        assert_eq!(table.len(), 0);

        let rule = RoutingRule {
            priority: 10,
            name: Some("test".to_string()),
            matcher: RuleMatcher::Always,
            primary: "provider1".to_string(),
            fallbacks: vec![],
        };

        table.add_rule(rule);
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn test_route_table_priority_ordering() {
        let rules = vec![
            RoutingRule {
                priority: 5,
                name: Some("low".to_string()),
                matcher: RuleMatcher::Always,
                primary: "provider1".to_string(),
                fallbacks: vec![],
            },
            RoutingRule {
                priority: 10,
                name: Some("high".to_string()),
                matcher: RuleMatcher::Always,
                primary: "provider2".to_string(),
                fallbacks: vec![],
            },
        ];

        let table = RouteTable::with_rules(rules);
        // High priority rule should be first
        assert_eq!(table.rules()[0].priority, 10);
        assert_eq!(table.rules()[1].priority, 5);
    }

    #[test]
    fn test_find_route_provider_override() {
        let rule = RoutingRule {
            priority: 10,
            name: Some("default".to_string()),
            matcher: RuleMatcher::Always,
            primary: "provider1".to_string(),
            fallbacks: vec![],
        };
        let table = RouteTable::with_rules(vec![rule]);

        let request = create_test_request("gpt-4");
        let context = RoutingContext::new().with_provider_override("override_provider");

        let decision = table.find_route(&request, &context).unwrap();
        assert_eq!(decision.primary, "override_provider");
        assert_eq!(decision.matched_rule, Some("provider_override".to_string()));
    }

    #[test]
    fn test_find_route_model_pattern() {
        let rule = RoutingRule {
            priority: 10,
            name: Some("gpt_rule".to_string()),
            matcher: RuleMatcher::model_pattern("^gpt-.*"),
            primary: "openai".to_string(),
            fallbacks: vec!["openai_backup".to_string()],
        };
        let table = RouteTable::with_rules(vec![rule]);

        let request = create_test_request("gpt-4");
        let context = RoutingContext::new();

        let decision = table.find_route(&request, &context).unwrap();
        assert_eq!(decision.primary, "openai");
        assert_eq!(decision.fallbacks, vec!["openai_backup"]);
        assert_eq!(decision.matched_rule, Some("gpt_rule".to_string()));
    }

    #[test]
    fn test_find_route_listener() {
        let rule = RoutingRule {
            priority: 10,
            name: Some("anthropic_listener".to_string()),
            matcher: RuleMatcher::Listener {
                listener: ListenerType::Anthropic,
            },
            primary: "anthropic".to_string(),
            fallbacks: vec![],
        };
        let table = RouteTable::with_rules(vec![rule]);

        let request = create_test_request("any-model");
        let context = RoutingContext::new().with_listener(ListenerType::Anthropic);

        let decision = table.find_route(&request, &context).unwrap();
        assert_eq!(decision.primary, "anthropic");
    }

    #[test]
    fn test_find_route_first_match_wins() {
        let rules = vec![
            RoutingRule {
                priority: 20,
                name: Some("high_priority".to_string()),
                matcher: RuleMatcher::model_pattern(".*"), // Matches everything
                primary: "provider1".to_string(),
                fallbacks: vec![],
            },
            RoutingRule {
                priority: 10,
                name: Some("low_priority".to_string()),
                matcher: RuleMatcher::Always,
                primary: "provider2".to_string(),
                fallbacks: vec![],
            },
        ];
        let table = RouteTable::with_rules(rules);

        let request = create_test_request("any-model");
        let context = RoutingContext::new();

        let decision = table.find_route(&request, &context).unwrap();
        // High priority rule should match first
        assert_eq!(decision.primary, "provider1");
        assert_eq!(decision.matched_rule, Some("high_priority".to_string()));
    }

    #[test]
    fn test_find_route_no_match() {
        let rule = RoutingRule {
            priority: 10,
            name: Some("gpt_only".to_string()),
            matcher: RuleMatcher::model_pattern("^gpt-.*"),
            primary: "openai".to_string(),
            fallbacks: vec![],
        };
        let table = RouteTable::with_rules(vec![rule]);

        let request = create_test_request("claude-3");
        let context = RoutingContext::new();

        let decision = table.find_route(&request, &context);
        assert!(decision.is_none());
    }

    #[test]
    fn test_route_table_default() {
        let table = RouteTable::default();
        assert!(table.is_empty());
        assert_eq!(table.len(), 0);
    }

    #[test]
    fn test_routing_context_builder() {
        let context = RoutingContext::new()
            .with_listener(ListenerType::OpenAI)
            .with_provider_override("custom")
            .with_header("X-Custom", "value");

        assert_eq!(context.listener, Some(ListenerType::OpenAI));
        assert_eq!(context.provider_override, Some("custom".to_string()));
        assert_eq!(context.headers.get("X-Custom"), Some(&"value".to_string()));
    }

    // Configuration Serialization Tests

    #[test]
    fn test_serialize_routing_rule_model_pattern() {
        let rule = RoutingRule {
            priority: 10,
            name: Some("gpt_rule".to_string()),
            matcher: RuleMatcher::model_pattern("^gpt-.*"),
            primary: "openai".to_string(),
            fallbacks: vec!["openai_backup".to_string()],
        };

        let json = serde_json::to_string(&rule).unwrap();
        let deserialized: RoutingRule = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.priority, 10);
        assert_eq!(deserialized.name, Some("gpt_rule".to_string()));
        assert_eq!(deserialized.primary, "openai");
        assert_eq!(deserialized.fallbacks, vec!["openai_backup"]);

        // Verify matcher is correct
        if let RuleMatcher::ModelPattern { pattern, .. } = deserialized.matcher {
            assert_eq!(pattern, "^gpt-.*");
        } else {
            panic!("Expected ModelPattern matcher");
        }
    }

    #[test]
    fn test_serialize_routing_rule_listener() {
        let rule = RoutingRule {
            priority: 5,
            name: None,
            matcher: RuleMatcher::Listener {
                listener: ListenerType::Anthropic,
            },
            primary: "anthropic".to_string(),
            fallbacks: vec![],
        };

        let json = serde_json::to_string(&rule).unwrap();
        let deserialized: RoutingRule = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.priority, 5);
        assert_eq!(deserialized.name, None);

        if let RuleMatcher::Listener { listener } = deserialized.matcher {
            assert_eq!(listener, ListenerType::Anthropic);
        } else {
            panic!("Expected Listener matcher");
        }
    }

    #[test]
    fn test_deserialize_routing_rule_from_json() {
        let json = r#"{
            "priority": 20,
            "name": "claude_rule",
            "matcher": {
                "type": "model",
                "pattern": "^claude-.*"
            },
            "primary": "anthropic",
            "fallbacks": ["anthropic_backup"]
        }"#;

        let rule: RoutingRule = serde_json::from_str(json).unwrap();

        assert_eq!(rule.priority, 20);
        assert_eq!(rule.name, Some("claude_rule".to_string()));
        assert_eq!(rule.primary, "anthropic");
        assert_eq!(rule.fallbacks, vec!["anthropic_backup"]);

        if let RuleMatcher::ModelPattern { pattern, .. } = rule.matcher {
            assert_eq!(pattern, "^claude-.*");
        } else {
            panic!("Expected ModelPattern matcher");
        }
    }

    #[test]
    fn test_deserialize_routing_rule_always_matcher() {
        let json = r#"{
            "matcher": {
                "type": "always"
            },
            "primary": "default_provider"
        }"#;

        let rule: RoutingRule = serde_json::from_str(json).unwrap();

        assert_eq!(rule.priority, 0); // Default
        assert_eq!(rule.name, None); // Default
        assert_eq!(rule.primary, "default_provider");
        assert_eq!(rule.fallbacks, Vec::<String>::new()); // Default

        assert!(matches!(rule.matcher, RuleMatcher::Always));
    }

    #[test]
    fn test_deserialize_listener_type() {
        let openai_json = r#""OpenAI""#;
        let anthropic_json = r#""Anthropic""#;

        let openai: ListenerType = serde_json::from_str(openai_json).unwrap();
        let anthropic: ListenerType = serde_json::from_str(anthropic_json).unwrap();

        assert_eq!(openai, ListenerType::OpenAI);
        assert_eq!(anthropic, ListenerType::Anthropic);
    }

    #[test]
    fn test_serialize_deserialize_route_table() {
        let rules = vec![
            RoutingRule {
                priority: 10,
                name: Some("rule1".to_string()),
                matcher: RuleMatcher::model_pattern("^gpt-.*"),
                primary: "openai".to_string(),
                fallbacks: vec![],
            },
            RoutingRule {
                priority: 5,
                name: Some("rule2".to_string()),
                matcher: RuleMatcher::Always,
                primary: "default".to_string(),
                fallbacks: vec![],
            },
        ];

        // Serialize rules
        let json = serde_json::to_string(&rules).unwrap();

        // Deserialize and create route table
        let deserialized_rules: Vec<RoutingRule> = serde_json::from_str(&json).unwrap();
        let table = RouteTable::with_rules(deserialized_rules);

        assert_eq!(table.len(), 2);
        // Verify priority ordering (highest first)
        assert_eq!(table.rules()[0].priority, 10);
        assert_eq!(table.rules()[1].priority, 5);
    }

    // Edge Case Tests

    #[test]
    fn test_circuit_breaker_exactly_at_threshold() {
        use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitState};

        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            success_threshold: 2,
            timeout: std::time::Duration::from_secs(60),
        };
        let cb = CircuitBreaker::new(config);

        // Exactly at threshold should open
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);

        cb.record_failure(); // 3rd failure - should open
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_health_monitor_exactly_at_threshold() {
        use crate::health::{HealthMonitor, HealthMonitorConfig, HealthStatus};

        let config = HealthMonitorConfig {
            min_requests: 10,
            healthy_threshold: 0.95,
            unhealthy_threshold: 0.75,
            failure_window: std::time::Duration::from_secs(60),
        };
        let monitor = HealthMonitor::new(config);
        monitor.register_provider("provider1");

        // Exactly 95% success rate (19 success, 1 failure)
        for _ in 0..19 {
            monitor.record_success("provider1");
        }
        monitor.record_failure("provider1");

        assert_eq!(monitor.get_status("provider1"), HealthStatus::Healthy);

        // One more failure drops to 94.7% (18/19) - should be degraded
        monitor.record_failure("provider1");
        assert_eq!(monitor.get_status("provider1"), HealthStatus::Degraded);
    }

    #[test]
    fn test_invalid_regex_pattern_returns_false() {
        let matcher = RuleMatcher::model_pattern("[invalid(regex"); // Invalid regex
        let request = create_test_request("any-model");
        let context = RoutingContext::new();

        // Should return false and log warning
        assert!(!matcher.matches(&request, &context));
    }

    #[test]
    fn test_record_metrics_for_unregistered_provider() {
        use crate::health::HealthMonitor;

        let monitor = HealthMonitor::with_defaults();

        // Recording for unregistered provider should be no-op (no panic)
        monitor.record_success("nonexistent");
        monitor.record_failure("nonexistent");

        // Should return None
        assert!(monitor.get_metrics("nonexistent").is_none());
    }

    #[test]
    fn test_empty_rule_name_uses_priority() {
        let rule = RoutingRule {
            priority: 42,
            name: None, // No name
            matcher: RuleMatcher::Always,
            primary: "provider1".to_string(),
            fallbacks: vec![],
        };
        let table = RouteTable::with_rules(vec![rule]);

        let request = create_test_request("any-model");
        let context = RoutingContext::new();

        let decision = table.find_route(&request, &context).unwrap();
        // Should generate name from priority
        assert_eq!(decision.matched_rule, Some("rule_priority_42".to_string()));
    }

    #[test]
    fn test_multiple_providers_with_same_health_status() {
        use crate::health::{HealthMonitor, HealthMonitorConfig};

        let config = HealthMonitorConfig {
            min_requests: 10,
            ..Default::default()
        };
        let monitor = HealthMonitor::new(config);

        monitor.register_provider("provider1");
        monitor.register_provider("provider2");
        monitor.register_provider("provider3");

        // All have high success rate
        for provider in &["provider1", "provider2", "provider3"] {
            for _ in 0..19 {
                monitor.record_success(provider);
            }
            monitor.record_failure(provider);
        }

        let healthy = monitor.get_healthy_providers();
        assert_eq!(healthy.len(), 3);
        assert!(healthy.contains(&"provider1".to_string()));
        assert!(healthy.contains(&"provider2".to_string()));
        assert!(healthy.contains(&"provider3".to_string()));
    }

    #[test]
    fn test_route_table_with_empty_fallbacks() {
        let rule = RoutingRule {
            priority: 10,
            name: Some("no_fallback".to_string()),
            matcher: RuleMatcher::Always,
            primary: "primary_only".to_string(),
            fallbacks: vec![], // Empty fallbacks
        };
        let table = RouteTable::with_rules(vec![rule]);

        let request = create_test_request("any-model");
        let context = RoutingContext::new();

        let decision = table.find_route(&request, &context).unwrap();
        assert_eq!(decision.primary, "primary_only");
        assert_eq!(decision.fallbacks, Vec::<String>::new());
    }

    #[test]
    fn test_complex_model_pattern_matching() {
        let test_cases = vec![
            ("^gpt-.*", "gpt-4", true),
            ("^gpt-.*", "gpt-3.5-turbo", true),
            ("^gpt-.*", "claude-3", false),
            ("^claude-\\d+.*", "claude-3-opus", true),
            ("^claude-\\d+.*", "claude-sonnet", false),
            (".*-turbo$", "gpt-3.5-turbo", true),
            (".*-turbo$", "gpt-4", false),
            ("^(gpt|claude)-.*", "gpt-4", true),
            ("^(gpt|claude)-.*", "claude-3", true),
            ("^(gpt|claude)-.*", "llama-2", false),
        ];

        for (pattern, model, expected) in test_cases {
            let matcher = RuleMatcher::model_pattern(pattern);
            let request = create_test_request(model);
            let context = RoutingContext::new();

            assert_eq!(
                matcher.matches(&request, &context),
                expected,
                "Pattern '{}' with model '{}' should be {}",
                pattern,
                model,
                expected
            );
        }
    }

    #[test]
    fn test_health_status_with_zero_requests() {
        use crate::health::{HealthMonitor, HealthStatus};

        let monitor = HealthMonitor::with_defaults();
        monitor.register_provider("new_provider");

        // No requests yet
        assert_eq!(
            monitor.get_status("new_provider"),
            HealthStatus::Unknown
        );
        // Unknown is treated as healthy for routing purposes
        assert!(monitor.is_healthy("new_provider"));
    }

    #[test]
    fn test_circuit_breaker_success_in_open_state() {
        use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitState};

        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 2,
            timeout: std::time::Duration::from_secs(60),
        };
        let cb = CircuitBreaker::new(config);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Recording success in open state should be handled gracefully
        cb.record_success();
        // State should remain open (can't close from open without going through half-open)
        assert_eq!(cb.state(), CircuitState::Open);
    }
}
