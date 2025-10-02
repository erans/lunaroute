//! Standard PII redactor implementation

use crate::detector::{Detection, PIIType};
use crate::redactor::{PIIRedactor, RedactionMode, RedactorConfig, TypeRedactionOverride};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;

type HmacSha256 = Hmac<Sha256>;

/// Standard implementation of PII redaction
pub struct StandardRedactor {
    config: RedactorConfig,
    type_overrides: HashMap<PIIType, TypeRedactionOverride>,
    hmac_key: Option<Vec<u8>>,
}

impl StandardRedactor {
    /// Create a new standard redactor with the given configuration
    pub fn new(config: RedactorConfig) -> Self {
        let type_overrides: HashMap<PIIType, TypeRedactionOverride> = config
            .type_overrides
            .iter()
            .map(|override_item| (override_item.pii_type, override_item.clone()))
            .collect();

        let hmac_key = config.hmac_secret.as_ref().map(|s| s.as_bytes().to_vec());

        Self {
            config,
            type_overrides,
            hmac_key,
        }
    }

    /// Get the redaction mode for a specific PII type
    fn get_mode_for_type(&self, pii_type: PIIType) -> RedactionMode {
        self.type_overrides
            .get(&pii_type)
            .map(|override_item| override_item.mode)
            .unwrap_or(self.config.mode)
    }

    /// Get the replacement text for a specific PII type
    fn get_replacement_for_type(&self, pii_type: PIIType) -> Option<&str> {
        self.type_overrides
            .get(&pii_type)
            .and_then(|override_item| override_item.replacement.as_deref())
    }

    /// Redact a single detection based on mode
    fn redact_detection(&self, detection: &Detection) -> String {
        let mode = self.get_mode_for_type(detection.pii_type);

        match mode {
            RedactionMode::Remove => String::new(),

            RedactionMode::Mask => {
                if let Some(replacement) = self.get_replacement_for_type(detection.pii_type) {
                    replacement.to_string()
                } else {
                    // Default masking based on type
                    match detection.pii_type {
                        PIIType::Email => "[EMAIL]".to_string(),
                        PIIType::Phone => "[PHONE]".to_string(),
                        PIIType::SSN => "[SSN]".to_string(),
                        PIIType::CreditCard => "[CREDIT_CARD]".to_string(),
                        PIIType::IPAddress => "[IP_ADDRESS]".to_string(),
                        PIIType::Custom => "[REDACTED]".to_string(),
                    }
                }
            }

            RedactionMode::Tokenize => {
                // Create a deterministic token using HMAC
                if let Some(key) = &self.hmac_key {
                    let mut mac = HmacSha256::new_from_slice(key)
                        .expect("HMAC can take key of any size");
                    mac.update(detection.text.as_bytes());
                    let result = mac.finalize();

                    // Use base64 engine for encoding
                    use base64::Engine;
                    let hash = base64::engine::general_purpose::STANDARD.encode(result.into_bytes());

                    // Use first 16 chars of base64-encoded hash for readability
                    let short_hash = &hash[..16.min(hash.len())];
                    format!("[{}:{}]", pii_type_abbrev(detection.pii_type), short_hash)
                } else {
                    // Fall back to masking if no HMAC key provided
                    // Don't call redact_detection recursively - just return the default mask
                    if let Some(replacement) = self.get_replacement_for_type(detection.pii_type) {
                        replacement.to_string()
                    } else {
                        match detection.pii_type {
                            PIIType::Email => "[EMAIL]".to_string(),
                            PIIType::Phone => "[PHONE]".to_string(),
                            PIIType::SSN => "[SSN]".to_string(),
                            PIIType::CreditCard => "[CREDIT_CARD]".to_string(),
                            PIIType::IPAddress => "[IP_ADDRESS]".to_string(),
                            PIIType::Custom => "[REDACTED]".to_string(),
                        }
                    }
                }
            }

            RedactionMode::Partial => {
                let show_chars = self.config.partial_show_chars;
                let text = &detection.text;

                if text.len() <= show_chars {
                    // If text is shorter than show_chars, just show asterisks
                    "*".repeat(text.len())
                } else {
                    // Show last N characters, mask the rest
                    let mask_len = text.len() - show_chars;
                    let masked = "*".repeat(mask_len);
                    let visible = &text[mask_len..];
                    format!("{}{}", masked, visible)
                }
            }
        }
    }
}

/// Get abbreviation for PII type
fn pii_type_abbrev(pii_type: PIIType) -> &'static str {
    match pii_type {
        PIIType::Email => "EM",
        PIIType::Phone => "PH",
        PIIType::SSN => "SSN",
        PIIType::CreditCard => "CC",
        PIIType::IPAddress => "IP",
        PIIType::Custom => "CUS",
    }
}

