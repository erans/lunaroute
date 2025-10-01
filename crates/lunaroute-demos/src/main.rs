//! LunaRoute Demo Server with Intelligent Routing
//!
//! This example demonstrates LunaRoute's intelligent routing features:
//! - Accepts OpenAI-compatible requests on /v1/chat/completions
//! - Routes to OpenAI or Anthropic based on model name
//! - Automatic fallback if primary provider fails
//! - Circuit breakers prevent repeated failures
//! - Health monitoring tracks provider status
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

use lunaroute_core::provider::Provider;
use lunaroute_egress::{
    anthropic::{AnthropicConfig, AnthropicConnector},
    openai::{OpenAIConfig, OpenAIConnector},
};
use lunaroute_ingress::openai;
use lunaroute_routing::{Router, RouteTable, RoutingRule, RuleMatcher};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{info, warn, Level};
use tracing_subscriber::FmtSubscriber;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    info!("🚀 Initializing LunaRoute Gateway with Intelligent Routing");

    // Setup providers
    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();

    // OpenAI provider
    if let Ok(api_key) = std::env::var("OPENAI_API_KEY") {
        info!("✓ OpenAI API key found - enabling OpenAI provider");
        let config = OpenAIConfig::new(api_key);
        let connector = OpenAIConnector::new(config)?;
        providers.insert("openai".to_string(), Arc::new(connector));
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
        let connector = AnthropicConnector::new(config)?;
        providers.insert("anthropic".to_string(), Arc::new(connector));
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

    // Create OpenAI ingress router with the intelligent router
    let app = openai::router(router);

    // Start server
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    let listener = TcpListener::bind(addr).await?;

    info!("");
    info!("✅ LunaRoute gateway listening on http://{}", addr);
    info!("   OpenAI-compatible endpoint: http://{}/v1/chat/completions", addr);
    info!("   Supported models: GPT-5 (gpt-5-mini), Claude Sonnet 4.5 (claude-sonnet-4-5)");
    info!("   Features: Intelligent routing, automatic fallback, circuit breakers");
    info!("");

    axum::serve(listener, app).await?;

    Ok(())
}
