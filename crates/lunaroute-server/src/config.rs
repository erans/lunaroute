use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ApiDialect {
    OpenAI,
    #[default]
    Anthropic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,

    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default)]
    pub api_dialect: ApiDialect,

    #[serde(default)]
    pub providers: ProvidersConfig,

    #[serde(default)]
    pub session_recording: lunaroute_session::SessionRecordingConfig,

    #[serde(default)]
    pub logging: LoggingConfig,

    #[serde(default)]
    pub routing: RoutingConfig,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_stats_max_sessions: Option<usize>,

    /// UI/Dashboard server configuration
    #[serde(default)]
    pub ui: lunaroute_ui::UiConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProvidersConfig {
    pub openai: Option<ProviderSettings>,
    pub anthropic: Option<ProviderSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    #[serde(default = "default_true")]
    pub enabled: bool,

    /// HTTP client configuration for connection pooling
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_client: Option<HttpClientSettings>,

    /// Custom headers to inject into requests sent to this provider
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_headers: Option<HeadersConfig>,

    /// Request body modifications (defaults, overrides, prepend)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_body: Option<RequestBodyModConfig>,

    /// Response body modifications (metadata injection, extension fields)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_body: Option<ResponseBodyModConfig>,
}

/// HTTP client configuration settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpClientSettings {
    /// Request timeout in seconds (default: 600 for OpenAI, 600 for Anthropic)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,

    /// Connection timeout in seconds (default: 10)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_timeout_secs: Option<u64>,

    /// Maximum number of idle connections per host (default: 32)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool_max_idle_per_host: Option<usize>,

    /// Pool idle timeout in seconds (default: 90)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool_idle_timeout_secs: Option<u64>,

    /// TCP keepalive interval in seconds (default: 60)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tcp_keepalive_secs: Option<u64>,

    /// Maximum number of retries for transient errors (default: 3)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,

    /// Enable connection pool metrics (default: true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_pool_metrics: Option<bool>,
}

impl HttpClientSettings {
    /// Convert to egress crate's HttpClientConfig, using defaults for unspecified fields
    pub fn to_http_client_config(&self) -> lunaroute_egress::HttpClientConfig {
        let defaults = lunaroute_egress::HttpClientConfig::default();

        lunaroute_egress::HttpClientConfig {
            timeout_secs: self.timeout_secs.unwrap_or(defaults.timeout_secs),
            connect_timeout_secs: self
                .connect_timeout_secs
                .unwrap_or(defaults.connect_timeout_secs),
            pool_max_idle_per_host: self
                .pool_max_idle_per_host
                .unwrap_or(defaults.pool_max_idle_per_host),
            pool_idle_timeout_secs: self
                .pool_idle_timeout_secs
                .unwrap_or(defaults.pool_idle_timeout_secs),
            tcp_keepalive_secs: self
                .tcp_keepalive_secs
                .unwrap_or(defaults.tcp_keepalive_secs),
            max_retries: self.max_retries.unwrap_or(defaults.max_retries),
            enable_pool_metrics: self
                .enable_pool_metrics
                .unwrap_or(defaults.enable_pool_metrics),
            user_agent: defaults.user_agent,
        }
    }
}

/// Configuration for custom HTTP headers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeadersConfig {
    /// Map of header name to header value (supports ${variable} template syntax)
    #[serde(flatten)]
    pub headers: HashMap<String, String>,
}

/// Configuration for request body modifications
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestBodyModConfig {
    /// Fields to set only if missing from client request (deep merge)
    /// Example: {"temperature": 0.7, "max_tokens": 1000}
    #[serde(skip_serializing_if = "Option::is_none")]
    pub defaults: Option<serde_json::Value>,

    /// Fields to always override in request body (deep merge, takes precedence)
    /// Example: {"temperature": 0.5} - will always replace client's temperature
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overrides: Option<serde_json::Value>,

    /// Messages to prepend to the messages array (useful for system messages)
    /// Example: [{"role": "system", "content": "You are a helpful assistant."}]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prepend_messages: Option<Vec<serde_json::Value>>,
}

/// Configuration for response body modifications
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseBodyModConfig {
    /// Whether to enable response body modifications
    #[serde(default)]
    pub enabled: bool,

    /// Namespace for metadata object (default: "lunaroute")
    /// Creates: {"choices": [...], "lunaroute": {"request_id": "...", ...}}
    #[serde(default = "default_metadata_namespace")]
    pub metadata_namespace: String,

    /// Fields to include in metadata (supports ${variable} template syntax)
    /// Example: {"request_id": "${request_id}", "provider": "${provider}"}
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<HashMap<String, String>>,

    /// Alternative: extension fields at top level (experimental)
    /// Creates: {"choices": [...], "x-request-id": "...", "x-provider": "..."}
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extension_fields: Option<HashMap<String, String>>,
}