impl PIIRedactor for StandardRedactor {
    fn redact(&self, text: &str, detections: &[Detection]) -> String {
        if detections.is_empty() {
            return text.to_string();
        }

        let mut result = String::with_capacity(text.len());
        let mut last_end = 0;

        for detection in detections {
            // Add text before this detection
            result.push_str(&text[last_end..detection.start]);

            // Add redacted version
            result.push_str(&self.redact_detection(detection));

            last_end = detection.end;
        }

        // Add remaining text
        result.push_str(&text[last_end..]);

        result
    }

    fn mode(&self) -> RedactionMode {
        self.config.mode
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_detection(pii_type: PIIType, start: usize, end: usize, text: &str) -> Detection {
        Detection {
            pii_type,
            start,
            end,
            text: text.to_string(),
            confidence: 0.9,
        }
    }

    #[test]
    fn test_remove_mode() {
        let config = RedactorConfig {
            mode: RedactionMode::Remove,
            partial_show_chars: 4,
            hmac_secret: None,
            type_overrides: Vec::new(),
        };

        let redactor = StandardRedactor::new(config);
        let text = "Email: test@example.com and phone: 555-123-4567";
        let detections = vec![
            create_detection(PIIType::Email, 7, 23, "test@example.com"),
            create_detection(PIIType::Phone, 35, 47, "555-123-4567"),
        ];

        let redacted = redactor.redact(text, &detections);
        // Corrected expected output - "Email: " + "" + " and phone: " + ""
        assert_eq!(redacted, "Email:  and phone: ");
    }

    #[test]
    fn test_mask_mode() {
        let config = RedactorConfig {
            mode: RedactionMode::Mask,
            partial_show_chars: 4,
            hmac_secret: None,
            type_overrides: Vec::new(),
        };

        let redactor = StandardRedactor::new(config);
        let text = "Email: test@example.com and SSN: 123-45-6789";
        // Correct calculation: "Email: " (7) + "test@example.com" (16, ends at 23) + " and SSN: " (10, starts at 23, ends at 33) + "123-45-6789" (11, starts at 33, ends at 44)
        let detections = vec![
            create_detection(PIIType::Email, 7, 23, "test@example.com"),
            create_detection(PIIType::SSN, 33, 44, "123-45-6789"),
        ];

        let redacted = redactor.redact(text, &detections);
        assert_eq!(redacted, "Email: [EMAIL] and SSN: [SSN]");
    }

    #[test]
    fn test_tokenize_mode() {
        let config = RedactorConfig {
            mode: RedactionMode::Tokenize,
            partial_show_chars: 4,
            hmac_secret: Some("test-secret-key".to_string()),
            type_overrides: Vec::new(),
        };

        let redactor = StandardRedactor::new(config);
        let text = "Email: test@example.com";
        let detections = vec![create_detection(
            PIIType::Email,
            7,
            23,
            "test@example.com",
        )];

        let redacted = redactor.redact(text, &detections);

        // Should contain tokenized version with [EM:...] format
        assert!(redacted.contains("[EM:"));
        assert!(redacted.contains("]"));
        assert!(!redacted.contains("test@example.com"));

        // Same email should produce same token (deterministic)
        let redacted2 = redactor.redact(text, &detections);
        assert_eq!(redacted, redacted2);
    }

    #[test]
    fn test_partial_mode() {
        let config = RedactorConfig {
            mode: RedactionMode::Partial,
            partial_show_chars: 4,
            hmac_secret: None,
            type_overrides: Vec::new(),
        };

        let redactor = StandardRedactor::new(config);

        // Credit card
        let text = "Card: 4532-0151-1416-7894";
        let detections = vec![create_detection(
            PIIType::CreditCard,
            6,
            25,
            "4532-0151-1416-7894",
        )];

        let redacted = redactor.redact(text, &detections);
        assert!(redacted.ends_with("7894")); // Last 4 digits visible
        assert!(redacted.contains("***")); // Rest is masked

        // Phone
        let text = "Phone: 555-123-4567";
        let detections = vec![create_detection(PIIType::Phone, 7, 19, "555-123-4567")];

        let redacted = redactor.redact(text, &detections);
        assert!(redacted.ends_with("4567")); // Last 4 visible
        assert!(redacted.contains("***")); // Rest is masked
    }

    #[test]
    fn test_type_overrides() {
        let config = RedactorConfig {
            mode: RedactionMode::Mask,
            partial_show_chars: 4,
            hmac_secret: None,
            type_overrides: vec![TypeRedactionOverride {
                pii_type: PIIType::Email,
                mode: RedactionMode::Remove,
                replacement: None,
            }],
        };

        let redactor = StandardRedactor::new(config);
        let text = "Email: test@example.com and phone: 555-123-4567";
        let detections = vec![
            create_detection(PIIType::Email, 7, 23, "test@example.com"),
            create_detection(PIIType::Phone, 35, 47, "555-123-4567"),
        ];

        let redacted = redactor.redact(text, &detections);

        // Email should be removed (override)
        // Phone should be masked (default)
        assert_eq!(redacted, "Email:  and phone: [PHONE]");
    }

    #[test]
    fn test_custom_replacement() {
        let config = RedactorConfig {
            mode: RedactionMode::Mask,
            partial_show_chars: 4,
            hmac_secret: None,
            type_overrides: vec![TypeRedactionOverride {
                pii_type: PIIType::Email,
                mode: RedactionMode::Mask,
                replacement: Some("<EMAIL_REDACTED>".to_string()),
            }],
        };

        let redactor = StandardRedactor::new(config);
        let text = "Contact: test@example.com";
        let detections = vec![create_detection(
            PIIType::Email,
            9,
            25,
            "test@example.com",
        )];

        let redacted = redactor.redact(text, &detections);
        assert_eq!(redacted, "Contact: <EMAIL_REDACTED>");
    }

    #[test]
    fn test_multiple_detections_sorted() {
        let config = RedactorConfig {
            mode: RedactionMode::Mask,
            partial_show_chars: 4,
            hmac_secret: None,
            type_overrides: Vec::new(),
        };

        let redactor = StandardRedactor::new(config);
        let text = "IP: 192.168.1.1, Email: test@test.com, Phone: 555-1234";
        let detections = vec![
            create_detection(PIIType::IPAddress, 4, 15, "192.168.1.1"),
            create_detection(PIIType::Email, 24, 37, "test@test.com"),
            create_detection(PIIType::Phone, 46, 54, "555-1234"),
        ];

        let redacted = redactor.redact(text, &detections);
        assert_eq!(
            redacted,
            "IP: [IP_ADDRESS], Email: [EMAIL], Phone: [PHONE]"
        );
    }

    #[test]
    fn test_no_detections() {
        let config = RedactorConfig::default();
        let redactor = StandardRedactor::new(config);

        let text = "No PII here!";
        let redacted = redactor.redact(text, &[]);

        assert_eq!(redacted, text);
    }

    #[test]
    fn test_tokenize_without_hmac_key() {
        let config = RedactorConfig {
            mode: RedactionMode::Tokenize,
            partial_show_chars: 4,
            hmac_secret: None, // No key provided
            type_overrides: Vec::new(),
        };

        let redactor = StandardRedactor::new(config);
        let text = "Email: test@example.com";
        let detections = vec![create_detection(
            PIIType::Email,
            7,
            23,
            "test@example.com",
        )];

        // Should fall back to masking
        let redacted = redactor.redact(text, &detections);
        assert_eq!(redacted, "Email: [EMAIL]");
    }

    #[test]
    fn test_partial_mode_short_text() {
        let config = RedactorConfig {
            mode: RedactionMode::Partial,
            partial_show_chars: 10,
            hmac_secret: None,
            type_overrides: Vec::new(),
        };

        let redactor = StandardRedactor::new(config);
        let text = "Short: abc";
        let detections = vec![create_detection(PIIType::Custom, 7, 10, "abc")];

        let redacted = redactor.redact(text, &detections);
        // Text shorter than partial_show_chars, should be all asterisks
        assert!(redacted.contains("***"));
    }

    #[test]
    fn test_deterministic_tokenization() {
        let config = RedactorConfig {
            mode: RedactionMode::Tokenize,
            partial_show_chars: 4,
            hmac_secret: Some("secret123".to_string()),
            type_overrides: Vec::new(),
        };

        let redactor = StandardRedactor::new(config);

        let detection = create_detection(PIIType::Email, 0, 16, "test@example.com");

        let text1 = "test@example.com";
        let text2 = "test@example.com";

        let redacted1 = redactor.redact(text1, &[detection.clone()]);
        let redacted2 = redactor.redact(text2, &[detection]);

        // Same email with same key should produce same token
        assert_eq!(redacted1, redacted2);
    }

    #[test]
    fn test_different_keys_different_tokens() {
        let detection = create_detection(PIIType::Email, 0, 16, "test@example.com");
        let text = "test@example.com";

        let config1 = RedactorConfig {
            mode: RedactionMode::Tokenize,
            partial_show_chars: 4,
            hmac_secret: Some("key1".to_string()),
            type_overrides: Vec::new(),
        };

        let config2 = RedactorConfig {
            mode: RedactionMode::Tokenize,
            partial_show_chars: 4,
            hmac_secret: Some("key2".to_string()),
            type_overrides: Vec::new(),
        };

        let redactor1 = StandardRedactor::new(config1);
        let redactor2 = StandardRedactor::new(config2);

        let redacted1 = redactor1.redact(text, &[detection.clone()]);
        let redacted2 = redactor2.redact(text, &[detection]);

        // Different keys should produce different tokens
        assert_ne!(redacted1, redacted2);
    }
}
