//! Shared ingress middleware

use crate::types::RequestMetadata;
use axum::{
    extract::Request,
    http::{header, HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};

/// Extension key for request metadata
#[derive(Clone)]
pub struct RequestMetadataExt(pub RequestMetadata);

/// Middleware to add request ID and trace context to all requests
pub async fn request_context_middleware(mut req: Request, next: Next) -> Response {
    let headers = req.headers();

    // Extract or generate request metadata
    let mut metadata = RequestMetadata::new();

    // Handle traceparent header if present
    if let Some(traceparent) = headers.get("traceparent")
        && let Ok(tp) = traceparent.to_str() {
            metadata = metadata.with_traceparent(tp);
        }

    // Extract client IP from X-Forwarded-For or X-Real-IP
    if let Some(forwarded_for) = headers.get("x-forwarded-for") {
        if let Ok(ip) = forwarded_for.to_str() {
            // Take the first IP in the list
            let client_ip = ip.split(',').next().unwrap_or(ip).trim().to_string();
            metadata = metadata.with_client_ip(client_ip);
        }
    } else if let Some(real_ip) = headers.get("x-real-ip")
        && let Ok(ip) = real_ip.to_str() {
            metadata = metadata.with_client_ip(ip.to_string());
        }

    // Extract user agent
    if let Some(user_agent) = headers.get(header::USER_AGENT)
        && let Ok(ua) = user_agent.to_str() {
            metadata = metadata.with_user_agent(ua.to_string());
        }

    // Add request ID to response headers
    let request_id = metadata.request_id.clone();

    // Insert metadata into request extensions
    req.extensions_mut().insert(RequestMetadataExt(metadata));

    // Call next middleware
    let mut response = next.run(req).await;

    // Add request ID to response headers
    // Safe to expect here: request_id format is always valid ASCII
    if let Ok(header_value) = request_id.to_string().parse() {
        response.headers_mut().insert("x-request-id", header_value);
    }

    response
}

/// Middleware to enforce body size limits
pub async fn body_size_limit_middleware(
    req: Request,
    next: Next,
    max_size: usize,
) -> Result<Response, StatusCode> {
    // Check content-length header
    if let Some(content_length) = req.headers().get(header::CONTENT_LENGTH)
        && let Ok(length_str) = content_length.to_str()
            && let Ok(length) = length_str.parse::<usize>()
                && length > max_size {
                    return Err(StatusCode::PAYLOAD_TOO_LARGE);
                }

    Ok(next.run(req).await)
}

/// CORS configuration
#[derive(Clone, Debug)]
pub struct CorsConfig {
    /// Allowed origins (use vec!["*"] for wildcard, but this is discouraged for production)
    pub allowed_origins: Vec<String>,
    /// Allowed HTTP methods
    pub allowed_methods: String,
    /// Allowed headers
    pub allowed_headers: String,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            // Default to localhost for development
            allowed_origins: vec!["http://localhost:3000".to_string()],
            allowed_methods: "GET, POST, OPTIONS".to_string(),
            allowed_headers: "Content-Type, Authorization, X-Request-ID".to_string(),
        }
    }
}

impl CorsConfig {
    /// Create a permissive CORS config for development (WARNING: Not for production!)
    pub fn permissive() -> Self {
        Self {
            allowed_origins: vec!["*".to_string()],
            allowed_methods: "GET, POST, PUT, PATCH, DELETE, OPTIONS".to_string(),
            allowed_headers: "Content-Type, Authorization, X-Request-ID".to_string(),
        }
    }
}

/// Middleware for CORS headers with configurable origins
pub async fn cors_middleware_with_config(
    config: CorsConfig,
) -> impl Fn(Request, Next) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>> + Clone {
    move |req: Request, next: Next| {
        let config = config.clone();
        Box::pin(async move {
            let request_origin = req.headers().get(header::ORIGIN)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            let mut response = next.run(req).await;
            let headers = response.headers_mut();

            // Determine which origin to allow
            let allowed_origin = if config.allowed_origins.contains(&"*".to_string()) {
                "*"
            } else if let Some(ref origin) = request_origin {
                if config.allowed_origins.contains(origin) {
                    origin.as_str()
                } else {
                    // Origin not allowed, don't set CORS headers
                    return response;
                }
            } else {
                // No origin header, use first allowed origin
                config.allowed_origins.first()
                    .map(|s| s.as_str())
                    .unwrap_or("http://localhost:3000")
            };

            headers.insert(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                allowed_origin.parse().unwrap(),
            );
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_METHODS,
                config.allowed_methods.parse().unwrap(),
            );
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                config.allowed_headers.parse().unwrap(),
            );

            response
        })
    }
}

/// Middleware for CORS headers (default configuration - localhost only)
///
/// **WARNING**: This middleware only allows localhost by default for security.
/// For production, use `cors_middleware_with_config` with specific allowed origins.
pub async fn cors_middleware(req: Request, next: Next) -> Response {
    let config = CorsConfig::default();
    cors_middleware_with_config(config).await(req, next).await
}

/// Middleware to add security headers
pub async fn security_headers_middleware(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;

    let headers = response.headers_mut();
    headers.insert(
        "x-content-type-options",
        "nosniff".parse().unwrap(),
    );
    headers.insert(
        "x-frame-options",
        "DENY".parse().unwrap(),
    );
    headers.insert(
        "x-xss-protection",
        "1; mode=block".parse().unwrap(),
    );
    headers.insert(
        "strict-transport-security",
        "max-age=31536000; includeSubDomains".parse().unwrap(),
    );

    response
}

