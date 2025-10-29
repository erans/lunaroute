//! Error types for LunaRoute Core

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Rate limit exceeded{}", retry_after_secs.map(|s| format!(": retry after {}s", s)).unwrap_or_default())]
    RateLimitExceeded { retry_after_secs: Option<u64> },

    #[error("Internal error: {0}")]
    Internal(String),

    // Multi-tenancy errors
    #[error("Invalid tenant: {0}")]
    InvalidTenant(String),

    #[error("Tenant required: {0}")]
    TenantRequired(String),

    #[error("Tenant not found: {0}")]
    TenantNotFound(String),

    // Configuration errors
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Configuration not found")]
    ConfigNotFound,

    #[error("Configuration validation failed: {0}")]
    ConfigValidation(String),

    // Session store errors
    #[error("Session not found: {0}")]
    SessionNotFound(String),

    #[error("Session store error: {0}")]
    SessionStore(String),

    // Database errors
    #[error("Database error: {0}")]
    Database(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
