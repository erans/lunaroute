//! Retry-After header parsing utilities
//!
//! This module provides functionality to parse the `retry-after` HTTP header
//! which can be in either numeric (seconds) or HTTP-date format (RFC 7231).
//!
//! ## Maximum Retry-After Duration
//!
//! Based on real-world API behavior:
//! - OpenAI: Can return up to 24 hours (86400s) for daily quota limits
//! - Anthropic: Daily quotas reset at midnight UTC, per-minute limits reset after 60s
//!
//! To prevent malicious or misconfigured providers from blocking indefinitely,
//! we cap retry-after at 48 hours (172800s) and log warnings for long durations.

use tracing::{debug, warn};

/// Maximum retry-after duration in seconds (48 hours).
///
/// This cap prevents malicious or misconfigured providers from setting
/// excessively long retry-after values that would block the provider indefinitely.
///
/// Real-world observed values:
/// - OpenAI daily quota: up to 86400s (24 hours)
/// - OpenAI per-minute: typically 50-60s
/// - Anthropic per-minute: 60s
///
/// The 48-hour cap provides generous headroom while preventing abuse.
const MAX_RETRY_AFTER_SECS: u64 = 172800; // 48 hours

/// Log a warning if retry-after exceeds this threshold (24 hours)
const WARN_THRESHOLD_SECS: u64 = 86400; // 24 hours

/// Parse the `retry-after` HTTP header value.
///
/// The `retry-after` header can be in two formats:
/// 1. Numeric: Number of seconds (e.g., "60", "120")
/// 2. HTTP-date: RFC 7231 date format (e.g., "Wed, 21 Oct 2015 07:28:00 GMT")
///
/// This function prioritizes the numeric format (most common from OpenAI/Anthropic),
/// and falls back to HTTP-date parsing if numeric parsing fails.
///
/// **Security:** Values are capped at 48 hours to prevent malicious or misconfigured
/// providers from blocking indefinitely. Warnings are logged for values exceeding 24 hours.
///
/// # Arguments
/// * `header_value` - The value of the retry-after header as a string
///
/// # Returns
/// * `Some(seconds)` - Number of seconds from now until retry is allowed (capped at 48h)
/// * `None` - If the header value is invalid or cannot be parsed
///
/// # Examples
/// ```
/// use lunaroute_egress::parse_retry_after;
///
/// // Numeric format (most common)
/// assert_eq!(parse_retry_after("60"), Some(60));
/// assert_eq!(parse_retry_after("120"), Some(120));
///
/// // Invalid input
/// assert_eq!(parse_retry_after("invalid"), None);
/// assert_eq!(parse_retry_after(""), None);
/// ```
pub fn parse_retry_after(header_value: &str) -> Option<u64> {
    // Try numeric format first (most common from OpenAI/Anthropic)
    if let Ok(seconds) = header_value.trim().parse::<u64>() {
        // Apply cap and log warnings for long durations
        let capped_seconds = apply_cap_and_warn(seconds, "numeric");
        debug!(
            retry_after_seconds = capped_seconds,
            original_seconds = seconds,
            "Parsed retry-after header (numeric format)"
        );
        return Some(capped_seconds);
    }

    // Try HTTP-date format (RFC 7231)
    // Format: "Wed, 21 Oct 2015 07:28:00 GMT"
    if let Ok(target_time) = chrono::DateTime::parse_from_rfc2822(header_value) {
        let now = chrono::Utc::now();
        let duration = target_time.signed_duration_since(now);

        if duration.num_seconds() > 0 {
            let seconds = duration.num_seconds() as u64;
            // Apply cap and log warnings for long durations
            let capped_seconds = apply_cap_and_warn(seconds, "HTTP-date");
            debug!(
                retry_after_seconds = capped_seconds,
                original_seconds = seconds,
                target_time = %target_time,
                "Parsed retry-after header (HTTP-date format)"
            );
            return Some(capped_seconds);
        } else {
            // Date is in the past, treat as 0 (can retry immediately)
            debug!(
                target_time = %target_time,
                "Parsed retry-after header with past date, treating as immediate retry"
            );
            return Some(0);
        }
    }

    // Unable to parse
    debug!(
        header_value = header_value,
        "Failed to parse retry-after header"
    );
    None
}

