//! Regex-based PII detector implementation

use crate::detector::{Detection, DetectorConfig, PIIDetector, PIIType};
use regex::Regex;
use std::sync::Arc;

/// Regex-based PII detector
pub struct RegexPIIDetector {
    config: DetectorConfig,
    email_regex: Arc<Regex>,
    phone_regex: Arc<Regex>,
    ssn_regex: Arc<Regex>,
    credit_card_regex: Arc<Regex>,
    ip_regex: Arc<Regex>,
    custom_regexes: Vec<(String, Arc<Regex>, f32)>, // (name, regex, confidence)
}

impl RegexPIIDetector {
    /// Create a new regex-based PII detector with the given configuration
    pub fn new(config: DetectorConfig) -> Result<Self, regex::Error> {
        // Compile all regex patterns
        let email_regex = Arc::new(Regex::new(
            r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Z|a-z]{2,}\b",
        )?);

        // Phone numbers: (123) 456-7890, 123-456-7890, 123.456.7890, +1 123 456 7890
        let phone_regex = Arc::new(Regex::new(
            r"(\+?\d{1,3}[-.\s]?)?(\(?\d{3}\)?[-.\s]?)?\d{3}[-.\s]?\d{4}\b",
        )?);

        // SSN: 123-45-6789 or 123456789
        let ssn_regex = Arc::new(Regex::new(r"\b\d{3}-?\d{2}-?\d{4}\b")?);

        // Credit cards: various formats (Visa, MC, Amex, Discover)
        // Matches 13-19 digit sequences with optional spaces/dashes
        let credit_card_regex = Arc::new(Regex::new(
            r"\b(?:\d{4}[-\s]?){3}\d{4,7}\b",
        )?);

        // IP addresses: IPv4 and IPv6
        let ip_regex = Arc::new(Regex::new(
            r"\b(?:(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\b|\b(?:[0-9a-fA-F]{1,4}:){7}[0-9a-fA-F]{1,4}\b",
        )?);

        // Compile custom patterns
        let mut custom_regexes = Vec::new();
        for pattern in &config.custom_patterns {
            let regex = Regex::new(&pattern.pattern)?;
            custom_regexes.push((
                pattern.name.clone(),
                Arc::new(regex),
                pattern.confidence,
            ));
        }

        Ok(Self {
            config,
            email_regex,
            phone_regex,
            ssn_regex,
            credit_card_regex,
            ip_regex,
            custom_regexes,
        })
    }

    /// Validate a potential credit card number using Luhn algorithm
    fn validate_credit_card(&self, number: &str) -> bool {
        let digits: Vec<u32> = number
            .chars()
            .filter(|c| c.is_ascii_digit())
            .filter_map(|c| c.to_digit(10))
            .collect();

        if digits.len() < 13 || digits.len() > 19 {
            return false;
        }

        // Luhn algorithm
        let checksum: u32 = digits
            .iter()
            .rev()
            .enumerate()
            .map(|(i, &d)| {
                if i % 2 == 1 {
                    let doubled = d * 2;
                    if doubled > 9 {
                        doubled - 9
                    } else {
                        doubled
                    }
                } else {
                    d
                }
            })
            .sum();

        checksum.is_multiple_of(10)
    }

    /// Validate a potential SSN
    fn validate_ssn(&self, ssn: &str) -> bool {
        let digits: String = ssn.chars().filter(|c| c.is_ascii_digit()).collect();

        if digits.len() != 9 {
            return false;
        }

        // Invalid SSN patterns
        // All zeros in any group
        if digits.starts_with("000")
            || digits[3..5] == *"00"
            || digits[5..9] == *"0000"
        {
            return false;
        }

        // SSNs starting with 666
        if digits.starts_with("666") {
            return false;
        }

        // SSNs starting with 9 (reserved for ITIN)
        if digits.starts_with('9') {
            return false;
        }

        true
    }

    /// Validate a potential phone number
    fn validate_phone(&self, phone: &str) -> bool {
        let digits: String = phone.chars().filter(|c| c.is_ascii_digit()).collect();

        // US/Canada phone numbers are 10 or 11 digits (with country code)
        // International can be more varied
        if digits.len() < 10 || digits.len() > 15 {
            return false;
        }

        // If 11 digits, should start with 1 (US/Canada country code)
        if digits.len() == 11 && !digits.starts_with('1') {
            return false;
        }

        true
    }
}

