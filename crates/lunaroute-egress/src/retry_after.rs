//! Retry-After header parsing utilities
//!
//! This module provides functionality to parse the `retry-after` HTTP header
//! which can be in either numeric (seconds) or HTTP-date format (RFC 7231).

use tracing::debug;

/// Parse the `retry-after` HTTP header value.
///
/// The `retry-after` header can be in two formats:
/// 1. Numeric: Number of seconds (e.g., "60", "120")
/// 2. HTTP-date: RFC 7231 date format (e.g., "Wed, 21 Oct 2015 07:28:00 GMT")
///
/// This function prioritizes the numeric format (most common from OpenAI/Anthropic),
/// and falls back to HTTP-date parsing if numeric parsing fails.
///
/// # Arguments
/// * `header_value` - The value of the retry-after header as a string
///
/// # Returns
/// * `Some(seconds)` - Number of seconds from now until retry is allowed
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
        debug!(
            retry_after_seconds = seconds,
            "Parsed retry-after header (numeric format)"
        );
        return Some(seconds);
    }

    // Try HTTP-date format (RFC 7231)
    // Format: "Wed, 21 Oct 2015 07:28:00 GMT"
    if let Ok(target_time) = chrono::DateTime::parse_from_rfc2822(header_value) {
        let now = chrono::Utc::now();
        let duration = target_time.signed_duration_since(now);

        if duration.num_seconds() > 0 {
            let seconds = duration.num_seconds() as u64;
            debug!(
                retry_after_seconds = seconds,
                target_time = %target_time,
                "Parsed retry-after header (HTTP-date format)"
            );
            return Some(seconds);
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
        assert_eq!(parse_retry_after("999999"), Some(999999));
    }
}
