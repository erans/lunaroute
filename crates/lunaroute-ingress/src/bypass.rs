//! Generic bypass proxy handler for unknown API paths
//!
//! This module provides a simple HTTP proxy that forwards requests directly
//! to the backend provider without any normalization or routing logic.
//! Used for paths not in the intercepted list (embeddings, audio, images, etc.)

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use lunaroute_routing::PathClassifier;
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, warn};

/// Bypass proxy error
#[derive(Debug, thiserror::Error)]
pub enum BypassError {
    #[error("HTTP request failed: {0}")]
    RequestFailed(#[from] reqwest::Error),

    #[error("Invalid header name: {0}")]
    InvalidHeaderName(String),

    #[error("Invalid header value: {0}")]
    InvalidHeaderValue(String),

    #[error("Body read error: {0}")]
    BodyReadError(String),

    #[error("No provider available for bypass")]
    NoProviderAvailable,
}

impl IntoResponse for BypassError {
    fn into_response(self) -> Response {
        let status = match &self {
            BypassError::NoProviderAvailable => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::BAD_GATEWAY,
        };

        let body = format!(
            "{{\"error\": \"bypass_proxy_error\", \"message\": \"{}\"}}",
            self
        );

        (status, body).into_response()
    }
}

/// Configuration for a bypass provider (simplified provider info)
#[derive(Debug, Clone)]
pub struct BypassProvider {
    /// Base URL of the provider (e.g., "https://api.openai.com/v1")
    pub base_url: String,
    /// API key for authentication
    pub api_key: String,
    /// Provider name for logging
    pub name: String,
    /// HTTP client
    pub client: Arc<Client>,
}

impl BypassProvider {
    /// Create a new bypass provider
    pub fn new(base_url: String, api_key: String, name: String, client: Arc<Client>) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            name,
            client,
        }
    }

    /// Build the full URL for a path
    fn build_url(&self, path: &str) -> String {
        let path = path.trim_start_matches('/');
        format!("{}/{}", self.base_url, path)
    }
}

/// Wrap an existing router with bypass functionality
///
/// This adds a fallback route that handles unknown paths by proxying them
/// directly to the provider if bypass is enabled.
pub fn with_bypass(
    router: Router,
    provider: Option<Arc<BypassProvider>>,
    classifier: Arc<PathClassifier>,
) -> Router {
    if !classifier.is_bypass_enabled() || provider.is_none() {
        // Bypass disabled, return router as-is
        return router;
    }

    // Clone for closure
    let provider_clone = provider.clone();
    let classifier_clone = classifier.clone();

    // Add fallback handler using a closure that captures the bypass state
    router.fallback(move |req: Request| {
        let provider = provider_clone.clone();
        let classifier = classifier_clone.clone();
        async move { bypass_handler_impl(provider, classifier, req).await }
    })
}

/// Implementation of bypass handler
async fn bypass_handler_impl(
    provider: Option<Arc<BypassProvider>>,
    classifier: Arc<PathClassifier>,
    req: Request,
) -> Result<Response, BypassError> {
    let path = req.uri().path().to_string(); // Clone path before moving req
    let method = req.method().clone();

    // Check if we should bypass this path
    if !classifier.should_bypass(&path) {
        // Path is intercepted but router didn't match (404)
        debug!("Path {} is intercepted but no handler matched", path);
        return Ok((StatusCode::NOT_FOUND, "Not Found").into_response());
    }

    // Get provider
    let provider = match provider {
        Some(p) => p,
        None => {
            warn!(
                "Bypass enabled but no provider configured for path: {}",
                path
            );
            return Err(BypassError::NoProviderAvailable);
        }
    };

    debug!(
        "Bypassing path {} to provider {} (bypass enabled)",
        path, provider.name
    );

    // Extract headers and body
    let (parts, body) = req.into_parts();
    let headers = parts.headers;

    // Read body
    let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(e) => {
            error!("Failed to read request body: {}", e);
            return Err(BypassError::BodyReadError(e.to_string()));
        }
    };

    // Proxy the request
    proxy_request(&provider, &path, method, headers, body_bytes).await
}

