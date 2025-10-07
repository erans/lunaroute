//! PII redaction for session recording
//!
//! This module provides integration between session recording and PII detection/redaction.

use crate::config::PIIConfig;
use lunaroute_core::{Result, normalized::*};
use lunaroute_pii::{
    CustomPattern, CustomRedactionMode, DetectorConfig, PIIDetector, PIIRedactor, RedactionMode,
    RedactorConfig, RegexPIIDetector, StandardRedactor,
};
use std::sync::Arc;

/// PII redaction service for session recording
pub struct SessionPIIRedactor {
    detector: Arc<RegexPIIDetector>,
    redactor: Arc<StandardRedactor>,
}

impl SessionPIIRedactor {
    /// Redact PII from a JSON string, preserving JSON structure
    fn redact_json_string(&self, json_str: &str) -> String {
        // Try to parse as JSON
        match serde_json::from_str::<serde_json::Value>(json_str) {
            Ok(mut value) => {
                // Recursively redact string values in JSON
                self.redact_json_value(&mut value);
                // Serialize back to JSON
                serde_json::to_string(&value).unwrap_or_else(|_| json_str.to_string())
            }
            Err(_) => {
                // Not valid JSON, fall back to string redaction
                let detections = self.detector.detect(json_str);
                self.redactor.redact(json_str, &detections)
            }
        }
    }

    /// Recursively redact PII in JSON values
    fn redact_json_value(&self, value: &mut serde_json::Value) {
        use serde_json::Value;
        match value {
            Value::String(s) => {
                let detections = self.detector.detect(s);
                *s = self.redactor.redact(s, &detections);
            }
            Value::Array(arr) => {
                for item in arr {
                    self.redact_json_value(item);
                }
            }
            Value::Object(obj) => {
                for (_key, val) in obj.iter_mut() {
                    self.redact_json_value(val);
                }
            }
            _ => {
                // Numbers, booleans, null - no PII to redact
            }
        }
    }

    /// Create a new PII redactor from configuration
    pub fn from_config(config: &PIIConfig) -> Result<Self> {
        // Convert config to DetectorConfig
        let custom_patterns: Vec<CustomPattern> = config
            .custom_patterns
            .iter()
            .map(|p| {
                let redaction_mode = match p.redaction_mode.as_str() {
                    "tokenize" => CustomRedactionMode::Tokenize,
                    _ => CustomRedactionMode::Mask,
                };

                CustomPattern {
                    name: p.name.clone(),
                    pattern: p.pattern.clone(),
                    confidence: p.confidence,
                    redaction_mode,
                    placeholder: p.placeholder.clone(),
                }
            })
            .collect();

        let detector_config = DetectorConfig {
            detect_email: config.detect_email,
            detect_phone: config.detect_phone,
            detect_ssn: config.detect_ssn,
            detect_credit_card: config.detect_credit_card,
            detect_ip_address: config.detect_ip_address,
            custom_patterns: custom_patterns.clone(),
            min_confidence: config.min_confidence,
        };

        let detector = RegexPIIDetector::new(detector_config).map_err(|e| {
            lunaroute_core::Error::Internal(format!("Failed to create PII detector: {}", e))
        })?;

        // Convert redaction mode string to enum
        let redaction_mode = match config.redaction_mode.as_str() {
            "remove" => RedactionMode::Remove,
            "mask" => RedactionMode::Mask,
            "tokenize" => RedactionMode::Tokenize,
            "partial" => RedactionMode::Partial,
            _ => RedactionMode::Mask, // Default
        };

        let redactor_config = RedactorConfig {
            mode: redaction_mode,
            partial_show_chars: config.partial_show_chars,
            hmac_secret: config.hmac_secret.clone(),
            type_overrides: Vec::new(),
        };

        let redactor = StandardRedactor::with_custom_patterns(redactor_config, custom_patterns);

        Ok(Self {
            detector: Arc::new(detector),
            redactor: Arc::new(redactor),
        })
    }

    /// Redact PII from a normalized request
    pub fn redact_request(&self, request: &mut NormalizedRequest) {
        // Redact PII from messages
        for message in &mut request.messages {
            match &mut message.content {
                MessageContent::Text(text) => {
                    let detections = self.detector.detect(text);
                    *text = self.redactor.redact(text, &detections);
                }
                MessageContent::Parts(parts) => {
                    for part in parts {
                        if let ContentPart::Text { text } = part {
                            let detections = self.detector.detect(text);
                            *text = self.redactor.redact(text, &detections);
                        }
                    }
                }
            }
        }

        // Redact PII from tool results if present
        for message in &mut request.messages {
            for tool_call in &mut message.tool_calls {
                // Redact from arguments (which is a JSON string) - use JSON-aware redaction
                tool_call.function.arguments =
                    self.redact_json_string(&tool_call.function.arguments);
            }

            if let Some(tool_call_id) = &mut message.tool_call_id {
                let detections = self.detector.detect(tool_call_id);
                *tool_call_id = self.redactor.redact(tool_call_id, &detections);
            }
        }
    }

    /// Redact PII from a normalized response
    pub fn redact_response(&self, response: &mut NormalizedResponse) {
        // Redact PII from choices
        for choice in &mut response.choices {
            match &mut choice.message.content {
                MessageContent::Text(text) => {
                    let detections = self.detector.detect(text);
                    *text = self.redactor.redact(text, &detections);
                }
                MessageContent::Parts(parts) => {
                    for part in parts {
                        if let ContentPart::Text { text } = part {
                            let detections = self.detector.detect(text);
                            *text = self.redactor.redact(text, &detections);
                        }
                    }
                }
            }

            // Redact from tool calls if present
            for tool_call in &mut choice.message.tool_calls {
                // Use JSON-aware redaction for tool call arguments
                tool_call.function.arguments =
                    self.redact_json_string(&tool_call.function.arguments);
            }
        }
    }

    /// Redact PII from a normalized stream event
    pub fn redact_stream_event(&self, event: &mut NormalizedStreamEvent) {
        match event {
            NormalizedStreamEvent::Delta { delta, .. } => {
                // Redact content delta
                if let Some(content) = &mut delta.content {
                    let detections = self.detector.detect(content);
                    *content = self.redactor.redact(content, &detections);
                }
            }
            NormalizedStreamEvent::ToolCallDelta { function, .. } => {
                // Redact function arguments delta
                // Note: streaming arguments may not be complete JSON, so we use string redaction
                if let Some(func) = function
                    && let Some(arguments) = &mut func.arguments
                {
                    let detections = self.detector.detect(arguments);
                    *arguments = self.redactor.redact(arguments, &detections);
                }
            }
            _ => {
                // Other event types don't have text content to redact
            }
        }
    }
}

#[cfg(test)]
#[path = "pii_redaction_tests.rs"]
mod tests;
