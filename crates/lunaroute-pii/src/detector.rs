//! PII detectors

mod regex_detector;

pub use regex_detector::RegexPIIDetector;

use serde::{Deserialize, Serialize};

/// PII detection result
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Detection {
    /// Type of PII detected
    pub pii_type: PIIType,

    /// Start position in the text
    pub start: usize,

    /// End position in the text
    pub end: usize,

    /// The detected text
    pub text: String,

    /// Confidence score (0.0 to 1.0)
    pub confidence: f32,
}

/// Types of PII that can be detected
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PIIType {
    /// Email address
    Email,

    /// Phone number
    Phone,

    /// Social Security Number
    SSN,

    /// Credit card number
    CreditCard,

    /// IP address
    IPAddress,

    /// Custom pattern
    Custom,
}

/// Trait for detecting PII in text
pub trait PIIDetector: Send + Sync {
    /// Detect PII in the given text
    fn detect(&self, text: &str) -> Vec<Detection>;

    /// Get the types of PII this detector can find
    fn supported_types(&self) -> Vec<PIIType>;
}

/// Configuration for a PII detector
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorConfig {
    /// Enable email detection
    pub detect_email: bool,

    /// Enable phone number detection
    pub detect_phone: bool,

    /// Enable SSN detection
    pub detect_ssn: bool,

    /// Enable credit card detection
    pub detect_credit_card: bool,

    /// Enable IP address detection
    pub detect_ip_address: bool,

    /// Custom regex patterns to detect
    pub custom_patterns: Vec<CustomPattern>,

    /// Minimum confidence threshold
    pub min_confidence: f32,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        Self {
            detect_email: true,
            detect_phone: true,
            detect_ssn: true,
            detect_credit_card: true,
            detect_ip_address: true,
            custom_patterns: Vec::new(),
            min_confidence: 0.7,
        }
    }
}

/// Redaction mode for custom patterns
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CustomRedactionMode {
    /// Use HMAC tokenization (deterministic, reversible)
    Tokenize,

    /// Use placeholder masking (simple string replacement)
    #[default]
    Mask,
}

/// Custom regex pattern for detection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomPattern {
    /// Name of the pattern
    pub name: String,

    /// Regex pattern
    pub pattern: String,

    /// Confidence score for matches
    pub confidence: f32,

    /// Redaction mode for this pattern
    #[serde(default)]
    pub redaction_mode: CustomRedactionMode,

    /// Placeholder text when redaction_mode is Mask (e.g., "[API_KEY]")
    /// If None, defaults to "[CUS:name]"
    pub placeholder: Option<String>,
}

#[cfg(test)]
mod tests;
