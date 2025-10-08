//! LunaRoute Production Server with Intelligent Routing
//!
//! This server provides:
//! - Accepts OpenAI-compatible requests on /v1/chat/completions
//! - Routes to OpenAI or Anthropic based on model name
//! - Automatic fallback if primary provider fails
//! - Circuit breakers prevent repeated failures
//! - Health monitoring tracks provider status
//! - Optional async session recording to JSONL and/or SQLite
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
//! curl http://localhost:8081/v1/chat/completions \
//!   -H "Content-Type: application/json" \
//!   -d '{
//!     "model": "gpt-5-mini",
//!     "messages": [{"role": "user", "content": "Hello from GPT!"}]
//!   }'
//!
//! # Claude Sonnet 4.5 (routes to Anthropic, falls back to OpenAI if unavailable)
//! curl http://localhost:8081/v1/chat/completions \
//!   -H "Content-Type: application/json" \
//!   -d '{
//!     "model": "claude-sonnet-4-5",
//!     "messages": [{"role": "user", "content": "Hello from Claude!"}]
//!   }'
//!
//! # Streaming request
//! curl http://localhost:8081/v1/chat/completions \
//!   -H "Content-Type: application/json" \
//!   -d '{
//!     "model": "gpt-5-mini",
//!     "messages": [{"role": "user", "content": "Count to 5"}],
//!     "stream": true
//!   }'
//! ```

mod config;
mod session_stats;

use clap::{Parser, Subcommand};
use config::{ApiDialect, ServerConfig};
use futures::StreamExt;
use lunaroute_core::provider::Provider;
use lunaroute_core::{
    error::Error as CoreError,
    normalized::{NormalizedRequest, NormalizedResponse, NormalizedStreamEvent},
};
use lunaroute_egress::{
    anthropic::{AnthropicConfig, AnthropicConnector},
    openai::{OpenAIConfig, OpenAIConnector, RequestBodyModConfig, ResponseBodyModConfig},
};
use lunaroute_ingress::{anthropic as anthropic_ingress, openai};
use lunaroute_observability::{HealthState, Metrics, health_router};
use lunaroute_routing::{RouteTable, Router, RoutingRule, RuleMatcher};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{Level, debug, info, warn};
use tracing_subscriber::{EnvFilter, FmtSubscriber};

const MOON: &str = r#"
         ___---___
      .--         --.
    ./   ()      .-. \.
   /   o    .   (   )  \
  / .            '-'    \    _                      ____             _
 | ()    .  O         .  |  | |   _   _ _ __   __ _|  _ \ ___  _   _| |_ ___
