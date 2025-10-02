//! LunaRoute Production Server with Intelligent Routing
//!
//! This server provides:
//! - Accepts OpenAI-compatible requests on /v1/chat/completions
//! - Routes to OpenAI or Anthropic based on model name
//! - Automatic fallback if primary provider fails
//! - Circuit breakers prevent repeated failures
//! - Health monitoring tracks provider status
//! - Optional session recording with GDPR-compliant IP anonymization
//! - Session query endpoints for debugging and analytics
//!
//! Usage:
//! ```bash
//! # With config file
//! lunaroute-server --config config.yaml
//!
//! # Or with environment variables
//! ANTHROPIC_API_KEY=your_key lunaroute-server
//!
//! # With both (env vars override config)
//! ANTHROPIC_API_KEY=your_key lunaroute-server --config config.yaml
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

mod config;
mod session_stats;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router as AxumRouter,
};
use clap::Parser;
use config::{ApiDialect, ServerConfig};
use lunaroute_core::provider::Provider;
use lunaroute_egress::{
    anthropic::{AnthropicConfig, AnthropicConnector},
    openai::{OpenAIConfig, OpenAIConnector},
};
use lunaroute_ingress::{anthropic as anthropic_ingress, openai};
use lunaroute_observability::{health_router, HealthState, Metrics};
use lunaroute_routing::{Router, RouteTable, RoutingRule, RuleMatcher};
use lunaroute_session::{FileSessionRecorder, RecordingProvider, SessionQuery, SessionRecorder};
use serde::Deserialize;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{debug, info, warn, Level};
use tracing_subscriber::FmtSubscriber;
use lunaroute_core::{
    normalized::{NormalizedRequest, NormalizedResponse, NormalizedStreamEvent},
    error::Error as CoreError,
};
use futures::StreamExt;

const MOON: &str = r#"
         ___---___
      .--         --.
    ./   ()      .-. \.
   /   o    .   (   )  \
  / .            '-'    \
 | ()    .  O         .  |
