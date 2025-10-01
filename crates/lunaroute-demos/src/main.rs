//! LunaRoute Demo Server with Intelligent Routing and Session Recording
//!
//! This example demonstrates LunaRoute's features:
//! - Accepts OpenAI-compatible requests on /v1/chat/completions
//! - Routes to OpenAI or Anthropic based on model name
//! - Automatic fallback if primary provider fails
//! - Circuit breakers prevent repeated failures
//! - Health monitoring tracks provider status
//! - Session recording captures all requests/responses with GDPR-compliant IP anonymization
//! - Session query endpoints for debugging and analytics
//!
//! Usage:
//! ```bash
//! # Requires both API keys for full functionality
//! OPENAI_API_KEY=your_key ANTHROPIC_API_KEY=your_key cargo run --package lunaroute-demos
//!
//! # OpenAI only (Anthropic as fallback will fail)
//! OPENAI_API_KEY=your_key cargo run --package lunaroute-demos
//! ```
//!
//! Test with:
//! ```bash
//! # OpenAI GPT-5 mini (routes to OpenAI)
//! curl http://localhost:3000/v1/chat/completions \
//!   -H "Content-Type: application/json" \
//!   -d '{
//!     "model": "gpt-5-mini",
//!     "messages": [{"role": "user", "content": "Hello from GPT!"}]
//!   }'
//!
//! # Claude Sonnet 4.5 (routes to Anthropic, falls back to OpenAI if unavailable)
//! curl http://localhost:3000/v1/chat/completions \
//!   -H "Content-Type: application/json" \
//!   -d '{
//!     "model": "claude-sonnet-4-5",
//!     "messages": [{"role": "user", "content": "Hello from Claude!"}]
//!   }'
//!
//! # Streaming request
//! curl http://localhost:3000/v1/chat/completions \
//!   -H "Content-Type: application/json" \
//!   -d '{
//!     "model": "gpt-5-mini",
//!     "messages": [{"role": "user", "content": "Count to 5"}],
//!     "stream": true
//!   }'
//! ```

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router as AxumRouter,
};
use lunaroute_core::provider::Provider;
use lunaroute_egress::{
    anthropic::{AnthropicConfig, AnthropicConnector},
    openai::{OpenAIConfig, OpenAIConnector},
};
use lunaroute_ingress::openai;
use lunaroute_observability::{health_router, HealthState, Metrics};
use lunaroute_routing::{Router, RouteTable, RoutingRule, RuleMatcher};
use lunaroute_session::{FileSessionRecorder, RecordingProvider, SessionQuery, SessionRecorder};
use serde::Deserialize;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{info, warn, Level};
use tracing_subscriber::FmtSubscriber;

// Query parameters for session list
#[derive(Deserialize)]
struct SessionQueryParams {
    provider: Option<String>,
    model: Option<String>,
    success: Option<bool>,
    streaming: Option<bool>,
    limit: Option<usize>,
}

// Handler for listing sessions
async fn list_sessions(
    State(recorder): State<Arc<FileSessionRecorder>>,
    Query(params): Query<SessionQueryParams>,
) -> impl IntoResponse {
    let mut query = SessionQuery::new();

    if let Some(provider) = params.provider {
        query = query.provider(provider);
    }
    if let Some(model) = params.model {
        query = query.model(model);
    }
    if let Some(success) = params.success {
        query = query.success(success);
    }
    if let Some(streaming) = params.streaming {
        query = query.streaming(streaming);
    }
    if let Some(limit) = params.limit {
        query = query.limit(limit);
    }

    match recorder.query_sessions(&query).await {
        Ok(sessions) => (StatusCode::OK, Json(sessions)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to query sessions: {}", e),
        )
            .into_response(),
    }
}

// Handler for getting a specific session
async fn get_session(
    State(recorder): State<Arc<FileSessionRecorder>>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    match recorder.get_session(&session_id).await {
        Ok(Some(session)) => (StatusCode::OK, Json(session)).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "Session not found").into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get session: {}", e),
        )
            .into_response(),
    }
}