|                         | | |  | | | | '_ \ / _` | |_) / _ \| | | | __/ _ \
|    o           ()       | | |__| |_| | | | | (_| |  _ < (_) | |_| | ||  __/
|       .--.          O   | |_____\__,_|_| |_|\__,_|_| \_\___/ \__,_|\__\___|
 | .   |    |            |
  \    `.__.'    o   .  /   https://lunaroute.org
   \                   /    version : {VERSION}
    `\  o    ()      /      commit  : {SHA}
      `--___   ___--'
            ---
"#;

/// LunaRoute Server - Intelligent LLM API Gateway
#[derive(Parser)]
#[command(name = "lunaroute-server")]
#[command(about = "LunaRoute production server for LLM API routing", long_about = None)]
#[command(before_help = MOON)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to configuration file (YAML or TOML)
    #[arg(
        short,
        long,
        value_name = "FILE",
        env = "LUNAROUTE_CONFIG",
        global = true
    )]
    config: Option<String>,

    /// API dialect to accept (openai or anthropic)
    #[arg(
        short = 'd',
        long,
        value_name = "DIALECT",
        env = "LUNAROUTE_DIALECT",
        global = true
    )]
    dialect: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the LunaRoute server (default if no command specified)
    Serve,
    /// Import JSONL session logs into SQLite database
    ImportSessions {
        /// Path to JSONL sessions directory
        #[arg(long, default_value = "~/.lunaroute/sessions")]
        sessions_dir: PathBuf,

        /// Path to SQLite database
        #[arg(long, default_value = "~/.lunaroute/sessions.db")]
        db_path: PathBuf,

        /// Number of sessions to process in one batch
        #[arg(long, default_value = "10")]
        batch_size: usize,

        /// Skip sessions that already exist in database
        #[arg(long, default_value = "true")]
        skip_existing: bool,

        /// Continue importing even if some sessions fail
        #[arg(long, default_value = "true")]
        continue_on_error: bool,

        /// Show what would be imported without writing to DB
        #[arg(long, default_value = "false")]
        dry_run: bool,
    },
}

/// Logging provider that prints all requests and responses to stdout
struct LoggingProvider {
    inner: Arc<dyn Provider>,
    provider_name: String,
}

impl LoggingProvider {
    fn new(inner: Arc<dyn Provider>, provider_name: String) -> Self {
        Self {
            inner,
            provider_name,
        }
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
        debug!(
            "│ Full request:\n{}",
            serde_json::to_string_pretty(&request)
                .unwrap_or_else(|e| format!("Serialization error: {}", e))
        );
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
        info!(
            "│ Tokens: input={}, output={}, total={}",
            response.usage.prompt_tokens,
            response.usage.completion_tokens,
            response.usage.prompt_tokens + response.usage.completion_tokens
        );
        debug!(
            "│ Full response:\n{}",
            serde_json::to_string_pretty(&response)
                .unwrap_or_else(|e| format!("Serialization error: {}", e))
        );
        info!("└─────────────────────────────────────────────────────────");

        Ok(response)
    }

    async fn stream(
        &self,
        request: NormalizedRequest,
    ) -> Result<
        Box<dyn futures::Stream<Item = Result<NormalizedStreamEvent, CoreError>> + Send + Unpin>,
        CoreError,
    > {
        info!("┌─────────────────────────────────────────────────────────");
        info!("│ REQUEST to {} (streaming)", self.provider_name);
        info!("├─────────────────────────────────────────────────────────");
        info!("│ Model: {}", request.model);
        info!("│ Messages: {} messages", request.messages.len());
        debug!(
            "│ Full request:\n{}",
            serde_json::to_string_pretty(&request)
                .unwrap_or_else(|e| format!("Serialization error: {}", e))
        );
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
                        info!(
                            "│ 📊 Usage: input={}, output={}, total={}",
                            usage.prompt_tokens,
                            usage.completion_tokens,
                            usage.prompt_tokens + usage.completion_tokens
                        );
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

    // Handle subcommands
    match cli.command {
        Some(Commands::ImportSessions {
            sessions_dir,
            db_path,
            batch_size,
            skip_existing,
            continue_on_error,
            dry_run,
        }) => {
            // Expand tilde in paths
            let sessions_dir = shellexpand::tilde(&sessions_dir.to_string_lossy()).to_string();
            let db_path = shellexpand::tilde(&db_path.to_string_lossy()).to_string();

            let config = lunaroute_session::ImportConfig {
                sessions_dir: PathBuf::from(sessions_dir),
                db_path: PathBuf::from(db_path),
                batch_size,
                skip_existing,
                continue_on_error,
                dry_run,
            };

            lunaroute_session::import_sessions(config).await?;
            return Ok(());
        }
        Some(Commands::Serve) | None => {
            // Continue with server startup (default behavior)
        }
    }

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
                return Err(format!(
                    "Invalid dialect '{}'. Use 'openai' or 'anthropic'",
                    dialect_str
                )
                .into());
            }
        }
    }

    // Initialize tracing with configured level and sqlx query control
    let log_level = match config.logging.level.to_lowercase().as_str() {
        "trace" => Level::TRACE,
        "debug" => Level::DEBUG,
        "info" => Level::INFO,
        "warn" => Level::WARN,
        "error" => Level::ERROR,
        _ => Level::INFO,
    };

    // Build EnvFilter with base level
    let mut filter = EnvFilter::new(format!("{}", log_level));

    // By default, set sqlx to WARN to suppress query logs
    // Only enable DEBUG for sqlx if log_sql_queries is true
    if !config.logging.log_sql_queries {
        match "sqlx=warn".parse() {
            Ok(directive) => filter = filter.add_directive(directive),
            Err(e) => tracing::warn!("Failed to set sqlx log filter: {}", e),
        }
    }

    let subscriber = FmtSubscriber::builder().with_env_filter(filter).finish();
    tracing::subscriber::set_global_default(subscriber)?;

    // Print the moon
    println!("{}", MOON);

    info!("🚀 Initializing LunaRoute Gateway with Intelligent Routing");

    // Setup async multi-writer session recording if enabled
    let async_recorder: Option<Arc<lunaroute_session::MultiWriterRecorder>> =
        if config.session_recording.enabled {
            match lunaroute_session::build_from_config(&config.session_recording).await? {
                Some(multi_recorder) => {
                    if config.session_recording.is_jsonl_enabled() {
                        info!(
                            "📝 JSONL session recording enabled: {:?}",
                            config
                                .session_recording
                                .jsonl
                                .as_ref()
                                .map(|c| &c.directory)
                        );
                    }
                    if config.session_recording.is_sqlite_enabled() {
                        info!(
                            "📝 SQLite session recording enabled: {:?}",
                            config.session_recording.sqlite.as_ref().map(|c| &c.path)
                        );
                    }
                    Some(Arc::new(multi_recorder))
                }
                None => {
                    info!("📝 Session recording enabled but no writers configured");
                    None
                }
            }
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
        // Get API key (empty string if not configured - will use client's header)
        let api_key = openai_config.api_key.clone().unwrap_or_default();

        if api_key.is_empty() {
            info!("✓ OpenAI provider enabled (no API key - will use client auth)");
        } else {
            info!("✓ OpenAI provider enabled");
        }

        let base_url = openai_config
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());

        // Use http_client config from YAML if provided, otherwise use defaults
        let client_config = openai_config
            .http_client
            .as_ref()
            .map(|c| c.to_http_client_config())
            .unwrap_or_default();

        let mut provider_config = OpenAIConfig {
            api_key: api_key.clone(),
            base_url,
            organization: None,
            client_config,
            custom_headers: None,
            request_body_config: None,
            response_body_config: None,
        };

        // Wire custom headers and body modifications
        if let Some(headers_config) = &openai_config.request_headers {
            provider_config.custom_headers = Some(headers_config.headers.clone());
        }
        if let Some(request_body) = &openai_config.request_body {
            provider_config.request_body_config = Some(RequestBodyModConfig {
                defaults: request_body.defaults.clone(),
                overrides: request_body.overrides.clone(),
                prepend_messages: request_body.prepend_messages.clone(),
            });
        }
        if let Some(response_body) = &openai_config.response_body {
            provider_config.response_body_config = Some(ResponseBodyModConfig {
                enabled: response_body.enabled,
                metadata_namespace: response_body.metadata_namespace.clone(),
                fields: response_body.fields.clone(),
                extension_fields: response_body.extension_fields.clone(),
            });
        }

        let conn = OpenAIConnector::new(provider_config)?;

        // Build the provider stack (order matters!)
        // 1. Start with connector
        // 2. Wrap with session recording if enabled
        // 3. Wrap with logging if enabled
        let connector = Arc::new(conn);
        openai_connector = Some(connector.clone()); // Save for passthrough
        // Session recording is now handled via async multi-writer in passthrough mode
        let provider: Arc<dyn Provider> = if config.logging.log_requests {
            info!("  Request/response logging: enabled");
            Arc::new(LoggingProvider::new(
                connector.clone(),
                "OpenAI".to_string(),
            ))
        } else {
            connector
        };

        providers.insert("openai".to_string(), provider);
    }

    // Anthropic provider
    if let Some(anthropic_config) = &config.providers.anthropic
        && anthropic_config.enabled
    {
        // Get API key (empty string if not configured - will use client's header)
        let api_key = anthropic_config.api_key.clone().unwrap_or_default();

        if api_key.is_empty() {
            info!("✓ Anthropic provider enabled (no API key - will use client auth)");
        } else {
            info!("✓ Anthropic provider enabled");
        }

        let base_url = anthropic_config
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.anthropic.com".to_string());

        // Use http_client config from YAML if provided, otherwise use defaults
        let client_config = anthropic_config
            .http_client
            .as_ref()
            .map(|c| c.to_http_client_config())
            .unwrap_or_default();

        let provider_config = AnthropicConfig {
            api_key: api_key.clone(),
            base_url,
            api_version: "2023-06-01".to_string(),
            client_config,
        };
        let conn = AnthropicConnector::new(provider_config)?;

        // Build the provider stack (order matters!)
        // Session recording is now handled via async multi-writer in passthrough mode
        let connector = Arc::new(conn);
        anthropic_connector = Some(connector.clone()); // Save for passthrough
        let provider: Arc<dyn Provider> = if config.logging.log_requests {
            info!("  Request/response logging: enabled");
            Arc::new(LoggingProvider::new(
                connector.clone(),
                "Anthropic".to_string(),
            ))
        } else {
            connector
        };

        providers.insert("anthropic".to_string(), provider);
    }

    // Allow starting without providers in certain scenarios (e.g., passthrough with client-provided keys)
    if providers.is_empty() {
        warn!("⚠️  No providers configured - requests will fail unless using passthrough mode");
        warn!("    To configure providers, either:");
        warn!("    - Set OPENAI_API_KEY or ANTHROPIC_API_KEY environment variables, or");
        warn!("    - Add api_key field to provider configuration in config file");
    }

    // Create routing rules
    let mut rules = vec![];

    // Route GPT models to OpenAI with Anthropic fallback
    if providers.contains_key("openai") {
        rules.push(RoutingRule {
            priority: 10,
            name: Some("gpt-to-openai".to_string()),
            matcher: RuleMatcher::model_pattern("^gpt-.*"),
            strategy: None,
            primary: Some("openai".to_string()),
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
            strategy: None,
            primary: Some("anthropic".to_string()),
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
        strategy: None,
        primary: Some(if providers.contains_key("openai") {
            "openai".to_string()
        } else {
            "anthropic".to_string()
        }),
        fallbacks: vec![],
    });

    info!("📋 Created {} routing rules", rules.len());
    for rule in &rules {
        info!(
            "   - {:?}: {:?} → {:?} (fallbacks: {:?})",
            rule.name, rule.matcher, rule.primary, rule.fallbacks
        );
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
    info!(
        "📊 Session statistics tracking enabled (max {} sessions)",
        config.session_stats_max_sessions.unwrap_or(100)
    );

    // Create ingress router based on selected dialect
    let api_router = match config.api_dialect {
        ApiDialect::OpenAI => {
            info!("📡 API dialect: OpenAI (/v1/chat/completions)");
            if is_openai_passthrough && openai_connector.is_some() {
                info!("⚡ Passthrough mode: OpenAI→OpenAI (no normalization)");
                openai::passthrough_router(
                    openai_connector.unwrap(),
                    Some(stats_tracker_clone),
                    Some(metrics.clone()),
                    async_recorder.clone(),
                )
            } else {
                openai::router(router)
            }
        }
        ApiDialect::Anthropic => {
            info!("📡 API dialect: Anthropic (/v1/messages)");
            if is_anthropic_passthrough {
                if let Some(connector) = anthropic_connector {
                    info!("⚡ Passthrough mode: Anthropic→Anthropic (no normalization)");
                    anthropic_ingress::passthrough_router(
                        connector,
                        Some(stats_tracker_clone),
                        Some(metrics.clone()),
                        async_recorder.clone(),
                    )
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

    // Combine routers
    let app = api_router.merge(health_router);

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
            info!(
                "   💡 For Claude Code: export ANTHROPIC_BASE_URL=http://{}",
                addr
            );
        }
    }
    info!("   Observability:");
    info!("   - Health check:       http://{}/healthz", addr);
    info!("   - Readiness check:    http://{}/readyz", addr);
    info!("   - Prometheus metrics: http://{}/metrics", addr);
    info!("");

    // Setup graceful shutdown handler
    let stats_tracker_for_shutdown = stats_tracker.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        // Only print stats if debug logging is enabled
        if tracing::level_filters::LevelFilter::current()
            >= tracing::level_filters::LevelFilter::DEBUG
        {
            info!("Shutdown signal received, printing session statistics...");
            stats_tracker_for_shutdown.print_summary();
        }
    });

    // Start UI server if enabled and SQLite is configured
    if config.ui.enabled {
        if let Some(sqlite_config) = &config.session_recording.sqlite {
            let db_path = sqlite_config.path.to_string_lossy().to_string();
            let ui_config = config.ui.clone();

            tokio::spawn(async move {
                match start_ui_server(ui_config, db_path).await {
                    Ok(_) => info!("UI server stopped"),
                    Err(e) => warn!("UI server error: {}", e),
                }
            });
        } else {
            warn!("📊 UI server enabled but SQLite session recording is not configured");
            warn!("   Enable SQLite session recording to use the UI dashboard");
        }
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    // Print stats one final time in case the shutdown handler didn't run
    // Only print if debug logging is enabled
    if tracing::level_filters::LevelFilter::current() >= tracing::level_filters::LevelFilter::DEBUG
    {
        info!("Server stopped, printing final session statistics...");
        stats_tracker.print_summary();
    }

    Ok(())
}

/// Start the UI server
async fn start_ui_server(config: lunaroute_ui::UiConfig, db_path: String) -> anyhow::Result<()> {
    // Connect to SQLite database
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&format!("sqlite://{}", db_path))
        .await?;

    let pool = std::sync::Arc::new(pool);

    // Create and start UI server
    let ui_server = lunaroute_ui::UiServer::new(config, pool);
    ui_server.serve().await?;

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
