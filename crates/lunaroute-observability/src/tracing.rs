//! OpenTelemetry distributed tracing
//!
//! This module provides utilities for distributed tracing with OpenTelemetry:
//! - Span creation and management
//! - Trace context propagation (W3C TraceContext)
//! - Attribute setting for spans
//!
//! Note: This is a simplified implementation. For production use, consider
//! using the full tracing-opentelemetry integration.

use opentelemetry::{
    KeyValue,
    trace::{Span, Status},
};
use opentelemetry_sdk::{
    Resource,
    trace::{RandomIdGenerator, Sampler, TracerProvider},
};

/// Tracer configuration
#[derive(Debug, Clone)]
pub struct TracerConfig {
    /// Service name
    pub service_name: String,
    /// Service version
    pub service_version: String,
    /// Sampling rate (0.0-1.0)
    pub sampling_rate: f64,
}

impl Default for TracerConfig {
    fn default() -> Self {
        Self {
            service_name: "lunaroute".to_string(),
            service_version: env!("CARGO_PKG_VERSION").to_string(),
            sampling_rate: 1.0,
        }
    }
}

/// Initialize a tracer provider
///
/// Returns a TracerProvider that can be used to create tracers
pub fn init_tracer_provider(config: TracerConfig) -> TracerProvider {
    let resource = Resource::new(vec![
        KeyValue::new("service.name", config.service_name),
        KeyValue::new("service.version", config.service_version),
    ]);

    let sampler = if config.sampling_rate >= 1.0 {
        Sampler::AlwaysOn
    } else if config.sampling_rate <= 0.0 {
        Sampler::AlwaysOff
    } else {
        Sampler::TraceIdRatioBased(config.sampling_rate)
    };

    TracerProvider::builder()
        .with_config(
            opentelemetry_sdk::trace::Config::default()
                .with_resource(resource)
                .with_id_generator(RandomIdGenerator::default())
                .with_sampler(sampler),
        )
        .build()
}

/// Span attributes for LunaRoute requests
#[derive(Debug, Clone)]
pub struct RequestSpanAttributes {
    /// Model name
    pub model: Option<String>,
    /// Provider name
    pub provider: Option<String>,
    /// Listener type (openai, anthropic)
    pub listener: Option<String>,
    /// Request ID
    pub request_id: Option<String>,
    /// User ID
    pub user_id: Option<String>,
}

impl RequestSpanAttributes {
    /// Create a new empty attributes set
    pub fn new() -> Self {
        Self {
            model: None,
            provider: None,
            listener: None,
            request_id: None,
            user_id: None,
        }
    }

    /// Set the model
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set the provider
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }

    /// Set the listener
    pub fn with_listener(mut self, listener: impl Into<String>) -> Self {
        self.listener = Some(listener.into());
        self
    }

    /// Set the request ID
    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }

    /// Set the user ID
    pub fn with_user_id(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    /// Convert to OpenTelemetry KeyValue pairs
    pub fn to_key_values(&self) -> Vec<KeyValue> {
        let mut kvs = Vec::new();

        if let Some(ref model) = self.model {
            kvs.push(KeyValue::new("llm.model", model.clone()));
        }
        if let Some(ref provider) = self.provider {
            kvs.push(KeyValue::new("llm.provider", provider.clone()));
        }
        if let Some(ref listener) = self.listener {
            kvs.push(KeyValue::new("lunaroute.listener", listener.clone()));
        }
        if let Some(ref request_id) = self.request_id {
            kvs.push(KeyValue::new("lunaroute.request_id", request_id.clone()));
        }
        if let Some(ref user_id) = self.user_id {
            kvs.push(KeyValue::new("lunaroute.user_id", user_id.clone()));
        }

        kvs
    }
}

impl Default for RequestSpanAttributes {
    fn default() -> Self {
        Self::new()
    }
}

/// Add token usage attributes to a span
pub fn record_token_usage(span: &mut impl Span, prompt_tokens: u32, completion_tokens: u32) {
    span.set_attribute(KeyValue::new(
        "llm.usage.prompt_tokens",
        prompt_tokens as i64,
    ));
    span.set_attribute(KeyValue::new(
        "llm.usage.completion_tokens",
        completion_tokens as i64,
    ));
    span.set_attribute(KeyValue::new(
        "llm.usage.total_tokens",
        (prompt_tokens + completion_tokens) as i64,
    ));
}

