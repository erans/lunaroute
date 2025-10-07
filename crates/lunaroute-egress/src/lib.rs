//! LunaRoute Egress Connectors
//!
//! This crate provides connectors to downstream LLM providers:
//! - OpenAI connector
//! - Anthropic connector

use thiserror::Error;

pub mod anthropic;
pub mod client;
pub mod openai;

/// Egress-specific errors
#[derive(Debug, Error)]
pub enum EgressError {
    /// HTTP request failed
    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),

    /// Provider returned an error response
    #[error("Provider error: {status_code} - {message}")]
    ProviderError { status_code: u16, message: String },

    /// Failed to parse provider response
    #[error("Failed to parse response: {0}")]
    ParseError(String),

    /// Stream error
    #[error("Stream error: {0}")]
    StreamError(String),

    /// Timeout error
    #[error("Request timeout after {0}s")]
    Timeout(u64),

    /// Rate limit exceeded
    #[error("Rate limit exceeded{}", retry_after_secs.map(|s| format!(": retry after {}s", s)).unwrap_or_default())]
    RateLimitExceeded { retry_after_secs: Option<u64> },

    /// Invalid configuration
    #[error("Invalid configuration: {0}")]
    ConfigError(String),

    /// Serialization error
    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),
}

/// Egress result type
pub type Result<T> = std::result::Result<T, EgressError>;

impl From<EgressError> for lunaroute_core::Error {
    fn from(err: EgressError) -> Self {
        // Convert egress errors to core errors
        lunaroute_core::Error::Provider(err.to_string())
    }
}
