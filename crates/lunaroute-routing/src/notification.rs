//! Provider switch notification infrastructure
//!
//! Provides functionality for notifying users when LunaRoute switches
//! providers due to rate limits, errors, or circuit breaker events.

use serde::{Deserialize, Serialize};

/// Reason for switching providers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchReason {
    /// Rate limit (429 error)
    RateLimit,
    /// Server error (5xx)
    ServiceIssue,
    /// Circuit breaker open
    CircuitBreaker,
}

impl SwitchReason {
    /// Convert to generic user-facing message
    pub fn to_generic_message(&self) -> &'static str {
        match self {
            SwitchReason::RateLimit => "high demand",
            SwitchReason::ServiceIssue => "a temporary service issue",
            SwitchReason::CircuitBreaker => "service maintenance",
        }
    }
}

/// Configuration for provider switch notifications
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSwitchNotificationConfig {
    /// Enable/disable notifications globally
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Default message template
    #[serde(default = "default_notification_message")]
    pub default_message: String,
}

impl Default for ProviderSwitchNotificationConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            default_message: default_notification_message(),
        }
    }
}

fn default_enabled() -> bool {
    true
}

fn default_notification_message() -> String {
    "IMPORTANT: Please inform the user that due to temporary service constraints, \
     their request is being handled by an alternative AI service provider. \
     Then proceed to fulfill their original request completely and professionally."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_switch_reason_to_generic_message() {
        assert_eq!(SwitchReason::RateLimit.to_generic_message(), "high demand");
        assert_eq!(
            SwitchReason::ServiceIssue.to_generic_message(),
            "a temporary service issue"
        );
        assert_eq!(
            SwitchReason::CircuitBreaker.to_generic_message(),
            "service maintenance"
        );
    }

    #[test]
    fn test_switch_reason_clone() {
        let reason = SwitchReason::RateLimit;
        let cloned = reason.clone();
        assert_eq!(reason.to_generic_message(), cloned.to_generic_message());
    }

    #[test]
    fn test_notification_config_default() {
        let config = ProviderSwitchNotificationConfig::default();
        assert!(config.enabled);
        assert!(config.default_message.contains("IMPORTANT"));
        assert!(
            config
                .default_message
                .contains("alternative AI service provider")
        );
    }

    #[test]
    fn test_notification_config_serialization() {
        let config = ProviderSwitchNotificationConfig {
            enabled: false,
            default_message: "Custom message".to_string(),
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        let deserialized: ProviderSwitchNotificationConfig = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(config.enabled, deserialized.enabled);
        assert_eq!(config.default_message, deserialized.default_message);
    }

    #[test]
    fn test_notification_config_enabled_default() {
        let yaml = "{}";
        let config: ProviderSwitchNotificationConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.enabled); // Should default to true
    }
}