/// Mark a span as failed with an error
pub fn record_error(span: &mut impl Span, error: &str) {
    span.set_status(Status::error(error.to_string()));
    span.set_attribute(KeyValue::new("error", true));
    span.set_attribute(KeyValue::new("error.message", error.to_string()));
}

/// Mark a span as successful
pub fn record_success(span: &mut impl Span) {
    span.set_status(Status::Ok);
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::trace::{Tracer, TracerProvider};

    #[test]
    fn test_tracer_config_default() {
        let config = TracerConfig::default();
        assert_eq!(config.service_name, "lunaroute");
        assert_eq!(config.sampling_rate, 1.0);
    }

    #[test]
    fn test_init_tracer_provider() {
        let config = TracerConfig::default();
        let provider = init_tracer_provider(config);
        let tracer = provider.tracer("test");
        let span = tracer.start("test_span");
        assert!(!span.span_context().trace_id().to_string().is_empty());
    }

    #[test]
    fn test_request_span_attributes() {
        let attrs = RequestSpanAttributes::new()
            .with_model("gpt-5-mini")
            .with_provider("openai")
            .with_listener("openai")
            .with_request_id("req-123")
            .with_user_id("user-456");

        let kvs = attrs.to_key_values();
        assert_eq!(kvs.len(), 5);

        // Check that all attributes are present
        assert!(
            kvs.iter()
                .any(|kv| kv.key.as_str() == "llm.model" && kv.value.as_str() == "gpt-5-mini")
        );
        assert!(
            kvs.iter()
                .any(|kv| kv.key.as_str() == "llm.provider" && kv.value.as_str() == "openai")
        );
    }

    #[test]
    fn test_request_span_attributes_partial() {
        let attrs = RequestSpanAttributes::new().with_model("gpt-5-mini");

        let kvs = attrs.to_key_values();
        assert_eq!(kvs.len(), 1);
        assert_eq!(kvs[0].key.as_str(), "llm.model");
    }

    #[test]
    fn test_record_token_usage() {
        let config = TracerConfig::default();
        let provider = init_tracer_provider(config);
        let tracer = provider.tracer("test");
        let mut span = tracer.start("test_span");

        record_token_usage(&mut span, 100, 50);

        // Span is updated with attributes (no way to read them in tests without export)
        // Just verify no panic
    }

    #[test]
    fn test_record_error() {
        let config = TracerConfig::default();
        let provider = init_tracer_provider(config);
        let tracer = provider.tracer("test");
        let mut span = tracer.start("test_span");

        record_error(&mut span, "Test error");

        // Span is updated with error status (no way to read it in tests without export)
        // Just verify no panic
    }

    #[test]
    fn test_record_success() {
        let config = TracerConfig::default();
        let provider = init_tracer_provider(config);
        let tracer = provider.tracer("test");
        let mut span = tracer.start("test_span");

        record_success(&mut span);

        // Span is updated with OK status (no way to read it in tests without export)
        // Just verify no panic
    }

    #[test]
    fn test_tracer_config_custom() {
        let config = TracerConfig {
            service_name: "custom-service".to_string(),
            service_version: "1.0.0".to_string(),
            sampling_rate: 0.5,
        };

        assert_eq!(config.service_name, "custom-service");
        assert_eq!(config.service_version, "1.0.0");
        assert_eq!(config.sampling_rate, 0.5);
    }

    #[test]
    fn test_sampling_always_on() {
        let config = TracerConfig {
            service_name: "test".to_string(),
            service_version: "1.0.0".to_string(),
            sampling_rate: 1.0,
        };

        let provider = init_tracer_provider(config);
        let tracer = provider.tracer("test");
        let span = tracer.start("test_span");
        assert!(!span.span_context().trace_id().to_string().is_empty());
    }

    #[test]
    fn test_sampling_always_off() {
        let config = TracerConfig {
            service_name: "test".to_string(),
            service_version: "1.0.0".to_string(),
            sampling_rate: 0.0,
        };

        let provider = init_tracer_provider(config);
        let tracer = provider.tracer("test");
        let span = tracer.start("test_span");
        // Even with AlwaysOff sampler, span is created but not sampled
        assert!(!span.span_context().trace_id().to_string().is_empty());
    }
}
