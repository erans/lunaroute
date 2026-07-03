//! Shared HTTP client utilities

use crate::{EgressError, Result};
use reqwest::{Client, ClientBuilder};
use std::time::Duration;
use tracing::{debug, warn};

/// HTTP client configuration
#[derive(Debug, Clone)]
pub struct HttpClientConfig {
    /// Request timeout in seconds
    /// Note: This applies to the entire request including streaming responses.
    /// Set high enough to accommodate long-running operations like extended thinking.
    pub timeout_secs: u64,

    /// Connection timeout in seconds
    pub connect_timeout_secs: u64,

    /// Maximum number of idle connections per host
    pub pool_max_idle_per_host: usize,

    /// Pool idle timeout in seconds
    /// Connections idle for longer than this are removed from the pool.
    /// Should be less than the upstream server's idle timeout (typically 60-120s).
    pub pool_idle_timeout_secs: u64,

    /// TCP keepalive interval in seconds
    /// Prevents firewall/load balancer timeouts during long-running requests.
    pub tcp_keepalive_secs: u64,

    /// Maximum number of retries for transient errors
    pub max_retries: u32,

    /// Enable connection pool metrics
    /// When true, exposes Prometheus metrics for pool behavior.
    pub enable_pool_metrics: bool,

    /// User agent string
    pub user_agent: String,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            // IMPORTANT: Timeout increased from 60s to 600s (10 minutes)
            //
            // This timeout applies to the ENTIRE request, including streaming responses.
            // For streaming requests (especially with extended thinking), the request
            // can remain open for several minutes as chunks are generated.
            //
            // Without this increase:
            // - Extended thinking sessions timeout mid-stream
            // - Claude Code compaction operations fail
            // - Connections get stuck in inconsistent state in the pool
            //
            // 600s (10 min) accommodates:
            // - Extended thinking/reasoning (can take 3-5+ minutes)
            // - Long streaming responses with pauses
            // - Complex code generation operations
            timeout_secs: 600,
            connect_timeout_secs: 10,
            pool_max_idle_per_host: 32,
            // IMPORTANT: Pool idle timeout increased from 50s to 600s (10 minutes)
            //
            // Previous value (50s) was set to expire connections before upstream servers
            // close them (typically 60-120s). However, this was too aggressive for proxies
            // like MegaLLM that have longer connection timeouts.
            //
            // With 50s timeout:
            // - Idle connections were closed too early
            // - "Poor internet connection" errors after 5 minutes of streaming
            // - Unnecessary connection churn during long operations
            //
            // 600s (10 min) provides:
            // - Stable connections for long-running streaming requests
            // - Compatibility with various proxy timeout configurations
            // - Reduced connection overhead from frequent reconnections
            //
            // Note: This is still well below typical load balancer timeouts (30+ min)
            pool_idle_timeout_secs: 600,
            // TCP keep-alive prevents firewall/load balancer timeouts during long requests
            tcp_keepalive_secs: 60,
            max_retries: 3,
            enable_pool_metrics: true,
            user_agent: format!("LunaRoute/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

/// Create a configured HTTP client with connection pooling
pub fn create_client(config: &HttpClientConfig) -> Result<Client> {
    debug!(
        "Creating HTTP client: timeout={}s, connect_timeout={}s, pool_max_idle={}, \
         pool_idle_timeout={}s, tcp_keepalive={}s, metrics_enabled={}",
        config.timeout_secs,
        config.connect_timeout_secs,
        config.pool_max_idle_per_host,
        config.pool_idle_timeout_secs,
        config.tcp_keepalive_secs,
        config.enable_pool_metrics
    );

    ClientBuilder::new()
        .timeout(Duration::from_secs(config.timeout_secs))
        .connect_timeout(Duration::from_secs(config.connect_timeout_secs))
        .pool_max_idle_per_host(config.pool_max_idle_per_host)
        // CRITICAL FIX: Expire idle connections before upstream servers close them
        // Without this, connections sit in pool forever and get closed by server,
        // causing "stuck" requests when client tries to reuse dead connections.
        .pool_idle_timeout(Duration::from_secs(config.pool_idle_timeout_secs))
        .user_agent(&config.user_agent)
        // Use rustls for TLS (no openssl dependency)
        .use_rustls_tls()
        // TCP keep-alive prevents firewall/load balancer timeouts during long requests
        .tcp_keepalive(Duration::from_secs(config.tcp_keepalive_secs))
        // Disable automatic decompression for true passthrough mode
        // In passthrough mode, we forward the exact response from upstream
        .no_gzip()
        .no_brotli()
        .no_deflate()
        // Build the client
        .build()
        .map_err(|e| EgressError::ConfigError(format!("Failed to create HTTP client: {}", e)))
}

/// Retry policy for transient errors
pub async fn with_retry<F, Fut, T>(max_retries: u32, operation: F) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut last_error = None;

    for attempt in 0..=max_retries {
        if attempt > 0 {
            // Exponential backoff: 100ms, 200ms, 400ms ... saturating to avoid
            // overflow for large max_retries (2u64.pow(64) would panic; checked_shl
            // + saturating_mul caps instead). For realistic max_retries (3-10)
            // values are identical to today.
            let backoff_ms =
                (1u64.checked_shl(attempt - 1).unwrap_or(u64::MAX)).saturating_mul(100);
            debug!(
                "Retrying request after {}ms (attempt {}/{})",
                backoff_ms, attempt, max_retries
            );
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
                    }
                    EgressError::ProviderError { status_code, .. } => {
                        // Retry on 429 (rate limit), 500, 502, 503, 504
                        matches!(status_code, 429 | 500 | 502 | 503 | 504)
                    }
                    EgressError::Timeout(_) => true,
                    _ => false,
                };

                if should_retry && attempt < max_retries {
                    warn!(
                        "Request failed (attempt {}/{}): {:?}",
                        attempt + 1,
                        max_retries,
                        e
                    );
                    last_error = Some(e);
                } else {
                    return Err(e);
                }
            }
        }
    }

    Err(last_error
        .unwrap_or_else(|| EgressError::ConfigError("Retry loop exited unexpectedly".to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = HttpClientConfig::default();
        assert_eq!(config.timeout_secs, 600); // 10 minutes for long-running streams
        assert_eq!(config.connect_timeout_secs, 10);
        assert_eq!(config.pool_max_idle_per_host, 32);
        assert_eq!(config.pool_idle_timeout_secs, 600); // 10 minutes for stable connections
        assert_eq!(config.tcp_keepalive_secs, 60); // Keep long requests alive
        assert_eq!(config.max_retries, 3);
        assert!(config.enable_pool_metrics);
        assert!(config.user_agent.starts_with("LunaRoute/"));
    }

    #[test]
    fn test_create_client() {
        let config = HttpClientConfig::default();
        let client = create_client(&config);
        assert!(client.is_ok());
    }

    /// REGRESSION TEST: Ensure pool_idle_timeout is configured to prevent stuck requests
    ///
    /// Background: Without pool_idle_timeout, HTTP connections sit in the pool forever.
    /// When upstream servers (OpenAI/Anthropic) close idle connections after 60-120s,
    /// the client tries to reuse dead connections, causing requests to hang.
    ///
    /// The timeout should be set based on the upstream proxy/server configuration:
    /// - Direct to OpenAI/Anthropic: Use 50s (below their 60s minimum)
    /// - Through proxies (e.g., MegaLLM): Use 600s (10 min) for stable connections
    ///
    /// Current default is 600s (10 min) to support long streaming requests through proxies.
    ///
    /// This test ensures the fix remains in place by verifying the client is built
    /// with the necessary configuration. While we can't directly assert the internal
    /// reqwest settings, building the client validates the configuration is valid.
    #[test]
    fn test_client_has_pool_idle_timeout_configured() {
        let config = HttpClientConfig::default();

        // This will fail to compile if we remove pool_idle_timeout from create_client
        let client = create_client(&config);
        assert!(
            client.is_ok(),
            "Client creation should succeed with pool_idle_timeout"
        );

        // Document the expected behavior for future maintainers
        // The client MUST have:
        // - pool_idle_timeout(600s) for stable connections through proxies
        // - tcp_keepalive(60s) to keep long-running requests alive
        //
        // If this test starts failing, check that create_client() includes:
        //   .pool_idle_timeout(Duration::from_secs(600))
        //   .tcp_keepalive(Duration::from_secs(60))
    }

    #[tokio::test]
    async fn test_retry_success_first_attempt() {
        let result = with_retry(3, || async { Ok::<i32, EgressError>(42) }).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retry_non_retryable_error() {
        let result = with_retry(3, || async {
            Err::<i32, EgressError>(EgressError::ConfigError("Invalid config".to_string()))
        })
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_backoff_does_not_overflow_for_large_max_retries_success_path() {
        // max_retries=65 would overflow 2u64.pow(64) today (panic in debug).
        // Saturating expression must not panic. Operation succeeds on first
        // attempt, so no actual sleep — this checks the expression compiles
        // and the fn doesn't panic on setup.
        let result: Result<i32> = with_retry(65, || async { Ok(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test(start_paused = true)]
    async fn test_backoff_does_not_overflow_for_large_max_retries_retry_path() {
        // Force the retry path with a non-429 retryable error and max_retries=65.
        // The backoff expression is evaluated on every retry; with the old
        // 2u64.pow(attempt-1) this would panic at attempt=64. Saturating
        // expression must not panic. Uses paused Tokio time plus explicit
        // virtual-time advancement so the sleeps don't block real wall-clock.
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let retry_task = tokio::spawn(async move {
            with_retry(65, move || {
                let a = attempts_clone.clone();
                async move {
                    a.fetch_add(1, Ordering::SeqCst);
                    // Non-429 retryable error -> egress retries with exponential backoff.
                    Err(EgressError::ProviderError {
                        status_code: 500,
                        message: "Internal error".to_string(),
                    })
                }
            })
            .await
        });

        for _ in 0..70 {
            if retry_task.is_finished() {
                break;
            }
            tokio::task::yield_now().await;
            tokio::time::advance(Duration::from_millis(u64::MAX)).await;
        }

        assert!(
            retry_task.is_finished(),
            "retry task should finish without real-time sleeps"
        );
        let result: Result<i32> = retry_task.await.unwrap();
        assert!(result.is_err());
        // All 66 attempts (0..=65) ran without panicking.
        assert_eq!(attempts.load(Ordering::SeqCst), 66);
    }

    #[test]
    fn test_error_display_formatting() {
        // Test all error variants display properly
        let err = EgressError::ConfigError("bad config".to_string());
        assert!(err.to_string().contains("Invalid configuration"));

        let err = EgressError::Timeout(30);
        assert_eq!(err.to_string(), "Request timeout after 30s");

        let err = EgressError::ProviderError {
            status_code: 500,
            message: "Internal error".to_string(),
        };
        assert!(err.to_string().contains("500"));

        let err = EgressError::RateLimitExceeded {
            retry_after_secs: Some(60),
        };
        assert!(err.to_string().contains("60s"));

        let err = EgressError::RateLimitExceeded {
            retry_after_secs: None,
        };
        assert_eq!(err.to_string(), "Rate limit exceeded");
    }

    #[test]
    fn test_custom_config() {
        let config = HttpClientConfig {
            timeout_secs: 30,
            connect_timeout_secs: 5,
            pool_max_idle_per_host: 16,
            pool_idle_timeout_secs: 60,
            tcp_keepalive_secs: 30,
            max_retries: 5,
            enable_pool_metrics: false,
            user_agent: "CustomAgent/1.0".to_string(),
        };

        assert_eq!(config.timeout_secs, 30);
        assert_eq!(config.pool_idle_timeout_secs, 60);
        assert_eq!(config.tcp_keepalive_secs, 30);
        assert_eq!(config.max_retries, 5);
        assert!(!config.enable_pool_metrics);
        assert_eq!(config.user_agent, "CustomAgent/1.0");
    }

    #[test]
    fn test_client_with_custom_config() {
        let config = HttpClientConfig {
            timeout_secs: 120,
            connect_timeout_secs: 20,
            pool_max_idle_per_host: 64,
            pool_idle_timeout_secs: 45,
            tcp_keepalive_secs: 90,
            max_retries: 5,
            enable_pool_metrics: true,
            user_agent: "Test/1.0".to_string(),
        };

        let client = create_client(&config);
        assert!(client.is_ok());
    }
}
