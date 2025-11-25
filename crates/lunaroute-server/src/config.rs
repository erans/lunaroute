use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ApiDialect {
    OpenAI,
    Anthropic,
    /// Accept both OpenAI and Anthropic API formats simultaneously
    /// - OpenAI format at /v1/chat/completions
    /// - Anthropic format at /v1/messages
    /// - Routes to appropriate provider based on model prefix
    #[default]
    Both,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,

    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default)]
    pub api_dialect: ApiDialect,

    /// HTTP server settings (TCP/SSE configuration)
    #[serde(default)]
    pub http_server: HttpServerSettings,

    #[serde(default)]
    pub providers: ProvidersConfig,

    #[serde(default)]
    pub session_recording: lunaroute_session::SessionRecordingConfig,

    #[serde(default)]
    pub logging: LoggingConfig,

    #[serde(default)]
    pub routing: RoutingConfig,

    /// Bypass configuration for unknown paths
    #[serde(default)]
    pub bypass: BypassConfig,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_stats_max_sessions: Option<usize>,

    /// UI/Dashboard server configuration
    #[serde(default)]
    pub ui: lunaroute_ui::UiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvidersConfig {
    pub openai: Option<ProviderSettings>,
    pub anthropic: Option<ProviderSettings>,
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        Self {
            // Enable both providers in passthrough mode by default
            // No api_key = passthrough (will use env vars or client headers)
            openai: Some(ProviderSettings {
                api_key: None,
                base_url: None,
                enabled: true,
                http_client: None,
                request_headers: None,
                request_body: None,
                response_body: None,
                codex_auth: None,
            }),
            anthropic: Some(ProviderSettings {
                api_key: None,
                base_url: None,
                enabled: true,
                http_client: None,
                request_headers: None,
                request_body: None,
                response_body: None,
                codex_auth: None,
            }),
        }
    }
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

    /// Codex authentication (read auth tokens from Codex CLI auth.json file)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_auth: Option<CodexAuthConfig>,
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

    /// Pool idle timeout in seconds (default: 50)
    /// Must be LOWER than upstream server timeout to prevent stale connections
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

/// HTTP server configuration settings (server-side HTTP/TCP behavior)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpServerSettings {
    /// Enable TCP_NODELAY (disable Nagle's algorithm)
    /// Set to true for low-latency streaming (SSE), false for bulk transfers
    /// Default: true
    #[serde(default = "default_tcp_nodelay")]
    pub tcp_nodelay: bool,

    /// TCP keepalive interval in seconds
    /// Sends keepalive packets to detect dead connections
    /// Default: 60
    #[serde(default = "default_tcp_keepalive_secs")]
    pub tcp_keepalive_secs: u64,

    /// SSE keepalive interval in seconds
    /// Sends comment lines to keep streaming connections alive
    /// Default: 15
    #[serde(default = "default_sse_keepalive_interval_secs")]
    pub sse_keepalive_interval_secs: u64,

    /// Enable SSE keepalive comments
    /// Default: true
    #[serde(default = "default_true")]
    pub sse_keepalive_enabled: bool,

    /// TCP send buffer size in bytes (null = OS default)
    /// Default: null
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_buffer_size: Option<usize>,

    /// TCP receive buffer size in bytes (null = OS default)
    /// Default: null
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recv_buffer_size: Option<usize>,
}