/// Extract request metadata from request extensions
pub fn extract_metadata(headers: &HeaderMap) -> Option<RequestMetadata> {
    // This is a helper for extracting from headers when extensions aren't available
    let mut metadata = RequestMetadata::new();

    if let Some(traceparent) = headers.get("traceparent")
        && let Ok(tp) = traceparent.to_str() {
            metadata = metadata.with_traceparent(tp);
        }

    if let Some(user_agent) = headers.get(header::USER_AGENT)
        && let Ok(ua) = user_agent.to_str() {
            metadata = metadata.with_user_agent(ua.to_string());
        }

    Some(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        middleware,
        routing::get,
        Router,
    };
    use tower::ServiceExt;

    async fn test_handler() -> &'static str {
        "OK"
    }

    #[tokio::test]
    async fn test_request_context_middleware() {
        let app = Router::new()
            .route("/test", get(test_handler))
            .layer(middleware::from_fn(request_context_middleware));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("x-request-id").is_some());
    }

    #[tokio::test]
    async fn test_request_context_with_traceparent() {
        let app = Router::new()
            .route("/test", get(test_handler))
            .layer(middleware::from_fn(request_context_middleware));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header("traceparent", "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("x-request-id").is_some());
    }

    #[tokio::test]
    async fn test_security_headers_middleware() {
        let app = Router::new()
            .route("/test", get(test_handler))
            .layer(middleware::from_fn(security_headers_middleware));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert!(response.headers().get("x-content-type-options").is_some());
        assert!(response.headers().get("x-frame-options").is_some());
        assert!(response.headers().get("strict-transport-security").is_some());
    }

    #[tokio::test]
    async fn test_cors_middleware() {
        let app = Router::new()
            .route("/test", get(test_handler))
            .layer(middleware::from_fn(cors_middleware));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert!(response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_some());
        assert!(response.headers().get(header::ACCESS_CONTROL_ALLOW_METHODS).is_some());
    }

    #[tokio::test]
    async fn test_body_size_limit_within_limit() {
        let max_size = 1024;
        let app = Router::new()
            .route("/test", get(test_handler))
            .layer(middleware::from_fn(move |req, next| {
                body_size_limit_middleware(req, next, max_size)
            }));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header(header::CONTENT_LENGTH, "512")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_body_size_limit_exceeds_limit() {
        let max_size = 1024;
        let app = Router::new()
            .route("/test", get(test_handler))
            .layer(middleware::from_fn(move |req, next| {
                body_size_limit_middleware(req, next, max_size)
            }));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header(header::CONTENT_LENGTH, "2048")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn test_body_size_limit_no_content_length() {
        let max_size = 1024;
        let app = Router::new()
            .route("/test", get(test_handler))
            .layer(middleware::from_fn(move |req, next| {
                body_size_limit_middleware(req, next, max_size)
            }));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_body_size_limit_at_limit() {
        let max_size = 1024;
        let app = Router::new()
            .route("/test", get(test_handler))
            .layer(middleware::from_fn(move |req, next| {
                body_size_limit_middleware(req, next, max_size)
            }));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header(header::CONTENT_LENGTH, "1024")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_body_size_limit_malformed_content_length() {
        let max_size = 1024;
        let app = Router::new()
            .route("/test", get(test_handler))
            .layer(middleware::from_fn(move |req, next| {
                body_size_limit_middleware(req, next, max_size)
            }));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header(header::CONTENT_LENGTH, "invalid")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // Should pass through when Content-Length is malformed
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_extract_metadata_with_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("traceparent", "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".parse().unwrap());
        headers.insert(header::USER_AGENT, "test-agent/1.0".parse().unwrap());

        let metadata = extract_metadata(&headers);
        assert!(metadata.is_some());

        let meta = metadata.unwrap();
        assert_eq!(meta.user_agent, Some("test-agent/1.0".to_string()));
    }

    #[tokio::test]
    async fn test_extract_metadata_empty_headers() {
        let headers = HeaderMap::new();
        let metadata = extract_metadata(&headers);
        assert!(metadata.is_some());

        let meta = metadata.unwrap();
        assert_eq!(meta.user_agent, None);
    }

    #[tokio::test]
    async fn test_request_context_multiple_forwarded_ips() {
        let app = Router::new()
            .route("/test", get(test_handler))
            .layer(middleware::from_fn(request_context_middleware));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header("x-forwarded-for", "203.0.113.1, 198.51.100.1, 192.0.2.1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        // The middleware should extract the first IP (203.0.113.1)
    }

    #[tokio::test]
    async fn test_request_context_x_real_ip_fallback() {
        let app = Router::new()
            .route("/test", get(test_handler))
            .layer(middleware::from_fn(request_context_middleware));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header("x-real-ip", "192.0.2.1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("x-request-id").is_some());
        // X-Real-IP should be extracted as client IP
    }

    #[tokio::test]
    async fn test_request_context_user_agent() {
        let app = Router::new()
            .route("/test", get(test_handler))
            .layer(middleware::from_fn(request_context_middleware));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header(header::USER_AGENT, "Mozilla/5.0 Test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("x-request-id").is_some());
        // User-Agent should be extracted
    }
}