|                         |
|    o           ()       |
|       .--.          O   |
 | .   |    |            |
  \    `.__.'    o   .  /
   \                   /
    `\  o    ()      /
      `--___   ___--'
            ---
"#;

/// LunaRoute Server - Intelligent LLM API Gateway
#[derive(Parser)]
#[command(name = "lunaroute-server")]
#[command(about = "LunaRoute production server for LLM API routing", long_about = None)]
#[command(before_help = MOON)]
struct Cli {
    /// Path to configuration file (YAML or TOML)
    #[arg(short, long, value_name = "FILE", env = "LUNAROUTE_CONFIG")]
    config: Option<String>,

    /// API dialect to accept (openai or anthropic)
    #[arg(short = 'd', long, value_name = "DIALECT", env = "LUNAROUTE_DIALECT")]
    dialect: Option<String>,
}

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

/// Logging provider that prints all requests and responses to stdout
struct LoggingProvider {
    inner: Arc<dyn Provider>,
    provider_name: String,
}

impl LoggingProvider {
    fn new(inner: Arc<dyn Provider>, provider_name: String) -> Self {
        Self { inner, provider_name }
    }
}

#[async_trait::async_trait]
impl Provider for LoggingProvider {
    async fn send(&self, request: NormalizedRequest) -> Result<NormalizedResponse, CoreError> {
        info!("┌─────────────────────────────────────────────────────────");
        info!("│ REQUEST to {} (non-streaming)", self.provider_name);
        info!("├─────────────────────────────────────────────────────────");
        info!("│ Model: {}", request.model);
        info!("│ Messages: {} messages", request.messages.len());
        debug!("│ Full request:\n{}", serde_json::to_string_pretty(&request).unwrap_or_else(|e| format!("Serialization error: {}", e)));
        info!("└─────────────────────────────────────────────────────────");

        let response = self.inner.send(request).await?;

        info!("┌─────────────────────────────────────────────────────────");
        info!("│ RESPONSE from {} (non-streaming)", self.provider_name);
        info!("├─────────────────────────────────────────────────────────");
        if !response.choices.is_empty() {
            let message = &response.choices[0].message;
            // MessageContent is an enum, check if it's text
            if let lunaroute_core::normalized::MessageContent::Text(text) = &message.content {
                info!("│ Content: {}", text);
            }
        }
        info!("│ Tokens: input={}, output={}, total={}",
            response.usage.prompt_tokens, response.usage.completion_tokens,
            response.usage.prompt_tokens + response.usage.completion_tokens);
        debug!("│ Full response:\n{}", serde_json::to_string_pretty(&response).unwrap_or_else(|e| format!("Serialization error: {}", e)));
        info!("└─────────────────────────────────────────────────────────");

        Ok(response)
    }

    async fn stream(
        &self,
        request: NormalizedRequest,
    ) -> Result<Box<dyn futures::Stream<Item = Result<NormalizedStreamEvent, CoreError>> + Send + Unpin>, CoreError> {
        info!("┌─────────────────────────────────────────────────────────");
        info!("│ REQUEST to {} (streaming)", self.provider_name);
        info!("├─────────────────────────────────────────────────────────");
        info!("│ Model: {}", request.model);
        info!("│ Messages: {} messages", request.messages.len());
        debug!("│ Full request:\n{}", serde_json::to_string_pretty(&request).unwrap_or_else(|e| format!("Serialization error: {}", e)));
        info!("└─────────────────────────────────────────────────────────");

        let stream = self.inner.stream(request).await?;
        let provider_name = self.provider_name.clone();

        info!("┌─────────────────────────────────────────────────────────");
        info!("│ STREAMING from {}", provider_name);
        info!("└─────────────────────────────────────────────────────────");

        // Wrap stream to log each event
        let logged_stream = stream.map(move |event| {
            if let Ok(ref evt) = event {
                match evt {
                    NormalizedStreamEvent::Start { .. } => {
                        debug!("│ 🟢 Stream started");
                    }
                    NormalizedStreamEvent::Delta { delta, .. } => {
                        if let Some(ref content) = delta.content {
                            info!("│ 📝 {}", content);
                        }
                    }
                    NormalizedStreamEvent::ToolCallDelta { function, .. } => {
                        if let Some(func) = function {
                            if let Some(name) = &func.name {
                                info!("│ 🔧 Tool call: {}", name);
                            }
                            if let Some(args) = &func.arguments {
                                debug!("│ 🔧 Tool args delta: {}", args);
                            }
                        }
                    }
                    NormalizedStreamEvent::Usage { usage } => {
                        info!("│ 📊 Usage: input={}, output={}, total={}",
                            usage.prompt_tokens, usage.completion_tokens,
                            usage.prompt_tokens + usage.completion_tokens);
                    }
                    NormalizedStreamEvent::End { finish_reason } => {
                        info!("│ 🏁 Stream ended: {:?}", finish_reason);
                    }
                    NormalizedStreamEvent::Error { error } => {
                        warn!("│ ❌ Stream error: {}", error);
                    }
                }
            }
            event
        });

        Ok(Box::new(logged_stream))
    }

    fn capabilities(&self) -> lunaroute_core::provider::ProviderCapabilities {
        self.inner.capabilities()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse CLI arguments
    let cli = Cli::parse();

    // Load configuration
    let mut config = if let Some(config_path) = cli.config {
        info!("📁 Loading configuration from: {}", config_path);
        ServerConfig::from_file(&config_path)?
    } else {
        info!("📁 Using default configuration");
        ServerConfig::default()
    };

    // Merge environment variables (they override config file)
    config.merge_env();

    // Apply CLI dialect override (highest precedence)
    if let Some(ref dialect_str) = cli.dialect {
        match dialect_str.to_lowercase().as_str() {
            "openai" => config.api_dialect = ApiDialect::OpenAI,
            "anthropic" => config.api_dialect = ApiDialect::Anthropic,
            _ => {
                return Err(format!("Invalid dialect '{}'. Use 'openai' or 'anthropic'", dialect_str).into());
            }
        }
    }

    // Initialize tracing with configured level
    let log_level = match config.logging.level.to_lowercase().as_str() {
        "trace" => Level::TRACE,
        "debug" => Level::DEBUG,
        "info" => Level::INFO,
        "warn" => Level::WARN,
        "error" => Level::ERROR,
        _ => Level::INFO,
    };
    let subscriber = FmtSubscriber::builder()
        .with_max_level(log_level)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    // Print the moon
    println!("{}", MOON);

    info!("🚀 Initializing LunaRoute Gateway with Intelligent Routing");

    // Setup session recording if enabled
    let recorder = if config.session_recording.enabled {
        info!("📝 Session recording enabled: {}", config.session_recording.directory);
        Some(Arc::new(FileSessionRecorder::new(&config.session_recording.directory)))
    } else {
        info!("📝 Session recording disabled");
        None
    };

    if config.logging.log_requests {
        info!("📋 Request/response logging enabled (stdout)");
    }

    // Setup providers
    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();

    // Store raw connectors for passthrough mode
    let mut anthropic_connector: Option<Arc<AnthropicConnector>> = None;
    let mut openai_connector: Option<Arc<OpenAIConnector>> = None;

    // OpenAI provider
    if let Some(openai_config) = &config.providers.openai
        && openai_config.enabled
    {
        if let Some(ref api_key) = openai_config.api_key {
                info!("✓ OpenAI provider enabled");
                let provider_config = OpenAIConfig::new(api_key.clone());
                let conn = OpenAIConnector::new(provider_config)?;

                // Build the provider stack (order matters!)
                // 1. Start with connector
                // 2. Wrap with session recording if enabled
                // 3. Wrap with logging if enabled
                let connector = Arc::new(conn);
                openai_connector = Some(connector.clone()); // Save for passthrough
                let provider: Arc<dyn Provider> = if let Some(ref rec) = recorder {
                    let recording = RecordingProvider::new(
                        connector.clone(),
                        rec.clone(),
                        "openai".to_string(),
                        "openai".to_string(),
                    );
                    if config.logging.log_requests {
                        info!("  Request/response logging: enabled");
                        Arc::new(LoggingProvider::new(Arc::new(recording), "OpenAI".to_string()))
                    } else {
                        Arc::new(recording)
                    }
                } else if config.logging.log_requests {
                    info!("  Request/response logging: enabled");
                    Arc::new(LoggingProvider::new(connector.clone(), "OpenAI".to_string()))
                } else {
                    connector
                };

            providers.insert("openai".to_string(), provider);
        } else {
            warn!("✗ OpenAI provider enabled but no API key provided");
        }
    }

    // Anthropic provider
    if let Some(anthropic_config) = &config.providers.anthropic
        && anthropic_config.enabled
    {
        if let Some(ref api_key) = anthropic_config.api_key {
                info!("✓ Anthropic provider enabled");
                let base_url = anthropic_config
                    .base_url
                    .clone()
                    .unwrap_or_else(|| "https://api.anthropic.com".to_string());

                let provider_config = AnthropicConfig {
                    api_key: api_key.clone(),
                    base_url,
                    api_version: "2023-06-01".to_string(),
                    client_config: Default::default(),
                };
                let conn = AnthropicConnector::new(provider_config)?;

                // Build the provider stack (order matters!)
                // 1. Start with connector
                // 2. Wrap with session recording if enabled
                // 3. Wrap with logging if enabled
                let connector = Arc::new(conn);
                anthropic_connector = Some(connector.clone()); // Save for passthrough
                let provider: Arc<dyn Provider> = if let Some(ref rec) = recorder {
                    let recording = RecordingProvider::new(
                        connector.clone(),
                        rec.clone(),
                        "anthropic".to_string(),
                        "openai".to_string(), // Listener is OpenAI (ingress format)
                    );
                    if config.logging.log_requests {
                        info!("  Request/response logging: enabled");
                        Arc::new(LoggingProvider::new(Arc::new(recording), "Anthropic".to_string()))
                    } else {
                        Arc::new(recording)
                    }
                } else if config.logging.log_requests {
                    info!("  Request/response logging: enabled");
                    Arc::new(LoggingProvider::new(connector.clone(), "Anthropic".to_string()))
                } else {
                    connector
                };

            providers.insert("anthropic".to_string(), provider);
        } else {
            warn!("✗ Anthropic provider enabled but no API key provided");
        }
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

    // Detect passthrough mode BEFORE creating router: dialect matches the only enabled provider
    // This skips normalization for optimal performance and 100% API fidelity
    let is_anthropic_passthrough = config.api_dialect == ApiDialect::Anthropic
        && anthropic_connector.is_some()
        && providers.len() == 1
        && providers.contains_key("anthropic");

    let is_openai_passthrough = config.api_dialect == ApiDialect::OpenAI
        && openai_connector.is_some()
        && providers.len() == 1
        && providers.contains_key("openai");

    let is_passthrough = is_anthropic_passthrough || is_openai_passthrough;

    // Create router with routing table (not needed in passthrough mode, but keep for consistency)
    let route_table = RouteTable::with_rules(rules);
    let router = Arc::new(Router::with_defaults(route_table, providers));

    if !is_passthrough {
        info!("✓ Router created with health monitoring and circuit breakers");
        info!("   Circuit breaker: 3 failures → open, 1 success → close");
        info!("   Health monitor: tracks success rate and recent failures");
    }

    // Initialize observability
    info!("📊 Initializing observability (metrics, health endpoints)");
    let metrics = Arc::new(Metrics::new()?);
    let health_state = HealthState::new(metrics.clone());

    // Initialize session statistics tracker
    let stats_tracker = Arc::new(session_stats::SessionStatsTracker::new(
        session_stats::SessionStatsConfig {
            max_sessions: config.session_stats_max_sessions.unwrap_or(100),
        },
    ));
    let stats_tracker_clone = stats_tracker.clone();
    info!("📊 Session statistics tracking enabled (max {} sessions)", config.session_stats_max_sessions.unwrap_or(100));

    // Create ingress router based on selected dialect
    let api_router = match config.api_dialect {
        ApiDialect::OpenAI => {
            info!("📡 API dialect: OpenAI (/v1/chat/completions)");
            if is_openai_passthrough && openai_connector.is_some() {
                info!("⚡ Passthrough mode: OpenAI→OpenAI (no normalization)");
                // TODO: Implement OpenAI passthrough
                openai::router(router)
            } else {
                openai::router(router)
            }
        }
        ApiDialect::Anthropic => {
            info!("📡 API dialect: Anthropic (/v1/messages)");
            if is_anthropic_passthrough {
                if let Some(connector) = anthropic_connector {
                    info!("⚡ Passthrough mode: Anthropic→Anthropic (no normalization)");
                    anthropic_ingress::passthrough_router(connector, Some(stats_tracker_clone), Some(metrics.clone()))
                } else {
                    anthropic_ingress::router(router)
                }
            } else {
                anthropic_ingress::router(router)
            }
        }
    };

    // Create health/metrics router
    let health_router = health_router(health_state);

    // Combine routers - optionally include session query endpoints
    let app = if let Some(recorder) = recorder {
        let session_router = session_router(recorder);
        api_router
            .merge(health_router)
            .merge(session_router)
    } else {
        api_router
            .merge(health_router)
    };

    // Start server
    let addr: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    let listener = TcpListener::bind(addr).await?;

    info!("");
    info!("✅ LunaRoute gateway listening on http://{}", addr);
    info!("   API endpoint:");
    match config.api_dialect {
        ApiDialect::OpenAI => {
            info!("   - OpenAI API: http://{}/v1/chat/completions", addr);
        }
        ApiDialect::Anthropic => {
            info!("   - Anthropic API: http://{}/v1/messages", addr);
            info!("   💡 For Claude Code: export ANTHROPIC_BASE_URL=http://{}", addr);
        }
    }
    info!("   Observability:");
    info!("   - Health check:       http://{}/healthz", addr);
    info!("   - Readiness check:    http://{}/readyz", addr);
    info!("   - Prometheus metrics: http://{}/metrics", addr);
    if config.session_recording.enabled {
        info!("   Session recording:");
        info!("   - List sessions: http://{}/sessions?limit=10", addr);
        info!("   - Get session:   http://{}/sessions/<session-id>", addr);
    }
    info!("");

    // Setup graceful shutdown handler
    let stats_tracker_for_shutdown = stats_tracker.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        // Only print stats if debug logging is enabled
        if tracing::level_filters::LevelFilter::current() >= tracing::level_filters::LevelFilter::DEBUG {
            info!("Shutdown signal received, printing session statistics...");
            stats_tracker_for_shutdown.print_summary();
        }
    });

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    // Print stats one final time in case the shutdown handler didn't run
    // Only print if debug logging is enabled
    if tracing::level_filters::LevelFilter::current() >= tracing::level_filters::LevelFilter::DEBUG {
        info!("Server stopped, printing final session statistics...");
        stats_tracker.print_summary();
    }

    Ok(())
}

/// Wait for shutdown signal (SIGINT or SIGTERM)
async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutdown signal received");
}
