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

mod app;
mod bootstrap;
mod config;
mod session_factory;
mod session_stats;

use clap::{Parser, Subcommand};
use config::{ApiDialect, ServerConfig};
use futures::StreamExt;
use lunaroute_config_file::FileConfigStore;
use lunaroute_core::provider::Provider;
use lunaroute_core::{
    config_store::ConfigStore,
    error::Error as CoreError,
    normalized::{NormalizedRequest, NormalizedResponse, NormalizedStreamEvent},
    session_store::SessionStore,
};
use lunaroute_egress::{
    anthropic::{AnthropicConfig, AnthropicConnector},
    openai::{OpenAIConfig, OpenAIConnector, RequestBodyModConfig, ResponseBodyModConfig},
};
use lunaroute_ingress::{BypassProvider, anthropic as anthropic_ingress, openai, with_bypass};
use lunaroute_observability::{HealthState, Metrics, health_router};
use lunaroute_routing::{PathClassifier, RouteTable, Router, RoutingRule, RuleMatcher};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{Level, debug, info, warn};
use tracing_subscriber::{EnvFilter, FmtSubscriber};

const VERSION: &str = env!("VERSION");
const SHA: &str = env!("SHA");

// Create version string at compile time using concat!
const VERSION_STRING: &str = concat!(env!("VERSION"), " (", env!("SHA"), ")");

fn moon_banner() -> String {
    format!(
        r#"
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
   \                   /    version : {}
    `\  o    ()      /      commit  : {}
      `--___   ___--'
            ---
"#,
        VERSION, SHA
    )
}

