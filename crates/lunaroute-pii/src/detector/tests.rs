//! Tests for PII detector types

use super::*;

#[test]
fn test_pii_type_variants() {
    let types = vec![
        PIIType::Email,
        PIIType::Phone,
        PIIType::SSN,
        PIIType::CreditCard,
        PIIType::IPAddress,
        PIIType::Custom,
    ];

    for pii_type in types {
        let json = serde_json::to_string(&pii_type).unwrap();
        let deserialized: PIIType = serde_json::from_str(&json).unwrap();
        assert_eq!(pii_type, deserialized);
    }
}

#[test]
fn test_detection_structure() {
    let detection = Detection {
        pii_type: PIIType::Email,
        start: 10,
        end: 30,
        text: "test@example.com".to_string(),
        confidence: 0.95,
    };

    assert_eq!(detection.pii_type, PIIType::Email);
    assert_eq!(detection.text.len(), 16);
    assert!(detection.confidence > 0.9);
}

#[test]
fn test_detection_serialization() {
    let detection = Detection {
        pii_type: PIIType::CreditCard,
        start: 0,
        end: 19,
        text: "4111-1111-1111-1111".to_string(),
        confidence: 0.99,
    };

    let json = serde_json::to_string(&detection).unwrap();
    let deserialized: Detection = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.pii_type, PIIType::CreditCard);
    assert_eq!(deserialized.start, 0);
    assert_eq!(deserialized.end, 19);
}

#[test]
fn test_detector_config_default() {
    let config = DetectorConfig::default();

    assert!(config.detect_email);
    assert!(config.detect_phone);
    assert!(config.detect_ssn);
    assert!(config.detect_credit_card);
    assert!(config.detect_ip_address);
    assert_eq!(config.custom_patterns.len(), 0);
    assert_eq!(config.min_confidence, 0.7);
}

#[test]
fn test_detector_config_custom() {
    let config = DetectorConfig {
        detect_email: true,
        detect_phone: false,
        detect_ssn: false,
        detect_credit_card: true,
        detect_ip_address: false,
        custom_patterns: vec![CustomPattern {
            name: "api_key".to_string(),
            pattern: r"sk-[a-zA-Z0-9]{32}".to_string(),
            confidence: 0.9,
        }],
        min_confidence: 0.8,
    };

    assert!(config.detect_email);
    assert!(!config.detect_phone);
    assert_eq!(config.custom_patterns.len(), 1);
    assert_eq!(config.min_confidence, 0.8);
}

#[test]
fn test_custom_pattern() {
    let pattern = CustomPattern {
        name: "api_token".to_string(),
        pattern: r"[A-Z0-9]{32}".to_string(),
        confidence: 0.85,
    };

    assert_eq!(pattern.name, "api_token");
    assert_eq!(pattern.confidence, 0.85);
}

#[test]
fn test_custom_pattern_serialization() {
    let pattern = CustomPattern {
        name: "secret".to_string(),
        pattern: r"\bsecret_\w+".to_string(),
        confidence: 0.75,
    };

    let json = serde_json::to_string(&pattern).unwrap();
    let deserialized: CustomPattern = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.name, "secret");
}

#[test]
fn test_multiple_detections() {
    let detections = [
        Detection {
            pii_type: PIIType::Email,
            start: 0,
            end: 16,
            text: "test@example.com".to_string(),
            confidence: 0.95,
        },
        Detection {
            pii_type: PIIType::Phone,
            start: 20,
            end: 32,
            text: "555-123-4567".to_string(),
            confidence: 0.88,
        },
    ];

    assert_eq!(detections.len(), 2);
    assert_eq!(detections[0].pii_type, PIIType::Email);
    assert_eq!(detections[1].pii_type, PIIType::Phone);
}

#[test]
fn test_detection_equality() {
    let det1 = Detection {
        pii_type: PIIType::SSN,
        start: 0,
        end: 11,
        text: "123-45-6789".to_string(),
        confidence: 0.9,
    };

    let det2 = Detection {
        pii_type: PIIType::SSN,
        start: 0,
        end: 11,
        text: "123-45-6789".to_string(),
        confidence: 0.9,
    };

    assert_eq!(det1, det2);
}

#[test]
fn test_pii_type_hash() {
    use std::collections::HashSet;

    let mut set = HashSet::new();
    set.insert(PIIType::Email);
    set.insert(PIIType::Phone);
    set.insert(PIIType::Email); // Duplicate

    assert_eq!(set.len(), 2);
    assert!(set.contains(&PIIType::Email));
    assert!(set.contains(&PIIType::Phone));
}
