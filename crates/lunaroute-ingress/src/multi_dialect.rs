//! Multi-Dialect Ingress Router
//!
//! Provides a unified router that accepts both OpenAI and Anthropic API formats simultaneously.
//! Routes are determined by the incoming endpoint path:
//! - /v1/chat/completions → OpenAI format
//! - /v1/messages → Anthropic format

use axum::Router;
use lunaroute_core::{SessionStore, provider::Provider};
use lunaroute_egress::{anthropic::AnthropicConnector, openai::OpenAIConnector};
use lunaroute_observability::Metrics;
use std::sync::Arc;

use crate::types::SessionStatsTracker;

/// Create a dual-dialect router that accepts both OpenAI and Anthropic formats
///
/// This creates a merged router with both:
/// - POST /v1/chat/completions (OpenAI format)
/// - POST /v1/messages (Anthropic format)
/// - GET /v1/models (OpenAI format)
///
/// The router will automatically route to the appropriate provider based on
/// the endpoint being called and any routing rules configured.
pub fn router(provider: Arc<dyn Provider>) -> Router {
    // Create both OpenAI and Anthropic routers
    let openai_router = crate::openai::router(provider.clone());
    let anthropic_router = crate::anthropic::router(provider);

    // Merge them together - Axum will handle routing based on path
    openai_router.merge(anthropic_router)
}

/// Create a dual-dialect passthrough router
///
/// This is used when:
/// - Both OpenAI and Anthropic connectors are available
/// - We want zero-copy passthrough for both formats
/// - Model-based routing determines which connector to use
///
/// # Passthrough Logic
/// - OpenAI format requests at /v1/chat/completions can be routed to either:
///   - OpenAI connector (if model matches gpt-*)
///   - Anthropic connector (if model matches claude-*, with normalization)
/// - Anthropic format requests at /v1/messages can be routed to either:
///   - Anthropic connector (if model matches claude-*)
///   - OpenAI connector (if model matches gpt-*, with normalization)
///
/// However, for true passthrough (no normalization), we need to route:
/// - OpenAI format → OpenAI provider for gpt-* models
/// - Anthropic format → Anthropic provider for claude-* models
///
/// The routing layer handles this automatically based on model patterns.
pub fn passthrough_router(
    openai_connector: Option<Arc<OpenAIConnector>>,
    anthropic_connector: Option<Arc<AnthropicConnector>>,
    stats_tracker: Option<Arc<dyn SessionStatsTracker>>,
    metrics: Option<Arc<Metrics>>,
    session_store: Option<Arc<dyn SessionStore>>,
    sse_keepalive_interval_secs: u64,
    sse_keepalive_enabled: bool,
) -> Router {
    let mut router = Router::new();

    // Add OpenAI passthrough routes if connector available
    if let Some(connector) = openai_connector {
        let openai_router = crate::openai::passthrough_router(
            connector,
            stats_tracker.clone(),
            metrics.clone(),
            session_store.clone(),
            sse_keepalive_interval_secs,
            sse_keepalive_enabled,
        );
        router = router.merge(openai_router);
    }

    // Add Anthropic passthrough routes if connector available
    if let Some(connector) = anthropic_connector {
        let anthropic_router = crate::anthropic::passthrough_router(
            connector,
            stats_tracker,
            metrics,
            session_store,
            sse_keepalive_interval_secs,
            sse_keepalive_enabled,
        );
        router = router.merge(anthropic_router);
    }

    router
}