impl Default for HttpServerSettings {
    fn default() -> Self {
        Self {
            tcp_nodelay: default_tcp_nodelay(),
            tcp_keepalive_secs: default_tcp_keepalive_secs(),
            sse_keepalive_interval_secs: default_sse_keepalive_interval_secs(),
            sse_keepalive_enabled: true,
            send_buffer_size: None,
            recv_buffer_size: None,
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

/// Codex authentication configuration (OpenAI only)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexAuthConfig {
    /// Enable Codex authentication
    #[serde(default)]
    pub enabled: bool,

    /// Path to Codex auth file (default: ~/.codex/auth.json)
    #[serde(default = "default_codex_auth_file")]
    pub auth_file: PathBuf,

    /// JSON field path for access token (default: "tokens.access_token")
    /// Supports nested paths using dot notation
    #[serde(default = "default_codex_token_field")]
    pub token_field: String,

    /// Optional account ID to send as chatgpt-account-id header
    /// If set, will override client's chatgpt-account-id header
    /// If not set, will try to read from auth.json or pass through client header
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

// SessionRecordingConfig is now imported from lunaroute_session crate

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,

    #[serde(default = "default_false")]
    pub log_requests: bool,

    /// Enable SQL query logging (default: false to reduce noise)
    #[serde(default = "default_false")]
    pub log_sql_queries: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BypassConfig {
    /// Enable automatic bypass for unknown paths (default: true)
    /// When enabled, paths not in the intercepted list are proxied directly
    /// to the provider without routing engine overhead
    #[serde(default = "default_bypass_enabled")]
    pub enabled: bool,

    /// Provider to use for bypassed requests (default: None = first available)
    /// If not specified, uses the first available provider
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

impl Default for BypassConfig {
    fn default() -> Self {
        Self {
            enabled: default_bypass_enabled(),
            provider: None,
        }
    }
}

fn default_bypass_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoutingConfig {
    #[serde(default)]
    pub rules: Vec<RoutingRule>,

    /// Provider switch notification configuration
    #[serde(default)]
    pub provider_switch_notification: Option<lunaroute_routing::ProviderSwitchNotificationConfig>,
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
            http_server: HttpServerSettings::default(),
            providers: ProvidersConfig::default(),
            session_recording: lunaroute_session::SessionRecordingConfig::default(),
            logging: LoggingConfig::default(),
            routing: RoutingConfig::default(),
            bypass: BypassConfig::default(),
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
            log_sql_queries: false,
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
        if let Ok(val) = std::env::var(format!("LUNAROUTE_{}_TIMEOUT_SECS", prefix))
            && let Ok(timeout) = val.parse::<u64>()
        {
            http_client.timeout_secs = Some(timeout);
        }

        // LUNAROUTE_<PREFIX>_CONNECT_TIMEOUT_SECS
        if let Ok(val) = std::env::var(format!("LUNAROUTE_{}_CONNECT_TIMEOUT_SECS", prefix))
            && let Ok(timeout) = val.parse::<u64>()
        {
            http_client.connect_timeout_secs = Some(timeout);
        }

        // LUNAROUTE_<PREFIX>_POOL_MAX_IDLE
        if let Ok(val) = std::env::var(format!("LUNAROUTE_{}_POOL_MAX_IDLE", prefix))
            && let Ok(max_idle) = val.parse::<usize>()
        {
            http_client.pool_max_idle_per_host = Some(max_idle);
        }

        // LUNAROUTE_<PREFIX>_POOL_IDLE_TIMEOUT_SECS
        if let Ok(val) = std::env::var(format!("LUNAROUTE_{}_POOL_IDLE_TIMEOUT_SECS", prefix))
            && let Ok(timeout) = val.parse::<u64>()
        {
            http_client.pool_idle_timeout_secs = Some(timeout);
        }

        // LUNAROUTE_<PREFIX>_TCP_KEEPALIVE_SECS
        if let Ok(val) = std::env::var(format!("LUNAROUTE_{}_TCP_KEEPALIVE_SECS", prefix))
            && let Ok(keepalive) = val.parse::<u64>()
        {
            http_client.tcp_keepalive_secs = Some(keepalive);
        }

        // LUNAROUTE_<PREFIX>_MAX_RETRIES
        if let Ok(val) = std::env::var(format!("LUNAROUTE_{}_MAX_RETRIES", prefix))
            && let Ok(retries) = val.parse::<u32>()
        {
            http_client.max_retries = Some(retries);
        }

        // LUNAROUTE_<PREFIX>_ENABLE_POOL_METRICS
        if let Some(enabled) = parse_bool_env(&format!("LUNAROUTE_{}_ENABLE_POOL_METRICS", prefix))
        {
            http_client.enable_pool_metrics = Some(enabled);
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
                "both" => self.api_dialect = ApiDialect::Both,
                _ => eprintln!(
                    "Warning: Invalid LUNAROUTE_DIALECT '{}', using default (valid: openai, anthropic, both)",
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
                codex_auth: None,
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
                codex_auth: None,
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
        if let Some(enabled) = parse_bool_env("LUNAROUTE_ENABLE_SESSION_RECORDING") {
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

        if let Some(enabled) = parse_bool_env("LUNAROUTE_ENABLE_JSONL_WRITER") {
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

        if let Some(enabled) = parse_bool_env("LUNAROUTE_ENABLE_SQLITE_WRITER") {
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
        if let Some(enabled) = parse_bool_env("LUNAROUTE_LOG_REQUESTS") {
            self.logging.log_requests = enabled;
        }

        if let Some(enabled) = parse_bool_env("LUNAROUTE_LOG_SQL_QUERIES") {
            self.logging.log_sql_queries = enabled;
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

        // UI server settings
        if let Some(enabled) = parse_bool_env("LUNAROUTE_UI_ENABLED") {
            self.ui.enabled = enabled;
        }

        if let Ok(val) = std::env::var("LUNAROUTE_UI_HOST") {
            self.ui.host = val;
        }

        if let Ok(val) = std::env::var("LUNAROUTE_UI_PORT")
            && let Ok(port) = val.parse::<u16>()
        {
            self.ui.port = port;
        }

        if let Some(enabled) = parse_bool_env("LUNAROUTE_UI_LOG_REQUESTS") {
            self.ui.log_requests = enabled;
        }

        // Bypass settings
        if let Some(enabled) = parse_bool_env("LUNAROUTE_BYPASS_ENABLED") {
            self.bypass.enabled = enabled;
        }

        if let Ok(val) = std::env::var("LUNAROUTE_BYPASS_PROVIDER") {
            self.bypass.provider = Some(val);
        }
    }
}

/// Parse boolean from environment variable with flexible string matching
/// Accepts: true/false, 1/0, yes/no, on/off (case-insensitive)
fn parse_bool_env(var_name: &str) -> Option<bool> {
    std::env::var(var_name)
        .ok()
        .and_then(|val| match val.to_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Some(true),
            "false" | "0" | "no" | "off" => Some(false),
            _ => {
                eprintln!(
                    "Warning: Invalid boolean value '{}' for {}, ignoring",
                    val, var_name
                );
                None
            }
        })
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

fn default_codex_auth_file() -> PathBuf {
    PathBuf::from("~/.codex/auth.json")
}

fn default_codex_token_field() -> String {
    "tokens.access_token".to_string()
}

fn default_tcp_nodelay() -> bool {
    true
}

fn default_tcp_keepalive_secs() -> u64 {
    60
}

fn default_sse_keepalive_interval_secs() -> u64 {
    15
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::env;

    #[test]
    fn test_http_client_settings_to_config_with_defaults() {
        let settings = HttpClientSettings {
            timeout_secs: None,
            connect_timeout_secs: None,
            pool_max_idle_per_host: None,
            pool_idle_timeout_secs: None,
            tcp_keepalive_secs: None,
            max_retries: None,
            enable_pool_metrics: None,
        };

        let config = settings.to_http_client_config();

        // Should match egress crate defaults
        assert_eq!(config.timeout_secs, 600);
        assert_eq!(config.connect_timeout_secs, 10);
        assert_eq!(config.pool_max_idle_per_host, 32);
        assert_eq!(config.pool_idle_timeout_secs, 600); // Updated from 50s to 600s in v0.1.4
        assert_eq!(config.tcp_keepalive_secs, 60);
        assert_eq!(config.max_retries, 3);
        assert!(config.enable_pool_metrics);
    }

    #[test]
    fn test_http_client_settings_to_config_with_custom_values() {
        let settings = HttpClientSettings {
            timeout_secs: Some(300),
            connect_timeout_secs: Some(5),
            pool_max_idle_per_host: Some(64),
            pool_idle_timeout_secs: Some(120),
            tcp_keepalive_secs: Some(30),
            max_retries: Some(5),
            enable_pool_metrics: Some(false),
        };

        let config = settings.to_http_client_config();

        assert_eq!(config.timeout_secs, 300);
        assert_eq!(config.connect_timeout_secs, 5);
        assert_eq!(config.pool_max_idle_per_host, 64);
        assert_eq!(config.pool_idle_timeout_secs, 120);
        assert_eq!(config.tcp_keepalive_secs, 30);
        assert_eq!(config.max_retries, 5);
        assert!(!config.enable_pool_metrics);
    }

    #[test]
    fn test_http_client_settings_partial_override() {
        let settings = HttpClientSettings {
            timeout_secs: Some(300),
            connect_timeout_secs: None, // Use default
            pool_max_idle_per_host: Some(64),
            pool_idle_timeout_secs: None, // Use default
            tcp_keepalive_secs: None,     // Use default
            max_retries: None,            // Use default
            enable_pool_metrics: None,    // Use default
        };

        let config = settings.to_http_client_config();

        assert_eq!(config.timeout_secs, 300); // Custom
        assert_eq!(config.connect_timeout_secs, 10); // Default
        assert_eq!(config.pool_max_idle_per_host, 64); // Custom
        assert_eq!(config.pool_idle_timeout_secs, 600); // Default (updated to 600s in v0.1.4)
    }

    #[test]
    #[serial]
    fn test_merge_http_client_env_all_variables() {
        // Setup environment variables
        unsafe {
            env::set_var("LUNAROUTE_OPENAI_TIMEOUT_SECS", "300");
            env::set_var("LUNAROUTE_OPENAI_CONNECT_TIMEOUT_SECS", "5");
            env::set_var("LUNAROUTE_OPENAI_POOL_MAX_IDLE", "64");
            env::set_var("LUNAROUTE_OPENAI_POOL_IDLE_TIMEOUT_SECS", "120");
            env::set_var("LUNAROUTE_OPENAI_TCP_KEEPALIVE_SECS", "30");
            env::set_var("LUNAROUTE_OPENAI_MAX_RETRIES", "5");
            env::set_var("LUNAROUTE_OPENAI_ENABLE_POOL_METRICS", "false");
        }

        let mut provider = ProviderSettings {
            api_key: Some("test-key".to_string()),
            base_url: None,
            enabled: true,
            http_client: None,
            request_headers: None,
            request_body: None,
            response_body: None,
            codex_auth: None,
        };

        merge_http_client_env(&mut provider, "OPENAI");

        let http_client = provider.http_client.unwrap();
        assert_eq!(http_client.timeout_secs, Some(300));
        assert_eq!(http_client.connect_timeout_secs, Some(5));
        assert_eq!(http_client.pool_max_idle_per_host, Some(64));
        assert_eq!(http_client.pool_idle_timeout_secs, Some(120));
        assert_eq!(http_client.tcp_keepalive_secs, Some(30));
        assert_eq!(http_client.max_retries, Some(5));
        assert_eq!(http_client.enable_pool_metrics, Some(false));

        // Cleanup
        unsafe {
            env::remove_var("LUNAROUTE_OPENAI_TIMEOUT_SECS");
            env::remove_var("LUNAROUTE_OPENAI_CONNECT_TIMEOUT_SECS");
            env::remove_var("LUNAROUTE_OPENAI_POOL_MAX_IDLE");
            env::remove_var("LUNAROUTE_OPENAI_POOL_IDLE_TIMEOUT_SECS");
            env::remove_var("LUNAROUTE_OPENAI_TCP_KEEPALIVE_SECS");
            env::remove_var("LUNAROUTE_OPENAI_MAX_RETRIES");
            env::remove_var("LUNAROUTE_OPENAI_ENABLE_POOL_METRICS");
        }
    }

    #[test]
    #[serial]
    fn test_merge_http_client_env_invalid_values_ignored() {
        // Setup environment variables with invalid values
        unsafe {
            env::set_var("LUNAROUTE_ANTHROPIC_TIMEOUT_SECS", "not-a-number");
            env::set_var("LUNAROUTE_ANTHROPIC_POOL_MAX_IDLE", "invalid");
            env::set_var("LUNAROUTE_ANTHROPIC_ENABLE_POOL_METRICS", "maybe");
        }

        let mut provider = ProviderSettings {
            api_key: Some("test-key".to_string()),
            base_url: None,
            enabled: true,
            http_client: None,
            request_headers: None,
            request_body: None,
            response_body: None,
            codex_auth: None,
        };

        merge_http_client_env(&mut provider, "ANTHROPIC");

        // http_client should be created but fields should be None (invalid values ignored)
        let http_client = provider.http_client.unwrap();
        assert_eq!(http_client.timeout_secs, None);
        assert_eq!(http_client.pool_max_idle_per_host, None);
        assert_eq!(http_client.enable_pool_metrics, None);

        // Cleanup
        unsafe {
            env::remove_var("LUNAROUTE_ANTHROPIC_TIMEOUT_SECS");
            env::remove_var("LUNAROUTE_ANTHROPIC_POOL_MAX_IDLE");
            env::remove_var("LUNAROUTE_ANTHROPIC_ENABLE_POOL_METRICS");
        }
    }

    #[test]
    #[serial]
    fn test_merge_http_client_env_partial_variables() {
        // Setup only some environment variables
        unsafe {
            env::set_var("LUNAROUTE_OPENAI_TIMEOUT_SECS", "300");
            env::set_var("LUNAROUTE_OPENAI_POOL_MAX_IDLE", "64");
        }
        // Other variables not set

        let mut provider = ProviderSettings {
            api_key: Some("test-key".to_string()),
            base_url: None,
            enabled: true,
            http_client: None,
            request_headers: None,
            request_body: None,
            response_body: None,
            codex_auth: None,
        };

        merge_http_client_env(&mut provider, "OPENAI");

        let http_client = provider.http_client.unwrap();
        assert_eq!(http_client.timeout_secs, Some(300));
        assert_eq!(http_client.pool_max_idle_per_host, Some(64));
        assert_eq!(http_client.connect_timeout_secs, None); // Not set
        assert_eq!(http_client.tcp_keepalive_secs, None); // Not set

        // Cleanup
        unsafe {
            env::remove_var("LUNAROUTE_OPENAI_TIMEOUT_SECS");
            env::remove_var("LUNAROUTE_OPENAI_POOL_MAX_IDLE");
        }
    }

    #[test]
    fn test_yaml_deserialization_with_http_client() {
        let yaml = r#"
host: "0.0.0.0"
port: 8081
providers:
  openai:
    enabled: true
    api_key: "test-key"
    http_client:
      timeout_secs: 300
      pool_max_idle_per_host: 64
      pool_idle_timeout_secs: 120
"#;

        let config: ServerConfig = serde_yaml::from_str(yaml).unwrap();

        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 8081);

        let openai = config.providers.openai.as_ref().unwrap();
        assert!(openai.enabled);
        assert_eq!(openai.api_key, Some("test-key".to_string()));

        let http_client = openai.http_client.as_ref().unwrap();
        assert_eq!(http_client.timeout_secs, Some(300));
        assert_eq!(http_client.pool_max_idle_per_host, Some(64));
        assert_eq!(http_client.pool_idle_timeout_secs, Some(120));
        assert_eq!(http_client.tcp_keepalive_secs, None); // Not specified
    }

    #[test]
    fn test_yaml_deserialization_without_http_client() {
        let yaml = r#"
host: "127.0.0.1"
port: 8081
providers:
  openai:
    enabled: true
    api_key: "test-key"
"#;

        let config: ServerConfig = serde_yaml::from_str(yaml).unwrap();

        let openai = config.providers.openai.as_ref().unwrap();
        assert!(openai.http_client.is_none()); // http_client section not provided
    }

    #[test]
    #[serial]
    fn test_server_config_merge_env_http_client() {
        // Setup environment
        unsafe {
            env::set_var("OPENAI_API_KEY", "test-api-key");
            env::set_var("LUNAROUTE_OPENAI_TIMEOUT_SECS", "300");
            env::set_var("LUNAROUTE_OPENAI_POOL_MAX_IDLE", "64");
        }

        let mut config = ServerConfig::default();
        config.merge_env();

        // Should have OpenAI provider configured
        let openai = config.providers.openai.as_ref().unwrap();
        assert_eq!(openai.api_key, Some("test-api-key".to_string()));

        // Should have http_client settings from env vars
        let http_client = openai.http_client.as_ref().unwrap();
        assert_eq!(http_client.timeout_secs, Some(300));
        assert_eq!(http_client.pool_max_idle_per_host, Some(64));

        // Cleanup
        unsafe {
            env::remove_var("OPENAI_API_KEY");
            env::remove_var("LUNAROUTE_OPENAI_TIMEOUT_SECS");
            env::remove_var("LUNAROUTE_OPENAI_POOL_MAX_IDLE");
        }
    }

    #[test]
    #[serial]
    fn test_configuration_precedence_yaml_then_env() {
        // Start with YAML config
        let yaml = r#"
providers:
  openai:
    enabled: true
    api_key: "yaml-key"
    http_client:
      timeout_secs: 200
      pool_max_idle_per_host: 32
"#;

        let mut config: ServerConfig = serde_yaml::from_str(yaml).unwrap();

        // Set env var that should override YAML
        unsafe {
            env::set_var("LUNAROUTE_OPENAI_TIMEOUT_SECS", "300");
            env::set_var("LUNAROUTE_OPENAI_POOL_MAX_IDLE", "64");
        }

        config.merge_env();

        let openai = config.providers.openai.as_ref().unwrap();
        let http_client = openai.http_client.as_ref().unwrap();

        // Env var should override YAML value
        assert_eq!(http_client.timeout_secs, Some(300));
        assert_eq!(http_client.pool_max_idle_per_host, Some(64));

        // Cleanup
        unsafe {
            env::remove_var("LUNAROUTE_OPENAI_TIMEOUT_SECS");
            env::remove_var("LUNAROUTE_OPENAI_POOL_MAX_IDLE");
        }
    }

    #[test]
    #[serial]
    fn test_merge_http_client_env_creates_http_client_if_missing() {
        let mut provider = ProviderSettings {
            api_key: Some("test-key".to_string()),
            base_url: None,
            enabled: true,
            http_client: None, // Start with None
            request_headers: None,
            request_body: None,
            response_body: None,
            codex_auth: None,
        };

        unsafe {
            env::set_var("LUNAROUTE_OPENAI_TIMEOUT_SECS", "300");
        }

        merge_http_client_env(&mut provider, "OPENAI");

        // Should create http_client if it didn't exist
        assert!(provider.http_client.is_some());
        let http_client = provider.http_client.unwrap();
        assert_eq!(http_client.timeout_secs, Some(300));

        // Cleanup
        unsafe {
            env::remove_var("LUNAROUTE_OPENAI_TIMEOUT_SECS");
        }
    }

    #[test]
    #[serial]
    fn test_merge_http_client_env_preserves_existing_values() {
        let mut provider = ProviderSettings {
            api_key: Some("test-key".to_string()),
            base_url: None,
            enabled: true,
            http_client: Some(HttpClientSettings {
                timeout_secs: Some(200),
                connect_timeout_secs: Some(5),
                pool_max_idle_per_host: None,
                pool_idle_timeout_secs: None,
                tcp_keepalive_secs: None,
                max_retries: None,
                enable_pool_metrics: None,
            }),
            request_headers: None,
            request_body: None,
            response_body: None,
            codex_auth: None,
        };

        // Set only one env var
        unsafe {
            env::set_var("LUNAROUTE_OPENAI_POOL_MAX_IDLE", "64");
        }

        merge_http_client_env(&mut provider, "OPENAI");

        let http_client = provider.http_client.unwrap();

        // Original values preserved
        assert_eq!(http_client.timeout_secs, Some(200));
        assert_eq!(http_client.connect_timeout_secs, Some(5));

        // New value added
        assert_eq!(http_client.pool_max_idle_per_host, Some(64));

        // Cleanup
        unsafe {
            env::remove_var("LUNAROUTE_OPENAI_POOL_MAX_IDLE");
        }
    }

    #[test]
    fn test_routing_config_with_notification() {
        let yaml = r#"
provider_switch_notification:
  enabled: true
  default_message: "Custom notification"
rules: []
"#;

        let config: RoutingConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.provider_switch_notification.is_some());
        let notif = config.provider_switch_notification.unwrap();
        assert!(notif.enabled);
        assert_eq!(notif.default_message, "Custom notification");
    }

    #[test]
    fn test_routing_config_without_notification() {
        let yaml = r#"
rules: []
"#;

        let config: RoutingConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.provider_switch_notification.is_none());
    }
}
