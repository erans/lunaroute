//! Shared HTTP client utilities

use crate::{EgressError, Result};
use reqwest::{Client, ClientBuilder};
use std::time::Duration;
use tracing::{debug, warn};

/// HTTP client configuration
#[derive(Debug, Clone)]
pub struct HttpClientConfig {
    /// Request timeout in seconds
    pub timeout_secs: u64,

    /// Connection timeout in seconds
    pub connect_timeout_secs: u64,

    /// Maximum number of idle connections per host
    pub pool_max_idle_per_host: usize,

    /// Maximum number of retries for transient errors
    pub max_retries: u32,

    /// User agent string
    pub user_agent: String,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 60,
            connect_timeout_secs: 10,
            pool_max_idle_per_host: 32,
            max_retries: 3,
            user_agent: format!("LunaRoute/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

/// Create a configured HTTP client with connection pooling
pub fn create_client(config: &HttpClientConfig) -> Result<Client> {
    ClientBuilder::new()
        .timeout(Duration::from_secs(config.timeout_secs))
        .connect_timeout(Duration::from_secs(config.connect_timeout_secs))
        .pool_max_idle_per_host(config.pool_max_idle_per_host)
        .user_agent(&config.user_agent)
        // Use rustls for TLS (no openssl dependency)
        .use_rustls_tls()
        // Build the client
        .build()
        .map_err(|e| EgressError::ConfigError(format!("Failed to create HTTP client: {}", e)))
}

/// Retry policy for transient errors
pub async fn with_retry<F, Fut, T>(
    max_retries: u32,
    operation: F,
) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut last_error = None;

    for attempt in 0..=max_retries {
        if attempt > 0 {
            let backoff_ms = 2u64.pow(attempt - 1) * 100; // Exponential backoff: 100ms, 200ms, 400ms
            debug!("Retrying request after {}ms (attempt {}/{})", backoff_ms, attempt, max_retries);
            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        }

        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                // Check if error is retryable
                let should_retry = match &e {
                    EgressError::HttpError(req_err) => {
                        // Retry on network errors, connection errors, timeouts
                        req_err.is_connect() || req_err.is_timeout() || req_err.is_request()
                    },
                    EgressError::ProviderError { status_code, .. } => {
                        // Retry on 429 (rate limit), 500, 502, 503, 504
                        matches!(status_code, 429 | 500 | 502 | 503 | 504)
                    },
                    EgressError::Timeout(_) => true,
                    _ => false,
                };

                if should_retry && attempt < max_retries {
                    warn!("Request failed (attempt {}/{}): {:?}", attempt + 1, max_retries, e);
                    last_error = Some(e);
                } else {
                    return Err(e);
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| EgressError::ConfigError("Retry loop exited unexpectedly".to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = HttpClientConfig::default();
        assert_eq!(config.timeout_secs, 60);
        assert_eq!(config.connect_timeout_secs, 10);
        assert_eq!(config.pool_max_idle_per_host, 32);
        assert_eq!(config.max_retries, 3);
        assert!(config.user_agent.starts_with("LunaRoute/"));
    }

    #[test]
    fn test_create_client() {
        let config = HttpClientConfig::default();
        let client = create_client(&config);
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn test_retry_success_first_attempt() {
        let result = with_retry(3, || async {
            Ok::<i32, EgressError>(42)
        }).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retry_non_retryable_error() {
        let result = with_retry(3, || async {
            Err::<i32, EgressError>(EgressError::ConfigError("Invalid config".to_string()))
        }).await;

        assert!(result.is_err());
    }
}
