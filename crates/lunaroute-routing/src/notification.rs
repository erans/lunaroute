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

/// Substitute template variables in a notification message
///
/// Supported variables:
/// - `${original_provider}`: Provider that failed
/// - `${new_provider}`: Provider being switched to
/// - `${reason}`: Generic reason for switch
/// - `${model}`: Model name from request
pub fn substitute_template_variables(
    template: &str,
    original_provider: &str,
    new_provider: &str,
    reason: &str,
    model: &str,
) -> String {
    template
        .replace("${original_provider}", original_provider)
        .replace("${new_provider}", new_provider)
        .replace("${reason}", reason)
        .replace("${model}", model)
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

    #[test]
    fn test_substitute_all_variables() {
        let template =
            "Switched from ${original_provider} to ${new_provider} due to ${reason} for ${model}";
        let result = substitute_template_variables(
            template,
            "openai-primary",
            "anthropic-backup",
            "high demand",
            "gpt-4",
        );

        assert_eq!(
            result,
            "Switched from openai-primary to anthropic-backup due to high demand for gpt-4"
        );
    }

    #[test]
    fn test_substitute_no_variables() {
        let template = "No variables here";
        let result =
            substitute_template_variables(template, "openai", "anthropic", "issue", "gpt-4");

        assert_eq!(result, "No variables here");
    }

    #[test]
    fn test_substitute_partial_variables() {
        let template = "Using ${new_provider} now";
        let result =
            substitute_template_variables(template, "openai", "anthropic", "issue", "gpt-4");

        assert_eq!(result, "Using anthropic now");
    }

    #[test]
    fn test_substitute_duplicate_variables() {
        let template = "${model} and ${model} again";
        let result =
            substitute_template_variables(template, "openai", "anthropic", "issue", "gpt-4");

        assert_eq!(result, "gpt-4 and gpt-4 again");
    }

    #[test]
    fn test_substitute_special_characters() {
        let template = "${new_provider}!";
        let result = substitute_template_variables(
            template,
            "openai",
            "anthropic-backup-2",
            "issue",
            "gpt-4-turbo",
        );

        assert_eq!(result, "anthropic-backup-2!");
    }
}