impl PIIDetector for RegexPIIDetector {
    fn detect(&self, text: &str) -> Vec<Detection> {
        let mut detections = Vec::new();

        // Email detection
        if self.config.detect_email {
            for capture in self.email_regex.find_iter(text) {
                let confidence = 0.95; // Email regex is quite specific
                if confidence >= self.config.min_confidence {
                    detections.push(Detection {
                        pii_type: PIIType::Email,
                        start: capture.start(),
                        end: capture.end(),
                        text: capture.as_str().to_string(),
                        confidence,
                    });
                }
            }
        }

        // Phone detection
        if self.config.detect_phone {
            for capture in self.phone_regex.find_iter(text) {
                let phone = capture.as_str();
                if self.validate_phone(phone) {
                    let confidence = 0.85; // Phone patterns can have false positives
                    if confidence >= self.config.min_confidence {
                        detections.push(Detection {
                            pii_type: PIIType::Phone,
                            start: capture.start(),
                            end: capture.end(),
                            text: phone.to_string(),
                            confidence,
                        });
                    }
                }
            }
        }

        // SSN detection
        if self.config.detect_ssn {
            for capture in self.ssn_regex.find_iter(text) {
                let ssn = capture.as_str();
                if self.validate_ssn(ssn) {
                    let confidence = 0.9; // SSN with validation is high confidence
                    if confidence >= self.config.min_confidence {
                        detections.push(Detection {
                            pii_type: PIIType::SSN,
                            start: capture.start(),
                            end: capture.end(),
                            text: ssn.to_string(),
                            confidence,
                        });
                    }
                }
            }
        }

        // Credit card detection
        if self.config.detect_credit_card {
            for capture in self.credit_card_regex.find_iter(text) {
                let card = capture.as_str();
                if self.validate_credit_card(card) {
                    let confidence = 0.95; // Luhn validation gives high confidence
                    if confidence >= self.config.min_confidence {
                        detections.push(Detection {
                            pii_type: PIIType::CreditCard,
                            start: capture.start(),
                            end: capture.end(),
                            text: card.to_string(),
                            confidence,
                        });
                    }
                }
            }
        }

        // IP address detection
        if self.config.detect_ip_address {
            for capture in self.ip_regex.find_iter(text) {
                let confidence = 0.99; // IP regex is very specific
                if confidence >= self.config.min_confidence {
                    detections.push(Detection {
                        pii_type: PIIType::IPAddress,
                        start: capture.start(),
                        end: capture.end(),
                        text: capture.as_str().to_string(),
                        confidence,
                    });
                }
            }
        }

        // Custom pattern detection
        for (name, regex, confidence) in &self.custom_regexes {
            if *confidence >= self.config.min_confidence {
                for capture in regex.find_iter(text) {
                    detections.push(Detection {
                        pii_type: PIIType::Custom,
                        start: capture.start(),
                        end: capture.end(),
                        text: format!("{}:{}", name, capture.as_str()),
                        confidence: *confidence,
                    });
                }
            }
        }

        // Sort detections by position
        detections.sort_by_key(|d| d.start);

        detections
    }