// SessionRecordingConfig is now imported from lunaroute_session crate

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,

    #[serde(default = "default_false")]
    pub log_requests: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoutingConfig {
    #[serde(default)]
    pub rules: Vec<RoutingRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRule {
    pub name: String,
    pub model_pattern: String,
    pub primary: String,
    #[serde(default)]
    pub fallbacks: Vec<String>,
    #[serde(default = "default_priority")]
    pub priority: u32,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            api_dialect: ApiDialect::default(),
            providers: ProvidersConfig::default(),
            session_recording: lunaroute_session::SessionRecordingConfig::default(),
            logging: LoggingConfig::default(),
            routing: RoutingConfig::default(),
            session_stats_max_sessions: Some(100),
            ui: lunaroute_ui::UiConfig::default(),
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            log_requests: false,
        }
    }
}

/// Helper function to merge HTTP client environment variables into provider settings
fn merge_http_client_env(provider: &mut ProviderSettings, prefix: &str) {
    // Ensure http_client exists
    if provider.http_client.is_none() {
        provider.http_client = Some(HttpClientSettings {
            timeout_secs: None,
            connect_timeout_secs: None,
            pool_max_idle_per_host: None,
            pool_idle_timeout_secs: None,
            tcp_keepalive_secs: None,
            max_retries: None,
            enable_pool_metrics: None,
        });
    }

    if let Some(http_client) = &mut provider.http_client {
        // LUNAROUTE_<PREFIX>_TIMEOUT_SECS
        if let Ok(val) = std::env::var(format!("LUNAROUTE_{}_TIMEOUT_SECS", prefix)) {
            if let Ok(timeout) = val.parse::<u64>() {
                http_client.timeout_secs = Some(timeout);
            }
        }

        // LUNAROUTE_<PREFIX>_CONNECT_TIMEOUT_SECS
        if let Ok(val) = std::env::var(format!("LUNAROUTE_{}_CONNECT_TIMEOUT_SECS", prefix)) {
            if let Ok(timeout) = val.parse::<u64>() {
                http_client.connect_timeout_secs = Some(timeout);
            }
        }

        // LUNAROUTE_<PREFIX>_POOL_MAX_IDLE
        if let Ok(val) = std::env::var(format!("LUNAROUTE_{}_POOL_MAX_IDLE", prefix)) {
            if let Ok(max_idle) = val.parse::<usize>() {
                http_client.pool_max_idle_per_host = Some(max_idle);
            }
        }

        // LUNAROUTE_<PREFIX>_POOL_IDLE_TIMEOUT_SECS
        if let Ok(val) = std::env::var(format!("LUNAROUTE_{}_POOL_IDLE_TIMEOUT_SECS", prefix)) {
            if let Ok(timeout) = val.parse::<u64>() {
                http_client.pool_idle_timeout_secs = Some(timeout);
            }
        }

        // LUNAROUTE_<PREFIX>_TCP_KEEPALIVE_SECS
        if let Ok(val) = std::env::var(format!("LUNAROUTE_{}_TCP_KEEPALIVE_SECS", prefix)) {
            if let Ok(keepalive) = val.parse::<u64>() {
                http_client.tcp_keepalive_secs = Some(keepalive);
            }
        }

        // LUNAROUTE_<PREFIX>_MAX_RETRIES
        if let Ok(val) = std::env::var(format!("LUNAROUTE_{}_MAX_RETRIES", prefix)) {
            if let Ok(retries) = val.parse::<u32>() {
                http_client.max_retries = Some(retries);
            }
        }

        // LUNAROUTE_<PREFIX>_ENABLE_POOL_METRICS
        if let Ok(val) = std::env::var(format!("LUNAROUTE_{}_ENABLE_POOL_METRICS", prefix)) {
            if let Ok(enabled) = val.parse::<bool>() {
                http_client.enable_pool_metrics = Some(enabled);
            }
        }
    }
}

