//! PII redaction

use crate::detector::{Detection, PIIType};
use serde::{Deserialize, Serialize};

/// Redaction mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactionMode {
    /// Remove the PII completely
    Remove,

    /// Replace with asterisks
    Mask,

    /// Replace with a token (reversible)
    Tokenize,

    /// Show partial data (e.g., last 4 digits)
    Partial,
}

/// Trait for redacting PII from text
pub trait PIIRedactor: Send + Sync {
    /// Redact PII from text based on detections
    fn redact(&self, text: &str, detections: &[Detection]) -> String;

    /// Get the redaction mode
    fn mode(&self) -> RedactionMode;
}

/// Configuration for PII redaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedactorConfig {
    /// Redaction mode to use
    pub mode: RedactionMode,

    /// Number of characters to show in partial mode
    pub partial_show_chars: usize,

    /// HMAC secret for tokenization (if using tokenize mode)
    pub hmac_secret: Option<String>,

    /// Per-type redaction overrides
    pub type_overrides: Vec<TypeRedactionOverride>,
}

impl Default for RedactorConfig {
    fn default() -> Self {
        Self {
            mode: RedactionMode::Mask,
            partial_show_chars: 4,
            hmac_secret: None,
            type_overrides: Vec::new(),
        }
    }
}

/// Override redaction behavior for a specific PII type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeRedactionOverride {
    /// PII type to override
    pub pii_type: PIIType,

    /// Redaction mode for this type
    pub mode: RedactionMode,

    /// Replacement text (for mask mode)
    pub replacement: Option<String>,
}

#[cfg(test)]
mod tests;
