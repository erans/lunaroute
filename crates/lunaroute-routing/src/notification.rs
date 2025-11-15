//! Provider switch notification infrastructure
//!
//! Provides functionality for notifying users when LunaRoute switches
//! providers due to rate limits, errors, or circuit breaker events.

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
}
