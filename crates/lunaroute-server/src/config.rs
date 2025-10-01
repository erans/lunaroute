use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiDialect {
    OpenAI,
    Anthropic,
}

impl Default for ApiDialect {
    fn default() -> Self {
        ApiDialect::Anthropic
    }
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
    pub session_recording: SessionRecordingConfig,

    #[serde(default)]
    pub logging: LoggingConfig,

    #[serde(default)]
    pub routing: RoutingConfig,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecordingConfig {
    #[serde(default = "default_false")]
    pub enabled: bool,

    #[serde(default = "default_sessions_dir")]
    pub directory: String,
}

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
            session_recording: SessionRecordingConfig::default(),
            logging: LoggingConfig::default(),
            routing: RoutingConfig::default(),
        }
    }
}

impl Default for SessionRecordingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            directory: default_sessions_dir(),
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

impl ServerConfig {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, Box<dyn std::error::Error>> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path)?;

        let config = if path.extension().and_then(|s| s.to_str()) == Some("toml") {
            toml::from_str(&contents)?
        } else {
            // Default to YAML
            serde_yaml::from_str(&contents)?
        };

        Ok(config)
    }

    /// Merge environment variables into config (env vars take precedence)
    pub fn merge_env(&mut self) {
        // API dialect
        if let Ok(val) = std::env::var("LUNAROUTE_DIALECT") {
            match val.to_lowercase().as_str() {
                "openai" => self.api_dialect = ApiDialect::OpenAI,
                "anthropic" => self.api_dialect = ApiDialect::Anthropic,
                _ => eprintln!("Warning: Invalid LUNAROUTE_DIALECT '{}', using default", val),
            }
        }

        // Provider API keys (no LUNAROUTE_ prefix for these)
        if let Ok(api_key) = std::env::var("OPENAI_API_KEY") {
            let provider = self.providers.openai.get_or_insert_with(|| ProviderSettings {
                api_key: None,
                base_url: None,
                enabled: true,
            });
            provider.api_key = Some(api_key);
        }

        if let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") {
            let provider = self.providers.anthropic.get_or_insert_with(|| ProviderSettings {
                api_key: None,
                base_url: None,
                enabled: true,
            });
            provider.api_key = Some(api_key);
        }

        // Session recording settings
        if let Ok(val) = std::env::var("LUNAROUTE_SESSIONS_DIR") {
            self.session_recording.directory = val;
        }

        if let Ok(val) = std::env::var("LUNAROUTE_ENABLE_SESSION_RECORDING") {
            if let Ok(enabled) = val.parse::<bool>() {
                self.session_recording.enabled = enabled;
            }
        }

        // Logging settings
        if let Ok(val) = std::env::var("LUNAROUTE_LOG_REQUESTS") {
            if let Ok(enabled) = val.parse::<bool>() {
                self.logging.log_requests = enabled;
            }
        }

        if let Ok(val) = std::env::var("LUNAROUTE_LOG_LEVEL") {
            self.logging.level = val;
        }

        // Server settings
        if let Ok(val) = std::env::var("LUNAROUTE_PORT") {
            if let Ok(port) = val.parse::<u16>() {
                self.port = port;
            }
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
    3000
}

fn default_sessions_dir() -> String {
    "./sessions".to_string()
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