/// Direct proxy a request to the bypass provider
///
/// # Arguments
/// * `provider` - The bypass provider to send the request to
/// * `path` - The API path (e.g., "/v1/embeddings")
/// * `method` - HTTP method
/// * `headers` - Request headers from the client
/// * `body` - Request body bytes
///
/// # Returns
/// Returns the raw response from the provider, preserving status, headers, and body
pub async fn proxy_request(
    provider: &BypassProvider,
    path: &str,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, BypassError> {
    let url = provider.build_url(path);

    debug!(
        "Bypass proxy: {} {} -> {} (provider: {})",
        method, path, url, provider.name
    );

    // Convert HeaderMap to HashMap<String, String>, filtering hop-by-hop headers
    let mut request_headers = HashMap::new();

    for (name, value) in headers.iter() {
        let name_str = name.as_str();

        // Skip hop-by-hop headers
        if is_hop_by_hop_header(name_str) {
            continue;
        }

        // Skip host header (will be set by reqwest)
        if name_str.eq_ignore_ascii_case("host") {
            continue;
        }

        // Get value as string
        if let Ok(value_str) = value.to_str() {
            request_headers.insert(name_str.to_string(), value_str.to_string());
        } else {
            warn!("Skipping header with non-UTF8 value: {}", name_str);
        }
    }

    // Add/override Authorization header with provider API key
    // Detect provider type and set appropriate auth header
    if provider.base_url.contains("anthropic.com") {
        request_headers.insert("x-api-key".to_string(), provider.api_key.clone());
        request_headers.insert("anthropic-version".to_string(), "2023-06-01".to_string());
    } else {
        // Default to OpenAI-style auth (Bearer token)
        request_headers.insert(
            "authorization".to_string(),
            format!("Bearer {}", provider.api_key),
        );
    }

    // Build reqwest request
    let mut req_builder = provider
        .client
        .request(method.clone(), &url)
        .body(body.to_vec());

    // Add all headers
    for (name, value) in request_headers {
        req_builder = req_builder.header(name, value);
    }

    // Send request
    let response = req_builder.send().await?;

    // Extract status and headers from provider response
    let status = response.status();
    let provider_headers = response.headers().clone();

    // Read response body
    let response_bytes = response.bytes().await?;

    debug!(
        "Bypass proxy response: {} bytes, status: {}",
        response_bytes.len(),
        status
    );

    // Build response headers, filtering hop-by-hop headers
    let mut response_header_map = HeaderMap::new();

    for (name, value) in provider_headers.iter() {
        let name_str = name.as_str();

        // Skip hop-by-hop headers
        if is_hop_by_hop_header(name_str) {
            continue;
        }

        // Clone header to response
        response_header_map.insert(name.clone(), value.clone());
    }

    // Build axum response
    let mut response = Response::new(Body::from(response_bytes.to_vec()));
    *response.status_mut() = status;
    *response.headers_mut() = response_header_map;

    Ok(response)
}

/// Check if a header is a hop-by-hop header that should not be forwarded
fn is_hop_by_hop_header(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_hop_by_hop_header() {
        assert!(is_hop_by_hop_header("Connection"));
        assert!(is_hop_by_hop_header("connection"));
        assert!(is_hop_by_hop_header("Keep-Alive"));
        assert!(is_hop_by_hop_header("Transfer-Encoding"));
        assert!(is_hop_by_hop_header("Upgrade"));

        assert!(!is_hop_by_hop_header("Content-Type"));
        assert!(!is_hop_by_hop_header("Authorization"));
        assert!(!is_hop_by_hop_header("User-Agent"));
    }

    #[test]
    fn test_bypass_provider_build_url() {
        let client = Arc::new(Client::new());
        let provider = BypassProvider::new(
            "https://api.openai.com/v1".to_string(),
            "test-key".to_string(),
            "openai".to_string(),
            client,
        );

        assert_eq!(
            provider.build_url("/v1/embeddings"),
            "https://api.openai.com/v1/v1/embeddings"
        );

        assert_eq!(
            provider.build_url("v1/embeddings"),
            "https://api.openai.com/v1/v1/embeddings"
        );
    }

    #[test]
    fn test_bypass_provider_base_url_trimming() {
        let client = Arc::new(Client::new());

        // With trailing slash
        let provider1 = BypassProvider::new(
            "https://api.openai.com/v1/".to_string(),
            "key".to_string(),
            "test".to_string(),
            client.clone(),
        );
        assert_eq!(provider1.base_url, "https://api.openai.com/v1");

        // Without trailing slash
        let provider2 = BypassProvider::new(
            "https://api.openai.com/v1".to_string(),
            "key".to_string(),
            "test".to_string(),
            client.clone(),
        );
        assert_eq!(provider2.base_url, "https://api.openai.com/v1");
    }
}