impl ServerConfig {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, Box<dyn std::error::Error>> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path)?;

        let mut config: ServerConfig = if path.extension().and_then(|s| s.to_str()) == Some("toml")
        {
            toml::from_str(&contents)?
        } else {
            // Default to YAML
            serde_yaml::from_str(&contents)?
        };

        // Expand tilde (~) in session recording paths
        config.session_recording.expand_paths();

        Ok(config)
    }

    /// Merge environment variables into config (env vars take precedence)
    pub fn merge_env(&mut self) {
        // API dialect
        if let Ok(val) = std::env::var("LUNAROUTE_DIALECT") {
            match val.to_lowercase().as_str() {
                "openai" => self.api_dialect = ApiDialect::OpenAI,
                "anthropic" => self.api_dialect = ApiDialect::Anthropic,
                _ => eprintln!(
                    "Warning: Invalid LUNAROUTE_DIALECT '{}', using default",
                    val
                ),
            }
        }

        // Provider API keys (no LUNAROUTE_ prefix for these)
        if let Ok(api_key) = std::env::var("OPENAI_API_KEY") {
            let provider = self.providers.openai.get_or_insert(ProviderSettings {
                api_key: None,
                base_url: None,
                enabled: true,
                http_client: None,
                request_headers: None,
                request_body: None,
                response_body: None,
            });
            provider.api_key = Some(api_key);
        }

        if let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") {
            let provider = self.providers.anthropic.get_or_insert(ProviderSettings {
                api_key: None,
                base_url: None,
                enabled: true,
                http_client: None,
                request_headers: None,
                request_body: None,
                response_body: None,
            });
            provider.api_key = Some(api_key);
        }

        // OpenAI HTTP client pool settings
        if let Some(openai) = &mut self.providers.openai {
            merge_http_client_env(openai, "OPENAI");
        }

        // Anthropic HTTP client pool settings
        if let Some(anthropic) = &mut self.providers.anthropic {
            merge_http_client_env(anthropic, "ANTHROPIC");
        }

        // Session recording settings
        if let Ok(val) = std::env::var("LUNAROUTE_ENABLE_SESSION_RECORDING")
            && let Ok(enabled) = val.parse::<bool>()
        {
            self.session_recording.enabled = enabled;
        }

        // JSONL writer settings
        if let Ok(val) = std::env::var("LUNAROUTE_SESSIONS_DIR") {
            if self.session_recording.jsonl.is_none() {
                self.session_recording.jsonl = Some(lunaroute_session::JsonlConfig::default());
            }
            if let Some(jsonl) = &mut self.session_recording.jsonl {
                jsonl.directory = PathBuf::from(val);
            }
        }

        if let Ok(val) = std::env::var("LUNAROUTE_ENABLE_JSONL_WRITER")
            && let Ok(enabled) = val.parse::<bool>()
        {
            if self.session_recording.jsonl.is_none() {
                self.session_recording.jsonl = Some(lunaroute_session::JsonlConfig::default());
            }
            if let Some(jsonl) = &mut self.session_recording.jsonl {
                jsonl.enabled = enabled;
            }
        }

        // SQLite writer settings
        if let Ok(val) = std::env::var("LUNAROUTE_SESSIONS_DB_PATH") {
            if self.session_recording.sqlite.is_none() {
                self.session_recording.sqlite = Some(lunaroute_session::SqliteConfig::default());
            }
            if let Some(sqlite) = &mut self.session_recording.sqlite {
                sqlite.path = PathBuf::from(val);
            }
        }

        if let Ok(val) = std::env::var("LUNAROUTE_ENABLE_SQLITE_WRITER")
            && let Ok(enabled) = val.parse::<bool>()
        {
            if self.session_recording.sqlite.is_none() {
                self.session_recording.sqlite = Some(lunaroute_session::SqliteConfig::default());
            }
            if let Some(sqlite) = &mut self.session_recording.sqlite {
                sqlite.enabled = enabled;
            }
        }

        // Expand tilde (~) in session recording paths after env var processing
        self.session_recording.expand_paths();

        // Logging settings
        if let Ok(val) = std::env::var("LUNAROUTE_LOG_REQUESTS")
            && let Ok(enabled) = val.parse::<bool>()
        {
            self.logging.log_requests = enabled;
        }

        if let Ok(val) = std::env::var("LUNAROUTE_LOG_LEVEL") {
            self.logging.level = val;
        }

        // Server settings
        if let Ok(val) = std::env::var("LUNAROUTE_PORT")
            && let Ok(port) = val.parse::<u16>()
        {
            self.port = port;
        }

        if let Ok(val) = std::env::var("LUNAROUTE_HOST") {
            self.host = val;
        }
    }
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    8081
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_priority() -> u32 {
    10
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn default_metadata_namespace() -> String {
    "lunaroute".to_string()
}
