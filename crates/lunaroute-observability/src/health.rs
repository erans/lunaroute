//! Health endpoints
//!
//! This module provides HTTP health check endpoints:
//! - `/healthz` - Liveness probe (always returns 200 OK if server is running)
//! - `/readyz` - Readiness probe (checks provider availability)
//! - `/metrics` - Prometheus metrics endpoint

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use prometheus::TextEncoder;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::metrics::Metrics;

/// Health check response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    /// Service status
    pub status: String,
    /// Optional message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Readiness check response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadinessResponse {
    /// Service status
    pub status: String,
    /// Provider statuses
    #[serde(skip_serializing_if = "Option::is_none")]
    pub providers: Option<Vec<ProviderStatus>>,
    /// Optional message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Provider status in readiness check
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderStatus {
    /// Provider name
    pub name: String,
    /// Provider status
    pub status: String,
    /// Success rate (0.0-1.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success_rate: Option<f64>,
}

/// Readiness checker trait
pub trait ReadinessChecker: Send + Sync {
    /// Check if the service is ready
    fn is_ready(&self) -> bool;

    /// Get provider statuses
    fn get_provider_statuses(&self) -> Vec<ProviderStatus>;
}

/// Health check state
#[derive(Clone)]
pub struct HealthState {
    /// Metrics collector
    pub metrics: Arc<Metrics>,
    /// Optional readiness checker
    pub readiness_checker: Option<Arc<dyn ReadinessChecker>>,
}

impl HealthState {
    /// Create a new health state
    pub fn new(metrics: Arc<Metrics>) -> Self {
        Self {
            metrics,
            readiness_checker: None,
        }
    }

    /// Create a new health state with readiness checker
    pub fn with_readiness_checker(
        metrics: Arc<Metrics>,
        readiness_checker: Arc<dyn ReadinessChecker>,
    ) -> Self {
        Self {
            metrics,
            readiness_checker: Some(readiness_checker),
        }
    }
}

/// Create health check router
pub fn health_router(state: HealthState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics_handler))
        .with_state(state)
}

/// Liveness probe handler
///
/// Returns 200 OK if the server is running
async fn healthz() -> impl IntoResponse {
    Json(HealthResponse {
        status: "ok".to_string(),
        message: None,
    })
}

/// Readiness probe handler
///
/// Returns 200 OK if the service is ready to accept requests
/// Returns 503 Service Unavailable if not ready
async fn readyz(State(state): State<HealthState>) -> Response {
    if let Some(checker) = &state.readiness_checker {
        if checker.is_ready() {
            let providers = checker.get_provider_statuses();
            (
                StatusCode::OK,
                Json(ReadinessResponse {
                    status: "ready".to_string(),
                    providers: Some(providers),
                    message: None,
                }),
            )
                .into_response()
        } else {
            let providers = checker.get_provider_statuses();
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ReadinessResponse {
                    status: "not_ready".to_string(),
                    providers: Some(providers),
                    message: Some("One or more providers are unavailable".to_string()),
                }),
            )
                .into_response()
        }
    } else {
        // No readiness checker, assume ready
        (
            StatusCode::OK,
            Json(ReadinessResponse {
                status: "ready".to_string(),
                providers: None,
                message: None,
            }),
        )
            .into_response()
    }
}

/// Prometheus metrics handler
///
/// Returns metrics in Prometheus text format
async fn metrics_handler(State(state): State<HealthState>) -> Response {
    let encoder = TextEncoder::new();
    let metric_families = state.metrics.registry().gather();

    match encoder.encode_to_string(&metric_families) {
        Ok(body) => (
            StatusCode::OK,
            [("Content-Type", "text/plain; version=0.0.4")],
            body,
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to encode metrics: {}", err),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt; // for oneshot

    struct MockReadinessChecker {
        ready: bool,
    }

    impl ReadinessChecker for MockReadinessChecker {
        fn is_ready(&self) -> bool {
            self.ready
        }

        fn get_provider_statuses(&self) -> Vec<ProviderStatus> {
            vec![
                ProviderStatus {
                    name: "openai".to_string(),
                    status: if self.ready {
                        "healthy".to_string()
                    } else {
                        "unhealthy".to_string()
                    },
                    success_rate: Some(if self.ready { 0.95 } else { 0.0 }),
                },
            ]
        }
    }

    #[tokio::test]
    async fn test_healthz() {
        let metrics = Arc::new(Metrics::new().unwrap());
        let state = HealthState::new(metrics);
        let app = health_router(state);

        let response = app
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_readyz_ready() {
        let metrics = Arc::new(Metrics::new().unwrap());
        let checker = Arc::new(MockReadinessChecker { ready: true });
        let state = HealthState::with_readiness_checker(metrics, checker);
        let app = health_router(state);

        let response = app
            .oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_readyz_not_ready() {
        let metrics = Arc::new(Metrics::new().unwrap());
        let checker = Arc::new(MockReadinessChecker { ready: false });
        let state = HealthState::with_readiness_checker(metrics, checker);
        let app = health_router(state);

        let response = app
            .oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_readyz_no_checker() {
        let metrics = Arc::new(Metrics::new().unwrap());
        let state = HealthState::new(metrics);
        let app = health_router(state);

        let response = app
            .oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_metrics() {
        let metrics = Arc::new(Metrics::new().unwrap());
        let state = HealthState::new(metrics);
        let app = health_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "text/plain; version=0.0.4"
        );
    }

    #[test]
    fn test_health_response_serialization() {
        let response = HealthResponse {
            status: "ok".to_string(),
            message: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"status\":\"ok\""));
    }

    #[test]
    fn test_readiness_response_serialization() {
        let response = ReadinessResponse {
            status: "ready".to_string(),
            providers: Some(vec![ProviderStatus {
                name: "openai".to_string(),
                status: "healthy".to_string(),
                success_rate: Some(0.95),
            }]),
            message: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"status\":\"ready\""));
        assert!(json.contains("\"providers\""));
    }

    #[test]
    fn test_provider_status_serialization() {
        let status = ProviderStatus {
            name: "openai".to_string(),
            status: "healthy".to_string(),
            success_rate: Some(0.95),
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"name\":\"openai\""));
        assert!(json.contains("\"success_rate\":0.95"));
    }
}