    fn supported_types(&self) -> Vec<PIIType> {
        let mut types = Vec::new();

        if self.config.detect_email {
            types.push(PIIType::Email);
        }
        if self.config.detect_phone {
            types.push(PIIType::Phone);
        }
        if self.config.detect_ssn {
            types.push(PIIType::SSN);
        }
        if self.config.detect_credit_card {
            types.push(PIIType::CreditCard);
        }
        if self.config.detect_ip_address {
            types.push(PIIType::IPAddress);
        }
        if !self.custom_regexes.is_empty() {
            types.push(PIIType::Custom);
        }

        types
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detector::CustomPattern;

    #[test]
    fn test_email_detection() {
        let config = DetectorConfig {
            detect_email: true,
            detect_phone: false,
            detect_ssn: false,
            detect_credit_card: false,
            detect_ip_address: false,
            custom_patterns: Vec::new(),
            min_confidence: 0.7,
        };

        let detector = RegexPIIDetector::new(config).unwrap();
        let text = "Contact me at john.doe@example.com for more info.";
        let detections = detector.detect(text);

        assert_eq!(detections.len(), 1);
        assert_eq!(detections[0].pii_type, PIIType::Email);
        assert_eq!(detections[0].text, "john.doe@example.com");
        assert!(detections[0].confidence >= 0.9);
    }

    #[test]
    fn test_phone_detection() {
        let config = DetectorConfig {
            detect_email: false,
            detect_phone: true,
            detect_ssn: false,
            detect_credit_card: false,
            detect_ip_address: false,
            custom_patterns: Vec::new(),
            min_confidence: 0.7,
        };

        let detector = RegexPIIDetector::new(config).unwrap();
        let text = "Call me at (555) 123-4567 or 555-987-6543.";
        let detections = detector.detect(text);

        assert_eq!(detections.len(), 2);
        assert!(detections.iter().all(|d| d.pii_type == PIIType::Phone));
    }

    #[test]
    fn test_ssn_detection() {
        let config = DetectorConfig {
            detect_email: false,
            detect_phone: false,
            detect_ssn: true,
            detect_credit_card: false,
            detect_ip_address: false,
            custom_patterns: Vec::new(),
            min_confidence: 0.7,
        };

        let detector = RegexPIIDetector::new(config).unwrap();

        // Valid SSN
        let text = "My SSN is 123-45-6789";
        let detections = detector.detect(text);
        assert_eq!(detections.len(), 1);
        assert_eq!(detections[0].pii_type, PIIType::SSN);

        // Invalid SSN (starts with 000)
        let text = "Bad SSN: 000-12-3456";
        let detections = detector.detect(text);
        assert_eq!(detections.len(), 0);

        // Invalid SSN (starts with 666)
        let text = "Bad SSN: 666-12-3456";
        let detections = detector.detect(text);
        assert_eq!(detections.len(), 0);
    }

    #[test]
    fn test_credit_card_detection() {
        let config = DetectorConfig {
            detect_email: false,
            detect_phone: false,
            detect_ssn: false,
            detect_credit_card: true,
            detect_ip_address: false,
            custom_patterns: Vec::new(),
            min_confidence: 0.7,
        };

        let detector = RegexPIIDetector::new(config).unwrap();

        // Valid Visa test card (passes Luhn algorithm)
        // 4532015112830366 is a known valid test card
        let text = "Card: 4532-0151-1283-0366";
        let detections = detector.detect(text);
        assert_eq!(detections.len(), 1);
        assert_eq!(detections[0].pii_type, PIIType::CreditCard);

        // Invalid card (fails Luhn)
        let text = "Bad card: 4532-0151-1283-0367";
        let detections = detector.detect(text);
        assert_eq!(detections.len(), 0);
    }

    #[test]
    fn test_ip_detection() {
        let config = DetectorConfig {
            detect_email: false,
            detect_phone: false,
            detect_ssn: false,
            detect_credit_card: false,
            detect_ip_address: true,
            custom_patterns: Vec::new(),
            min_confidence: 0.7,
        };

        let detector = RegexPIIDetector::new(config).unwrap();
        let text = "Server IP: 192.168.1.1 and IPv6: 2001:0db8:85a3:0000:0000:8a2e:0370:7334";
        let detections = detector.detect(text);

        assert_eq!(detections.len(), 2);
        assert!(detections.iter().all(|d| d.pii_type == PIIType::IPAddress));
    }

    #[test]
    fn test_custom_pattern() {
        let config = DetectorConfig {
            detect_email: false,
            detect_phone: false,
            detect_ssn: false,
            detect_credit_card: false,
            detect_ip_address: false,
            custom_patterns: vec![CustomPattern {
                name: "api_key".to_string(),
                pattern: r"sk-[a-zA-Z0-9]{32}".to_string(),
                confidence: 0.9,
            }],
            min_confidence: 0.7,
        };

        let detector = RegexPIIDetector::new(config).unwrap();
        let text = "API key: sk-abcdefghijklmnopqrstuvwxyz123456";
        let detections = detector.detect(text);

        assert_eq!(detections.len(), 1);
        assert_eq!(detections[0].pii_type, PIIType::Custom);
        assert!(detections[0].text.contains("api_key"));
    }

    #[test]
    fn test_multiple_pii_types() {
        let config = DetectorConfig::default();
        let detector = RegexPIIDetector::new(config).unwrap();

        // Using valid test card 4532015112830366
        let text = "Contact john@example.com at 555-123-4567. SSN: 123-45-6789, Card: 4532-0151-1283-0366, IP: 192.168.1.1";
        let detections = detector.detect(text);

        assert!(detections.len() >= 4);
        assert!(detections.iter().any(|d| d.pii_type == PIIType::Email));
        assert!(detections.iter().any(|d| d.pii_type == PIIType::Phone));
        assert!(detections.iter().any(|d| d.pii_type == PIIType::SSN));
        assert!(detections.iter().any(|d| d.pii_type == PIIType::CreditCard));
        assert!(detections.iter().any(|d| d.pii_type == PIIType::IPAddress));
    }

    #[test]
    fn test_min_confidence_filtering() {
        let config = DetectorConfig {
            detect_email: true,
            detect_phone: true,
            detect_ssn: true,
            detect_credit_card: true,
            detect_ip_address: true,
            custom_patterns: Vec::new(),
            min_confidence: 0.99, // Very high threshold
        };

        let detector = RegexPIIDetector::new(config).unwrap();

        // Only IP should pass (confidence 0.99), others are lower
        let text = "Email: test@test.com, IP: 192.168.1.1";
        let detections = detector.detect(text);

        assert!(detections.iter().all(|d| d.confidence >= 0.99));
    }

    #[test]
    fn test_sorted_detections() {
        let config = DetectorConfig::default();
        let detector = RegexPIIDetector::new(config).unwrap();

        let text = "IP 192.168.1.1 and email test@test.com and phone 555-123-4567";
        let detections = detector.detect(text);

        // Verify detections are sorted by start position
        for i in 1..detections.len() {
            assert!(detections[i].start >= detections[i - 1].start);
        }
    }
}