/// LunaRoute Server - Intelligent LLM API Gateway
#[derive(Parser)]
#[command(name = "lunaroute-server")]
#[command(about = "LunaRoute production server for LLM API routing", long_about = None)]
#[command(version = VERSION_STRING)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to bootstrap configuration file (YAML or TOML)
    /// Determines whether to use file-based or database-backed configuration
    #[arg(
        short = 'b',
        long,
        value_name = "FILE",
        env = "LUNAROUTE_BOOTSTRAP",
        global = true
    )]
    bootstrap: Option<String>,

    /// Path to configuration file (YAML or TOML)
    /// Only used in file-based mode (ignored if bootstrap specifies database mode)
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
    /// Start server in background and output shell commands to set environment variables
    /// Usage: eval $(lunaroute-server env)
    Env {
        /// Host to bind to (default: 127.0.0.1)
        #[arg(long)]
        host: Option<String>,

        /// Port to bind to (default: 8081)
        #[arg(long)]
        port: Option<u16>,
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

/// Handle the 'env' subcommand - start server in background and output export commands
fn handle_env_command(
    host: Option<String>,
    port: Option<u16>,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::Duration;

    let host = host.unwrap_or_else(|| "127.0.0.1".to_string());
    let port = port.unwrap_or(8081);

    // Get the current executable path
    let exe_path = std::env::current_exe()?;

    // Build the command to start the server
    let mut cmd = Command::new(&exe_path);
    cmd.arg("serve")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null());

    // Add environment variables if they exist in the current environment
    if let Ok(val) = std::env::var("LUNAROUTE_HOST") {
        cmd.env("LUNAROUTE_HOST", val);
    } else {
        cmd.env("LUNAROUTE_HOST", &host);
    }

    if let Ok(val) = std::env::var("LUNAROUTE_PORT") {
        cmd.env("LUNAROUTE_PORT", val);
    } else {
        cmd.env("LUNAROUTE_PORT", port.to_string());
    }

    // Preserve other relevant environment variables
    for (key, value) in std::env::vars() {
        if key.starts_with("LUNAROUTE_") || key == "OPENAI_API_KEY" || key == "ANTHROPIC_API_KEY" {
            cmd.env(key, value);
        }
    }

    // Spawn the server process in background
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                // Create a new session to detach from parent
                libc::setsid();
                Ok(())
            });
        }
    }

    let child = cmd.spawn()?;

    // Detach the child process so it continues running
    #[cfg(unix)]
    {
        // The process is already detached via setsid()
        // Just forget about it so we don't wait for it
        std::mem::forget(child);
    }

    #[cfg(not(unix))]
    {
        // On non-Unix systems, just spawn and forget
        std::mem::forget(child);
    }

    // Wait briefly for server to start
    thread::sleep(Duration::from_millis(500));

    // Try to connect to verify server is running
    let addr = format!("{}:{}", host, port);
    let mut retries = 0;
    let max_retries = 10;

    while retries < max_retries {
        match std::net::TcpStream::connect(&addr) {
            Ok(_) => break,
            Err(_) => {
                retries += 1;
                if retries < max_retries {
                    thread::sleep(Duration::from_millis(200));
                }
            }
        }
    }

    if retries == max_retries {
        eprintln!("# Warning: Could not verify server started on {}", addr);
        eprintln!("# Check logs for errors");
    }

    // Output shell export commands
    println!("export ANTHROPIC_BASE_URL=http://{}:{}", host, port);
    println!("export OPENAI_BASE_URL=http://{}:{}/v1", host, port);
    eprintln!("# LunaRoute server started on http://{}:{}", host, port);
    eprintln!("# Web UI available at http://{}:8082", host);

    Ok(())
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
        Some(Commands::Env { host, port }) => {
            // Start server in background and output shell export commands
            handle_env_command(host, port)?;
            return Ok(());
        }
        Some(Commands::Serve) | None => {
            // Continue with server startup (default behavior)
        }
    }

    // ============================================================================
    // PHASE 6: Bootstrap configuration - determines config source
    // ============================================================================

    // Load bootstrap configuration
    let bootstrap_path = cli
        .bootstrap
        .as_deref()
        .unwrap_or("~/.lunaroute/bootstrap.yaml");

    let bootstrap = match bootstrap::BootstrapConfig::from_file(bootstrap_path) {
        Ok(config) => {
            info!("📋 Loaded bootstrap config from: {}", bootstrap_path);
            config
        }
        Err(e) => {
            // If bootstrap config doesn't exist, use default (file-based mode)
            info!(
                "📋 No bootstrap config found ({}), using default file-based mode",
                e
            );
            bootstrap::BootstrapConfig::default()
        }
    };

    // Load configuration based on bootstrap source
    let mut config: ServerConfig;
    let config_store: Option<Arc<dyn ConfigStore>>;
    let tenant_id: Option<lunaroute_core::tenant::TenantId>;

    match bootstrap.source {
        bootstrap::ConfigSource::File => {
            info!("📁 Config source: File-based (single-tenant)");

            // Determine config file path (CLI > bootstrap > default)
            let file_path = cli
                .config
                .clone()
                .or_else(|| {
                    bootstrap
                        .file_path
                        .as_ref()
                        .map(|p| p.to_string_lossy().to_string())
                })
                .unwrap_or_else(|| "~/.lunaroute/config.yaml".to_string());

            // Load config from file
            config = match ServerConfig::from_file(&file_path) {
                Ok(c) => {
                    info!("✓ Loaded configuration from: {}", file_path);
                    c
                }
                Err(e) => {
                    warn!("Failed to load config from {}: {}", file_path, e);
                    info!("Using default configuration");
                    ServerConfig::default()
                }
            };

            // Create FileConfigStore
            config_store = match FileConfigStore::new(&file_path).await {
                Ok(store) => {
                    info!("✓ Initialized FileConfigStore");
                    Some(Arc::new(store))
                }
                Err(e) => {
                    warn!("Failed to create FileConfigStore: {}", e);
                    warn!("Config hot-reloading will not be available");
                    None
                }
            };

            tenant_id = None;
        }

        bootstrap::ConfigSource::Database => {
            #[cfg(feature = "postgres")]
            {
                info!("📁 Config source: PostgreSQL database");

                let database_url = bootstrap
                    .database_url
                    .as_ref()
                    .ok_or_else(|| "database_url is required for database mode".to_string())?;

                // Create PostgresConfigStore
                let pg_store = lunaroute_config_postgres::PostgresConfigStore::new(database_url)
                    .await
                    .map_err(|e| format!("Failed to create PostgresConfigStore: {}", e))?;

                info!("✓ Connected to PostgreSQL config store");

                // Convert bootstrap tenant_id to TenantId
                tenant_id = bootstrap
                    .tenant_id
                    .map(lunaroute_core::tenant::TenantId::from_uuid);

                if let Some(ref tid) = tenant_id {
                    info!("✓ Single-tenant database mode: {}", tid);
                } else {
                    info!("✓ Multi-tenant database mode (tenant from request)");
                }

                // Load config from database
                let config_json = pg_store
                    .get_config(tenant_id)
                    .await
                    .map_err(|e| format!("Failed to load config from database: {}", e))?;

                // Parse config JSON into ServerConfig
                config = serde_json::from_value(config_json)
                    .map_err(|e| format!("Failed to parse config: {}", e))?;

                info!("✓ Loaded configuration from database");

                config_store = Some(Arc::new(pg_store));
            }

            #[cfg(not(feature = "postgres"))]
            {
                return Err(
                    "Database-backed configuration requires the 'postgres' feature. \
                     Build with: cargo build --features postgres"
                        .into(),
                );
            }
        }
    }

    // Merge environment variables (they override config file/database)
    config.merge_env();

    // ============================================================================
    // PHASE 7: Initialize session store
    // ============================================================================
    let session_store: Option<Arc<dyn SessionStore>> = if config.session_recording.enabled
        && config.session_recording.has_writers()
    {
        match session_factory::create_session_store(&config.session_recording).await {
            Ok(store) => {
                info!("✓ Initialized session store");
                Some(store)
            }
            Err(e) => {
                warn!(
                    "Failed to create session store: {}. Session recording will not be available.",
                    e
                );
                None
            }
        }
    } else {
        None
    };

    // Create AppState if both stores are available
    // Clone session_store for passthrough routers before creating AppState
    let session_store_for_passthrough = session_store.clone();
    let _app_state: Option<app::AppState> = if let (Some(config_st), Some(session_st)) =
        (config_store, session_store)
    {
        let state = app::AppState::new(
            config_st, session_st, tenant_id, // Use tenant_id from bootstrap config
        );
        if tenant_id.is_some() {
            info!("✓ Initialized AppState with trait-based stores (single-tenant database mode)");
        } else {
            info!("✓ Initialized AppState with trait-based stores");
        }
        Some(state)
    } else {
        info!(
            "ℹ️  AppState not initialized (stores unavailable). Using legacy configuration and session recording."
        );
        None
    };

    // Note: AppState is currently unused but provides infrastructure for future integration.
    // Routes will be updated in a future phase to use trait-based stores via AppState.
    // For now, the existing direct config and session recording logic remains active.
    // ============================================================================

    // Apply CLI dialect override (highest precedence)
    if let Some(ref dialect_str) = cli.dialect {
        match dialect_str.to_lowercase().as_str() {
            "openai" => config.api_dialect = ApiDialect::OpenAI,
            "anthropic" => config.api_dialect = ApiDialect::Anthropic,
            "both" => config.api_dialect = ApiDialect::Both,
            _ => {
                return Err(format!(
                    "Invalid dialect '{}'. Use 'openai', 'anthropic', or 'both'",
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
    println!("{}", moon_banner());

    info!("🚀 Initializing LunaRoute Gateway with Intelligent Routing");

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
            codex_auth: openai_config.codex_auth.as_ref().map(|c| {
                lunaroute_egress::openai::CodexAuthConfig {
                    enabled: c.enabled,
                    auth_file: c.auth_file.clone(),
                    token_field: c.token_field.clone(),
                    account_id: c.account_id.clone(),
                }
            }),
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

        let conn = OpenAIConnector::new(provider_config).await?;

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

    // For dual-dialect mode, enable passthrough if BOTH connectors are available
    // This allows OpenAI→OpenAI and Anthropic→Anthropic passthrough simultaneously
    let is_dual_passthrough = config.api_dialect == ApiDialect::Both
        && openai_connector.is_some()
        && anthropic_connector.is_some();

    let is_passthrough = is_anthropic_passthrough || is_openai_passthrough || is_dual_passthrough;

    // Capture bypass provider info BEFORE connectors are moved
    let bypass_provider_info = if config.bypass.enabled {
        // Determine which provider to use for bypass
        let provider_name = if let Some(name) = &config.bypass.provider {
            Some(name.as_str())
        } else if openai_connector.is_some() {
            Some("openai")
        } else if anthropic_connector.is_some() {
            Some("anthropic")
        } else {
            None
        };

        match provider_name {
            Some("openai") if openai_connector.is_some() => {
                let base_url = config
                    .providers
                    .openai
                    .as_ref()
                    .and_then(|p| p.base_url.clone())
                    .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
                let api_key = config
                    .providers
                    .openai
                    .as_ref()
                    .and_then(|p| p.api_key.clone())
                    .unwrap_or_default();

                Some(("openai".to_string(), base_url, api_key))
            }
            Some("anthropic") if anthropic_connector.is_some() => {
                let base_url = config
                    .providers
                    .anthropic
                    .as_ref()
                    .and_then(|p| p.base_url.clone())
                    .unwrap_or_else(|| "https://api.anthropic.com/v1".to_string());
                let api_key = config
                    .providers
                    .anthropic
                    .as_ref()
                    .and_then(|p| p.api_key.clone())
                    .unwrap_or_default();

                Some(("anthropic".to_string(), base_url, api_key))
            }
            _ => None,
        }
    } else {
        None
    };

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
                    session_store_for_passthrough.clone(),
                    config.http_server.sse_keepalive_interval_secs,
                    config.http_server.sse_keepalive_enabled,
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
                        session_store_for_passthrough.clone(),
                        config.http_server.sse_keepalive_interval_secs,
                        config.http_server.sse_keepalive_enabled,
                    )
                } else {
                    anthropic_ingress::router(router)
                }
            } else {
                anthropic_ingress::router(router)
            }
        }
        ApiDialect::Both => {
            info!("📡 API dialect: Both (OpenAI + Anthropic)");
            info!("   - OpenAI format:   /v1/chat/completions");
            info!("   - Anthropic format: /v1/messages");

            if is_dual_passthrough {
                info!(
                    "⚡ Dual passthrough mode: OpenAI→OpenAI + Anthropic→Anthropic (no normalization)"
                );
                info!("   Routes determined by model prefix:");
                info!("   - gpt-* models    → OpenAI provider (passthrough)");
                info!("   - claude-* models → Anthropic provider (passthrough)");

                lunaroute_ingress::multi_dialect::passthrough_router(
                    openai_connector,
                    anthropic_connector,
                    Some(stats_tracker_clone),
                    Some(metrics.clone()),
                    session_store_for_passthrough.clone(),
                    config.http_server.sse_keepalive_interval_secs,
                    config.http_server.sse_keepalive_enabled,
                )
            } else {
                info!("🔄 Dual dialect with routing (normalization may occur)");
                lunaroute_ingress::multi_dialect::router(router)
            }
        }
    };

    // Initialize bypass functionality (if enabled)
    let path_classifier = Arc::new(PathClassifier::new(config.bypass.enabled));

    if config.bypass.enabled {
        info!("🚀 Bypass enabled for unknown API paths");
        info!(
            "   Intercepted paths: /v1/chat/completions, /v1/messages, /v1/models, /healthz, /readyz, /metrics"
        );
        info!("   Bypassed paths: /v1/embeddings, /v1/audio/*, /v1/images/*, and others");
    }

    // Create bypass provider from captured info
    let bypass_provider = bypass_provider_info.map(|(name, base_url, api_key)| {
        info!("   Bypass provider: {} ({})", name, base_url);
        Arc::new(BypassProvider::new(
            base_url,
            api_key,
            name,
            Arc::new(reqwest::Client::new()),
        ))
    });

    if config.bypass.enabled && bypass_provider.is_none() {
        warn!("⚠️  Bypass enabled but no valid provider configured. Bypass will be disabled.");
    }

    // Wrap api_router with bypass functionality
    let api_router = with_bypass(api_router, bypass_provider, path_classifier);

    // Create health/metrics router
    let health_router = health_router(health_state);

    // Combine routers
    let app = api_router.merge(health_router);

    // Start server
    let addr: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    let listener = TcpListener::bind(addr).await?;

    // Apply HTTP server TCP settings
    let http_config = &config.http_server;

    // Set TCP_NODELAY to disable Nagle's algorithm for low-latency SSE
    if http_config.tcp_nodelay {
        // Note: tokio's TcpListener doesn't expose set_nodelay directly
        // The nodelay setting will be applied per-connection in the accept loop if needed
        // For now, we'll document this as a known limitation
        info!("  HTTP Server Config:");
        info!(
            "    TCP_NODELAY: {} (applied per-connection)",
            http_config.tcp_nodelay
        );
    }

    if let Some(size) = http_config.send_buffer_size {
        info!("    Send buffer: {} bytes", size);
    }

    if let Some(size) = http_config.recv_buffer_size {
        info!("    Receive buffer: {} bytes", size);
    }

    info!(
        "    SSE keepalive: {}s (enabled: {})",
        http_config.sse_keepalive_interval_secs, http_config.sse_keepalive_enabled
    );
    info!("    TCP keepalive: {}s", http_config.tcp_keepalive_secs);

    info!("");
    info!("✅ LunaRoute gateway listening on http://{}", addr);
    info!("   API endpoints:");
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
        ApiDialect::Both => {
            info!("   - OpenAI API:      http://{}/v1/chat/completions", addr);
            info!("   - Anthropic API:   http://{}/v1/messages", addr);
            info!(
                "   💡 For Claude Code: export ANTHROPIC_BASE_URL=http://{}",
                addr
            );
            info!(
                "   💡 For Codex:       export OPENAI_BASE_URL=http://{}",
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

    // Flush session events before exit to ensure all pending events are written
    if let Some(ref store) = session_store_for_passthrough {
        info!("Flushing pending session events...");
        if let Err(e) = store.flush().await {
            warn!("Failed to flush session events during shutdown: {}", e);
        } else {
            info!("✓ Session events flushed successfully");
        }
    }

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
