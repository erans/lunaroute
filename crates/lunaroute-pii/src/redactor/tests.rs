//! Tests for PII redactor types

use super::*;
use crate::detector::PIIType;

#[test]
fn test_redaction_mode_variants() {
    let modes = vec![
        RedactionMode::Remove,
        RedactionMode::Mask,
        RedactionMode::Tokenize,
        RedactionMode::Partial,
    ];

    for mode in modes {
        let json = serde_json::to_string(&mode).unwrap();
        let deserialized: RedactionMode = serde_json::from_str(&json).unwrap();
        assert_eq!(mode, deserialized);
    }
}

#[test]
fn test_redactor_config_default() {
    let config = RedactorConfig::default();

    assert_eq!(config.mode, RedactionMode::Mask);
    assert_eq!(config.partial_show_chars, 4);
    assert!(config.hmac_secret.is_none());
    assert_eq!(config.type_overrides.len(), 0);
}

#[test]
fn test_redactor_config_custom() {
    let config = RedactorConfig {
        mode: RedactionMode::Tokenize,
        partial_show_chars: 6,
        hmac_secret: Some("secret_key_123".to_string()),
        type_overrides: vec![TypeRedactionOverride {
            pii_type: PIIType::Email,
            mode: RedactionMode::Partial,
            replacement: None,
        }],
    };

    assert_eq!(config.mode, RedactionMode::Tokenize);
    assert_eq!(config.partial_show_chars, 6);
    assert!(config.hmac_secret.is_some());
    assert_eq!(config.type_overrides.len(), 1);
}

#[test]
fn test_type_redaction_override() {
    let override_config = TypeRedactionOverride {
        pii_type: PIIType::CreditCard,
        mode: RedactionMode::Partial,
        replacement: Some("[CARD]".to_string()),
    };

    assert_eq!(override_config.pii_type, PIIType::CreditCard);
    assert_eq!(override_config.mode, RedactionMode::Partial);
    assert_eq!(override_config.replacement.as_ref().unwrap(), "[CARD]");
}

#[test]
fn test_type_override_serialization() {
    let override_config = TypeRedactionOverride {
        pii_type: PIIType::SSN,
        mode: RedactionMode::Mask,
        replacement: Some("***-**-****".to_string()),
    };

    let json = serde_json::to_string(&override_config).unwrap();
    let deserialized: TypeRedactionOverride = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.pii_type, PIIType::SSN);
    assert_eq!(deserialized.mode, RedactionMode::Mask);
}

#[test]
fn test_multiple_type_overrides() {
    let overrides = [
        TypeRedactionOverride {
            pii_type: PIIType::Email,
            mode: RedactionMode::Partial,
            replacement: None,
        },
        TypeRedactionOverride {
            pii_type: PIIType::Phone,
            mode: RedactionMode::Mask,
            replacement: Some("[PHONE]".to_string()),
        },
        TypeRedactionOverride {
            pii_type: PIIType::CreditCard,
            mode: RedactionMode::Remove,
            replacement: None,
        },
    ];

    assert_eq!(overrides.len(), 3);
    assert_eq!(overrides[0].pii_type, PIIType::Email);
    assert_eq!(overrides[1].mode, RedactionMode::Mask);
    assert_eq!(overrides[2].mode, RedactionMode::Remove);
}

#[test]
fn test_redactor_config_serialization() {
    let config = RedactorConfig {
        mode: RedactionMode::Tokenize,
        partial_show_chars: 8,
        hmac_secret: Some("test_secret".to_string()),
        type_overrides: vec![],
    };

    let json = serde_json::to_string(&config).unwrap();
    let deserialized: RedactorConfig = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.mode, RedactionMode::Tokenize);
    assert_eq!(deserialized.partial_show_chars, 8);
}

#[test]
fn test_redaction_mode_equality() {
    assert_eq!(RedactionMode::Mask, RedactionMode::Mask);
    assert_ne!(RedactionMode::Mask, RedactionMode::Remove);
    assert_ne!(RedactionMode::Tokenize, RedactionMode::Partial);
}