/// Apply maximum cap to retry-after value and log warnings for long durations
fn apply_cap_and_warn(seconds: u64, format: &str) -> u64 {
    if seconds > MAX_RETRY_AFTER_SECS {
        warn!(
            original_seconds = seconds,
            capped_seconds = MAX_RETRY_AFTER_SECS,
            format = format,
            original_hours = seconds / 3600,
            capped_hours = MAX_RETRY_AFTER_SECS / 3600,
            "Retry-after value exceeds maximum, capping at 48 hours. \
             This may indicate a misconfigured or malicious provider."
        );
        return MAX_RETRY_AFTER_SECS;
    }

    if seconds > WARN_THRESHOLD_SECS {
        warn!(
            seconds = seconds,
            hours = seconds / 3600,
            format = format,
            "Retry-after value is unusually long (>24 hours). \
             This typically indicates hitting a daily quota limit."
        );
    }

    seconds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_numeric_format() {
        assert_eq!(parse_retry_after("60"), Some(60));
        assert_eq!(parse_retry_after("120"), Some(120));
        assert_eq!(parse_retry_after("0"), Some(0));
        assert_eq!(parse_retry_after("3600"), Some(3600));
        assert_eq!(parse_retry_after("  60  "), Some(60)); // with whitespace
    }

    #[test]
    fn test_parse_invalid_input() {
        assert_eq!(parse_retry_after(""), None);
        assert_eq!(parse_retry_after("invalid"), None);
        assert_eq!(parse_retry_after("abc123"), None);
        assert_eq!(parse_retry_after("-60"), None); // negative not allowed in u64
    }

    #[test]
    fn test_parse_http_date_format() {
        // Create a future date
        let future = chrono::Utc::now() + chrono::Duration::seconds(120);
        let http_date = future.to_rfc2822();

        let result = parse_retry_after(&http_date);
        assert!(result.is_some());

        // Should be approximately 120 seconds (allow some tolerance for test execution time)
        let seconds = result.unwrap();
        assert!(
            seconds >= 118 && seconds <= 122,
            "Expected ~120 seconds, got {}",
            seconds
        );
    }

    #[test]
    fn test_parse_http_date_past() {
        // Create a past date
        let past = chrono::Utc::now() - chrono::Duration::seconds(60);
        let http_date = past.to_rfc2822();

        // Should return 0 (can retry immediately)
        assert_eq!(parse_retry_after(&http_date), Some(0));
    }

    #[test]
    fn test_parse_edge_cases() {
        assert_eq!(parse_retry_after("0"), Some(0));
        assert_eq!(parse_retry_after("999999"), Some(MAX_RETRY_AFTER_SECS)); // Capped
    }

    #[test]
    fn test_parse_capping_excessive_values() {
        // Test capping at 48 hours (172800 seconds)
        let far_future_seconds = "500000"; // ~138 hours
        assert_eq!(
            parse_retry_after(far_future_seconds),
            Some(MAX_RETRY_AFTER_SECS)
        );

        // Test HTTP-date format with far future
        let far_future = chrono::Utc::now() + chrono::Duration::hours(200);
        let http_date = far_future.to_rfc2822();
        assert_eq!(parse_retry_after(&http_date), Some(MAX_RETRY_AFTER_SECS));
    }

    #[test]
    fn test_parse_values_within_cap() {
        // 24 hours (86400 seconds) - should trigger warning but not be capped
        assert_eq!(parse_retry_after("86400"), Some(86400));

        // 30 hours (108000 seconds) - should trigger warning but not be capped
        assert_eq!(parse_retry_after("108000"), Some(108000));

        // 47 hours (169200 seconds) - just under cap, should warn
        assert_eq!(parse_retry_after("169200"), Some(169200));

        // Exactly at cap
        assert_eq!(parse_retry_after("172800"), Some(MAX_RETRY_AFTER_SECS));
    }

    #[test]
    fn test_parse_normal_values_no_warning() {
        // Values under 24 hours should not trigger warnings
        assert_eq!(parse_retry_after("60"), Some(60)); // 1 minute
        assert_eq!(parse_retry_after("3600"), Some(3600)); // 1 hour
        assert_eq!(parse_retry_after("43200"), Some(43200)); // 12 hours
    }
}
