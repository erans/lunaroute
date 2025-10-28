//! Integration tests for the bypass feature
//!
//! Tests that unknown API paths are bypassed directly to the provider
//! without going through the routing engine.

use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use lunaroute_ingress::{BypassProvider, with_bypass};
use lunaroute_routing::PathClassifier;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower::ServiceExt;

/// Mock provider server for testing
async fn mock_provider_server() -> String {
    let app = Router::new()
        .route("/v1/embeddings", post(handle_embeddings))
        .route("/v1/audio/transcriptions", post(handle_audio))
        .route("/v1/images/generations", post(handle_images))
        .route("/v1/chat/completions", post(handle_chat_completions));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    format!("http://127.0.0.1:{}", addr.port())
}

async fn handle_embeddings() -> Response {
    (
        StatusCode::OK,
        r#"{"object":"list","data":[{"object":"embedding","embedding":[0.1,0.2,0.3],"index":0}],"model":"text-embedding-3-small","usage":{"prompt_tokens":5,"total_tokens":5}}"#,
    )
        .into_response()
}

async fn handle_audio() -> Response {
    (
        StatusCode::OK,
        r#"{"text":"Hello, this is a transcription test."}"#,
    )
        .into_response()
}

async fn handle_images() -> Response {
    (
        StatusCode::OK,
        r#"{"created":1234567890,"data":[{"url":"https://example.com/image.png"}]}"#,
    )
        .into_response()
}

async fn handle_chat_completions() -> Response {
    (
        StatusCode::OK,
        r#"{"id":"chatcmpl-123","object":"chat.completion","created":1234567890,"model":"gpt-4","choices":[{"index":0,"message":{"role":"assistant","content":"Hello!"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#,
    )
        .into_response()
}

#[tokio::test]
async fn test_bypass_enabled_proxies_embeddings() {
    // Start mock provider
    let provider_url = mock_provider_server().await;
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Create bypass provider
    let bypass_provider = Arc::new(BypassProvider::new(
        provider_url.clone(),
        "test-key".to_string(),
        "test-provider".to_string(),
        Arc::new(reqwest::Client::new()),
    ));

    // Create path classifier with bypass enabled
    let classifier = Arc::new(PathClassifier::new(true));

    // Create a simple router with no routes (all requests will go to fallback)
    let app = Router::new();

    // Wrap with bypass
    let app = with_bypass(app, Some(bypass_provider), classifier);

    // Test embeddings endpoint (should be bypassed)
    let request = Request::builder()
        .method(Method::POST)
        .uri("/v1/embeddings")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"model":"text-embedding-3-small","input":"test"}"#,
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    assert!(body_str.contains("embedding"));
}

#[tokio::test]
async fn test_bypass_enabled_proxies_audio() {
    // Start mock provider
    let provider_url = mock_provider_server().await;
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Create bypass provider
    let bypass_provider = Arc::new(BypassProvider::new(
        provider_url,
        "test-key".to_string(),
        "test-provider".to_string(),
        Arc::new(reqwest::Client::new()),
    ));

    // Create path classifier with bypass enabled
    let classifier = Arc::new(PathClassifier::new(true));

    // Create a simple router
    let app = Router::new();
    let app = with_bypass(app, Some(bypass_provider), classifier);

    // Test audio endpoint (should be bypassed)
    let request = Request::builder()
        .method(Method::POST)
        .uri("/v1/audio/transcriptions")
        .header("content-type", "multipart/form-data")
        .body(Body::from(r#"{"file":"audio.mp3"}"#))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    assert!(body_str.contains("transcription"));
}

#[tokio::test]
async fn test_bypass_enabled_proxies_images() {
    // Start mock provider
    let provider_url = mock_provider_server().await;
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Create bypass provider
    let bypass_provider = Arc::new(BypassProvider::new(
        provider_url,
        "test-key".to_string(),
        "test-provider".to_string(),
        Arc::new(reqwest::Client::new()),
    ));

    // Create path classifier with bypass enabled
    let classifier = Arc::new(PathClassifier::new(true));

    // Create a simple router
    let app = Router::new();
    let app = with_bypass(app, Some(bypass_provider), classifier);

    // Test images endpoint (should be bypassed)
    let request = Request::builder()
        .method(Method::POST)
        .uri("/v1/images/generations")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"prompt":"a cat","n":1}"#))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    assert!(body_str.contains("url"));
}

#[tokio::test]
async fn test_bypass_disabled_returns_404() {
    // Create path classifier with bypass DISABLED
    let classifier = Arc::new(PathClassifier::new(false));

    // Create a simple router with no routes
    let app = Router::new();
    let app = with_bypass(app, None, classifier);

    // Test embeddings endpoint (should return 404 since bypass is disabled)
    let request = Request::builder()
        .method(Method::POST)
        .uri("/v1/embeddings")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"model":"test","input":"test"}"#))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    // Should return 404 because bypass is disabled and no route matches
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_intercepted_path_not_bypassed() {
    // Start mock provider
    let provider_url = mock_provider_server().await;
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Create bypass provider
    let bypass_provider = Arc::new(BypassProvider::new(
        provider_url,
        "test-key".to_string(),
        "test-provider".to_string(),
        Arc::new(reqwest::Client::new()),
    ));

    // Create path classifier with bypass enabled
    let classifier = Arc::new(PathClassifier::new(true));

    // Create a router with a handler for chat/completions (intercepted path)
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|| async { (StatusCode::OK, "from router handler") }),
    );

    let app = with_bypass(app, Some(bypass_provider), classifier);

    // Test chat/completions endpoint (should go through router, not bypass)
    let request = Request::builder()
        .method(Method::POST)
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"model":"gpt-4","messages":[]}"#))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body.to_vec()).unwrap();

    // Should get response from router handler, not bypassed to provider
    assert_eq!(body_str, "from router handler");
}

#[tokio::test]
async fn test_unknown_intercepted_path_returns_404() {
    // Create path classifier with bypass enabled
    let classifier = Arc::new(PathClassifier::new(true));

    // Create bypass provider
    let bypass_provider = Arc::new(BypassProvider::new(
        "http://localhost:9999".to_string(),
        "test-key".to_string(),
        "test-provider".to_string(),
        Arc::new(reqwest::Client::new()),
    ));

    // Create a router with no routes
    let app = Router::new();
    let app = with_bypass(app, Some(bypass_provider), classifier);

    // Test /healthz (intercepted path but no handler)
    let request = Request::builder()
        .method(Method::GET)
        .uri("/healthz")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    // Should return 404 because it's an intercepted path but no handler exists
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