// Create session router
fn session_router(recorder: Arc<FileSessionRecorder>) -> AxumRouter {
    AxumRouter::new()
        .route("/sessions", get(list_sessions))
        .route("/sessions/:session_id", get(get_session))
        .with_state(recorder)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    info!("🚀 Initializing LunaRoute Gateway with Intelligent Routing");

    // Check if session recording is enabled
    let enable_session_recording = std::env::var("ENABLE_SESSION_RECORDING")
        .unwrap_or_else(|_| "true".to_string())
        .parse::<bool>()
        .unwrap_or(true);

    let log_requests = std::env::var("LOG_REQUESTS")
        .unwrap_or_else(|_| "false".to_string())
        .parse::<bool>()
        .unwrap_or(false);

    // Setup session recording if enabled
    let recorder = if enable_session_recording {
        let sessions_dir = std::env::var("SESSIONS_DIR")
            .unwrap_or_else(|_| "./sessions".to_string());
        info!("📝 Session recording enabled: {}", sessions_dir);
        Some(Arc::new(FileSessionRecorder::new(&sessions_dir)))
    } else {
        info!("📝 Session recording disabled");
        None
    };

    if log_requests {
        info!("📋 Request/response logging enabled (stdout)");
    }

    // Setup providers
    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();

    // OpenAI provider
    if let Ok(api_key) = std::env::var("OPENAI_API_KEY") {
        info!("✓ OpenAI API key found - enabling OpenAI provider");
        let config = OpenAIConfig::new(api_key);
        let connector = Arc::new(OpenAIConnector::new(config)?);

        // Wrap with session recording
        let recording_provider = RecordingProvider::new(
            connector,
            recorder.clone(),
            "openai".to_string(),
            "openai".to_string(),
        );
        providers.insert("openai".to_string(), Arc::new(recording_provider));
    } else {
        warn!("✗ OPENAI_API_KEY not set - OpenAI provider disabled");
    }

    // Anthropic provider
    if let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") {
        info!("✓ Anthropic API key found - enabling Anthropic provider");
        let config = AnthropicConfig {
            api_key,
            base_url: "https://api.anthropic.com".to_string(),
            api_version: "2023-06-01".to_string(),
            client_config: Default::default(),
        };
        let connector = Arc::new(AnthropicConnector::new(config)?);

        // Wrap with session recording
        let recording_provider = RecordingProvider::new(
            connector,
            recorder.clone(),
            "anthropic".to_string(),
            "openai".to_string(), // Listener is OpenAI (ingress format)
        );
        providers.insert("anthropic".to_string(), Arc::new(recording_provider));
    } else {
        warn!("✗ ANTHROPIC_API_KEY not set - Anthropic provider disabled");
    }

    if providers.is_empty() {
        return Err("No API keys provided. Set OPENAI_API_KEY and/or ANTHROPIC_API_KEY".into());
    }

    // Create routing rules
    let mut rules = vec![];

    // Route GPT models to OpenAI with Anthropic fallback
    if providers.contains_key("openai") {
        rules.push(RoutingRule {
            priority: 10,
            name: Some("gpt-to-openai".to_string()),
            matcher: RuleMatcher::model_pattern("^gpt-.*"),
            primary: "openai".to_string(),
            fallbacks: if providers.contains_key("anthropic") {
                vec!["anthropic".to_string()]
            } else {
                vec![]
            },
        });
    }

    // Route Claude models to Anthropic with OpenAI fallback
    if providers.contains_key("anthropic") {
        rules.push(RoutingRule {
            priority: 10,
            name: Some("claude-to-anthropic".to_string()),
            matcher: RuleMatcher::model_pattern("^claude-.*"),
            primary: "anthropic".to_string(),
            fallbacks: if providers.contains_key("openai") {
                vec!["openai".to_string()]
            } else {
                vec![]
            },
        });
    }

    // Default fallback route (catches all)
    rules.push(RoutingRule {
        priority: 1,
        name: Some("default-route".to_string()),
        matcher: RuleMatcher::Always,
        primary: if providers.contains_key("openai") {
            "openai".to_string()
        } else {
            "anthropic".to_string()
        },
        fallbacks: vec![],
    });

    info!("📋 Created {} routing rules", rules.len());
    for rule in &rules {
        info!("   - {:?}: {:?} → {} (fallbacks: {:?})",
            rule.name, rule.matcher, rule.primary, rule.fallbacks);
    }

    // Create router with routing table
    let route_table = RouteTable::with_rules(rules);
    let router = Arc::new(Router::with_defaults(route_table, providers));

    info!("✓ Router created with health monitoring and circuit breakers");
    info!("   Circuit breaker: 3 failures → open, 1 success → close");
    info!("   Health monitor: tracks success rate and recent failures");

    // Initialize observability
    info!("📊 Initializing observability (metrics, health endpoints)");
    let metrics = Arc::new(Metrics::new()?);
    let health_state = HealthState::new(metrics.clone());

    // Create OpenAI ingress router with the intelligent router
    let api_router = openai::router(router);

    // Create health/metrics router
    let health_router = health_router(health_state);

    // Create session query router
    let session_router = session_router(recorder.clone());

    // Combine routers
    let app = api_router.merge(health_router).merge(session_router);

    // Start server
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    let listener = TcpListener::bind(addr).await?;

    info!("");
    info!("✅ LunaRoute gateway listening on http://{}", addr);
    info!("   API endpoints:");
    info!("   - OpenAI-compatible: http://{}/v1/chat/completions", addr);
    info!("   Observability endpoints:");
    info!("   - Health check:       http://{}/healthz", addr);
    info!("   - Readiness check:    http://{}/readyz", addr);
    info!("   - Prometheus metrics: http://{}/metrics", addr);
    info!("   Session endpoints:");
    info!("   - List sessions:      http://{}/sessions?provider=openai&limit=10", addr);
    info!("   - Get session:        http://{}/sessions/<session-id>", addr);
    info!("   Supported models: GPT-5 (gpt-5-mini), Claude Sonnet 4.5 (claude-sonnet-4-5)");
    info!("   Features: Intelligent routing, automatic fallback, circuit breakers, session recording");
    info!("");

    axum::serve(listener, app).await?;

    Ok(())
}
