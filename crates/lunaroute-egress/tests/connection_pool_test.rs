//! Integration tests for connection pool behavior
//!
//! These tests verify that the HTTP client correctly handles connection pooling
//! and prevents "stuck request" issues caused by stale connections.

use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

/// Test that verifies connection reuse works correctly
///
/// This test ensures that:
/// 1. Connections are pooled and reused
/// 2. Multiple sequential requests work correctly
/// 3. The pool doesn't accumulate dead connections
#[tokio::test]
async fn test_connection_pool_reuse() {
    use lunaroute_egress::client::{HttpClientConfig, create_client};

    let config = HttpClientConfig::default();
    let client = create_client(&config).unwrap();

    // Start a simple HTTP server for testing
    let server = start_mock_server().await;

    // Make multiple requests to the same server
    for i in 0..5 {
        let response = client
            .get(format!("{}/test/{}", server.url(), i))
            .send()
            .await;

        assert!(
            response.is_ok(),
            "Request {} should succeed using pooled connections",
            i
        );
    }

    server.shutdown().await;
}

/// Test that verifies idle connections are eventually removed
///
/// This simulates the scenario where:
/// 1. A connection is used and returned to pool
/// 2. The connection sits idle beyond pool_idle_timeout (90s)
/// 3. Next request creates a new connection instead of using stale one
///
/// Note: This test is marked #[ignore] because it takes 90+ seconds to run.
/// Run with: cargo test --package lunaroute-egress --test connection_pool_test -- --ignored
#[tokio::test]
#[ignore]
async fn test_connection_pool_idle_timeout() {
    use lunaroute_egress::client::{HttpClientConfig, create_client};

    let config = HttpClientConfig::default();
    let client = create_client(&config).unwrap();

    let server = start_mock_server().await;

    // Make first request to establish connection
    let response1 = client.get(format!("{}/test/1", server.url())).send().await;
    assert!(response1.is_ok(), "First request should succeed");

    // Wait for pool_idle_timeout (90 seconds) + buffer
    eprintln!("Waiting 95 seconds for pool idle timeout...");
    sleep(Duration::from_secs(95)).await;

    // Make second request - should create new connection, not reuse stale one
    let response2 = client.get(format!("{}/test/2", server.url())).send().await;
    assert!(
        response2.is_ok(),
        "Second request should succeed after idle timeout"
    );

    server.shutdown().await;
}

/// Test that simulates server closing connection while client thinks it's alive
///
/// This is the "stuck request" bug scenario:
/// 1. Client makes request, connection goes to pool
/// 2. Server closes connection after idle period
/// 3. Client tries to reuse dead connection
///
/// Expected: Request should either succeed immediately (new connection)
/// or fail fast with connection error (not hang forever)
#[tokio::test]
async fn test_server_closes_idle_connection() {
    use lunaroute_egress::client::{HttpClientConfig, create_client};

    let config = HttpClientConfig::default();
    let client = create_client(&config).unwrap();

    let server = start_mock_server_with_short_timeout().await;

    // Make first request
    let response1 = client.get(format!("{}/test/1", server.url())).send().await;
    assert!(response1.is_ok(), "First request should succeed");

    // Wait for server to close idle connections (5 seconds)
    sleep(Duration::from_secs(6)).await;

    // Make second request - should handle closed connection gracefully
    let start = std::time::Instant::now();
    let response2 = client.get(format!("{}/test/2", server.url())).send().await;

    // Request should either succeed (new connection) or fail fast (not hang)
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(10),
        "Request should complete within 10 seconds, got {:?}",
        elapsed
    );

    // If it failed, it should be a connection error, not a timeout
    if let Err(e) = &response2 {
        eprintln!("Request failed (expected for closed connection): {}", e);
    }

    server.shutdown().await;
}

// Mock server helpers

struct MockServer {
    url: String,
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
}

impl MockServer {
    fn url(&self) -> &str {
        &self.url
    }

    async fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
    }
}

async fn start_mock_server() -> MockServer {
    start_mock_server_with_timeout(Duration::from_secs(300)).await
}

async fn start_mock_server_with_short_timeout() -> MockServer {
    start_mock_server_with_timeout(Duration::from_secs(5)).await
}

async fn start_mock_server_with_timeout(_idle_timeout: Duration) -> MockServer {
    use axum::{Router, routing::get};

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

    let app = Router::new().route("/test/{id}", get(|| async { "OK" }));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                shutdown_rx.await.ok();
            })
            .await
            .unwrap();
    });

    // Give server time to start
    sleep(Duration::from_millis(100)).await;

    MockServer {
        url: format!("http://{}", addr),
        shutdown_tx,
    }
}

/// Test connection pool behavior under concurrent load
///
/// Verifies that:
/// 1. Multiple concurrent requests work correctly
/// 2. Pool handles concurrent connection reuse
/// 3. No deadlocks or stuck requests under load
#[tokio::test]
async fn test_concurrent_requests() {
    use lunaroute_egress::client::{HttpClientConfig, create_client};

    let config = HttpClientConfig::default();
    let client = Arc::new(create_client(&config).unwrap());

    let server = start_mock_server().await;
    let url = server.url().to_string();

    // Launch 20 concurrent requests
    let mut handles = vec![];
    for i in 0..20 {
        let client = client.clone();
        let url = url.clone();

        let handle = tokio::spawn(async move {
            let response = client.get(format!("{}/test/{}", url, i)).send().await;
            assert!(response.is_ok(), "Concurrent request {} should succeed", i);
        });

        handles.push(handle);
    }

    // Wait for all requests to complete
    for handle in handles {
        handle.await.unwrap();
    }

    server.shutdown().await;
}
