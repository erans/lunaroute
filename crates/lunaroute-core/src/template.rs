//! Template variable substitution engine
//!
//! Provides ${variable} style template substitution with support for:
//! - Runtime variables (request_id, session_id, provider, model, etc.)
//! - Environment variables (${env.VAR_NAME})
//! - Escape mechanism ($${variable} → ${variable})
//! - Safe handling of missing variables

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;

/// Regex for matching ${variable} or ${env.VAR_NAME} patterns
static TEMPLATE_VAR_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\$\{([a-zA-Z_][a-zA-Z0-9_]*(?:\.[a-zA-Z_][a-zA-Z0-9_]*)?)\}").unwrap()
});

/// Regex for matching escaped variables $${variable}
static ESCAPED_VAR_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"\$\$\{([^}]+)\}").unwrap());

/// Sensitive environment variable prefixes
const SENSITIVE_PREFIXES: &[&str] = &[
    "AWS_",
    "GITHUB_",
    "GITLAB_",
    "AZURE_",
    "GCP_",
    "DOCKER_",
    "NPM_",
    "PYPI_",
    "CARGO_",
    "OPENAI_",
    "ANTHROPIC_",
];

/// Sensitive environment variable patterns (suffixes and substrings)
const SENSITIVE_PATTERNS: &[&str] = &[
    "_KEY",
    "_SECRET",
    "_PASSWORD",
    "_TOKEN",
    "_CREDS",
    "_AUTH",
    "_PRIVATE",
    "_CERT",
    "_PEM",
    "_JWT",
    "_OAUTH",
    "_APIKEY",
];

/// Check if an environment variable name is potentially sensitive
fn is_sensitive_env_var(var_name: &str) -> bool {
    let upper = var_name.to_uppercase();

    // Check prefixes (e.g., AWS_, GITHUB_, etc.)
    if SENSITIVE_PREFIXES
        .iter()
        .any(|prefix| upper.starts_with(prefix))
    {
        return true;
    }

    // Check patterns (suffixes and substrings)
    if SENSITIVE_PATTERNS
        .iter()
        .any(|pattern| upper.ends_with(pattern) || upper.contains(pattern))
    {
        return true;
    }

    // Check exact matches
    matches!(
        upper.as_str(),
        "PASSWORD" | "SECRET" | "TOKEN" | "KEY" | "CREDENTIALS"
    )
}

/// Context for template variable substitution
///
/// # Thread Safety
///
/// This struct is NOT thread-safe and should not be shared across threads.
/// It maintains an internal cache of environment variables that is not protected
/// by synchronization primitives. Create a new `TemplateContext` instance for each
/// request or thread context.
///
/// The struct is `Clone`, but clones share no state - each clone gets its own
/// independent environment variable cache.
#[derive(Debug, Clone, Default)]
pub struct TemplateContext {
    /// Unique request ID
    pub request_id: String,
    /// Session ID (if session recording enabled)
    pub session_id: Option<String>,
    /// Provider name (openai, anthropic, etc.)
    pub provider: String,
    /// Model name from request
    pub model: String,
    /// ISO 8601 timestamp
    pub timestamp: String,
    /// Client IP (anonymized if PII enabled)
    pub client_ip: Option<String>,
    /// Client User-Agent
    pub user_agent: Option<String>,
    /// Whether response was served from cache
    pub cached: Option<bool>,
    /// Environment variables (loaded on demand)
    env_vars: HashMap<String, String>,
}

impl TemplateContext {
    /// Create a new template context with required fields
    pub fn new(request_id: String, provider: String, model: String) -> Self {
        Self {
            request_id,
            provider,
            model,
            timestamp: chrono::Utc::now().to_rfc3339(),
            ..Default::default()
        }
    }

    /// Set session ID
    pub fn with_session_id(mut self, session_id: String) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Set client IP
    pub fn with_client_ip(mut self, client_ip: String) -> Self {
        self.client_ip = Some(client_ip);
        self
    }

    /// Set user agent
    pub fn with_user_agent(mut self, user_agent: String) -> Self {
        self.user_agent = Some(user_agent);
        self
    }

    /// Set cached flag
    pub fn with_cached(mut self, cached: bool) -> Self {
        self.cached = Some(cached);
        self
    }

    /// Load an environment variable (with security filtering)
    fn get_env_var(&mut self, var_name: &str) -> Option<String> {
        // Security: reject sensitive variable names using improved filtering
        if is_sensitive_env_var(var_name) {
            tracing::warn!(
                "Rejecting access to potentially sensitive environment variable: {}",
                var_name
            );
            return None;
        }

        // Check cache first
        if let Some(value) = self.env_vars.get(var_name) {
            return Some(value.clone());
        }

        // Load from environment
        if let Ok(value) = std::env::var(var_name) {
            self.env_vars.insert(var_name.to_string(), value.clone());
            Some(value)
        } else {
            None
        }
    }

    /// Get a variable value by name
    fn get_variable(&mut self, var_name: &str) -> Option<String> {
        // Handle nested syntax: env.VAR_NAME
        if let Some(env_var) = var_name.strip_prefix("env.") {
            return self.get_env_var(env_var);
        }

        // Handle standard variables
        match var_name {
            "request_id" => Some(self.request_id.clone()),
            "session_id" => self.session_id.clone(),
            "provider" => Some(self.provider.clone()),
            "model" => Some(self.model.clone()),
            "timestamp" => Some(self.timestamp.clone()),
            "client_ip" => self.client_ip.clone(),
            "user_agent" => self.user_agent.clone(),
            "cached" => self.cached.map(|c| c.to_string()),
            _ => None,
        }
    }
}

/// Substitute template variables in a string
///
/// Replaces ${variable} with values from the context.
/// Escaped variables $${variable} become ${variable} (literal).
/// Missing variables are kept as-is and logged at debug level.
///
/// # Examples
///
/// ```ignore
/// let mut ctx = TemplateContext::new("req-123".to_string(), "openai".to_string(), "gpt-4".to_string());
/// let result = substitute_string("Request: ${request_id}", &mut ctx);
/// assert_eq!(result, "Request: req-123");
/// ```
pub fn substitute_string(template: &str, context: &mut TemplateContext) -> String {
    // First, handle escaped variables by temporarily replacing them
    let with_escapes_handled = ESCAPED_VAR_REGEX.replace_all(template, "<<<ESCAPED:$1>>>");

    // Perform variable substitution
    let result = TEMPLATE_VAR_REGEX.replace_all(&with_escapes_handled, |caps: &regex::Captures| {
        let var_name = &caps[1];
        context.get_variable(var_name).unwrap_or_else(|| {
            // Keep original if variable not found, but log for debugging
            tracing::debug!(
                "Template variable not found, keeping as-is: ${{{}}}",
                var_name
            );
            format!("${{{}}}", var_name)
        })
    });

    // Restore escaped variables as literals
    result.replace("<<<ESCAPED:", "${").replace(">>>", "}")
}

/// Substitute template variables in a JSON value
///
/// Recursively walks the JSON structure and substitutes strings.
pub fn substitute_value(value: &Value, context: &mut TemplateContext) -> Value {
    match value {
        Value::String(s) => Value::String(substitute_string(s, context)),
        Value::Array(arr) => {
            Value::Array(arr.iter().map(|v| substitute_value(v, context)).collect())
        }
        Value::Object(obj) => Value::Object(
            obj.iter()
                .map(|(k, v)| (k.clone(), substitute_value(v, context)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

/// Substitute template variables in a headers map
pub fn substitute_headers(
    headers: &HashMap<String, String>,
    context: &mut TemplateContext,
) -> HashMap<String, String> {
    headers
        .iter()
        .map(|(k, v)| (k.clone(), substitute_string(v, context)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_context() -> TemplateContext {
        TemplateContext {
            request_id: "req-123".to_string(),
            session_id: Some("sess-456".to_string()),
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            timestamp: "2025-10-04T00:00:00Z".to_string(),
            client_ip: Some("192.168.1.0".to_string()),
            user_agent: Some("test-agent".to_string()),
            cached: Some(true),
            env_vars: HashMap::new(),
        }
    }

    #[test]
    fn test_substitute_simple_variable() {
        let mut ctx = test_context();
        let result = substitute_string("Request: ${request_id}", &mut ctx);
        assert_eq!(result, "Request: req-123");
    }

    #[test]
    fn test_substitute_multiple_variables() {
        let mut ctx = test_context();
        let result = substitute_string("${provider}:${model} [${request_id}]", &mut ctx);
        assert_eq!(result, "openai:gpt-4 [req-123]");
    }

    #[test]
    fn test_substitute_optional_variable_present() {
        let mut ctx = test_context();
        let result = substitute_string("Session: ${session_id}", &mut ctx);
        assert_eq!(result, "Session: sess-456");
    }

    #[test]
    fn test_substitute_optional_variable_absent() {
        let mut ctx = TemplateContext::new(
            "req-1".to_string(),
            "test".to_string(),
            "model-1".to_string(),
        );
        let result = substitute_string("Session: ${session_id}", &mut ctx);
        assert_eq!(result, "Session: ${session_id}"); // Kept as-is
    }

    #[test]
    fn test_substitute_escaped_variable() {
        let mut ctx = test_context();
        let result = substitute_string("Literal: $${variable}", &mut ctx);
        assert_eq!(result, "Literal: ${variable}");
    }

    #[test]
    fn test_substitute_mixed_escaped_and_real() {
        let mut ctx = test_context();
        let result = substitute_string("Real: ${request_id}, Literal: $${not_real}", &mut ctx);
        assert_eq!(result, "Real: req-123, Literal: ${not_real}");
    }

    #[test]
    fn test_substitute_env_variable() {
        unsafe {
            std::env::set_var("TEST_VAR", "test-value");
        }
        let mut ctx = test_context();
        let result = substitute_string("Env: ${env.TEST_VAR}", &mut ctx);
        assert_eq!(result, "Env: test-value");
        unsafe {
            std::env::remove_var("TEST_VAR");
        }
    }

    #[test]
    fn test_substitute_env_variable_missing() {
        let mut ctx = test_context();
        let result = substitute_string("Env: ${env.NONEXISTENT}", &mut ctx);
        assert_eq!(result, "Env: ${env.NONEXISTENT}");
    }

    #[test]
    fn test_substitute_env_variable_cached() {
        unsafe {
            std::env::set_var("CACHE_TEST", "cached");
        }
        let mut ctx = test_context();

        // First call loads from env
        let result1 = substitute_string("${env.CACHE_TEST}", &mut ctx);
        assert_eq!(result1, "cached");

        // Change env var
        unsafe {
            std::env::set_var("CACHE_TEST", "different");
        }

        // Second call uses cache
        let result2 = substitute_string("${env.CACHE_TEST}", &mut ctx);
        assert_eq!(result2, "cached"); // Still uses cached value

        unsafe {
            std::env::remove_var("CACHE_TEST");
        }
    }

    #[test]
    fn test_substitute_rejects_sensitive_env_vars() {
        unsafe {
            std::env::set_var("API_KEY", "secret");
            std::env::set_var("MY_PASSWORD", "secret");
            std::env::set_var("AUTH_TOKEN", "secret");
        }

        let mut ctx = test_context();

        assert_eq!(
            substitute_string("${env.API_KEY}", &mut ctx),
            "${env.API_KEY}"
        );
        assert_eq!(
            substitute_string("${env.MY_PASSWORD}", &mut ctx),
            "${env.MY_PASSWORD}"
        );
        assert_eq!(
            substitute_string("${env.AUTH_TOKEN}", &mut ctx),
            "${env.AUTH_TOKEN}"
        );

        unsafe {
            std::env::remove_var("API_KEY");
            std::env::remove_var("MY_PASSWORD");
            std::env::remove_var("AUTH_TOKEN");
        }
    }

    #[test]
    fn test_substitute_unknown_variable() {
        let mut ctx = test_context();
        let result = substitute_string("Unknown: ${unknown_var}", &mut ctx);
        assert_eq!(result, "Unknown: ${unknown_var}");
    }

    #[test]
    fn test_substitute_empty_string() {
        let mut ctx = test_context();
        let result = substitute_string("", &mut ctx);
        assert_eq!(result, "");
    }

    #[test]
    fn test_substitute_no_variables() {
        let mut ctx = test_context();
        let result = substitute_string("Plain text", &mut ctx);
        assert_eq!(result, "Plain text");
    }

    #[test]
    fn test_substitute_value_string() {
        let mut ctx = test_context();
        let value = Value::String("Request: ${request_id}".to_string());
        let result = substitute_value(&value, &mut ctx);
        assert_eq!(result, Value::String("Request: req-123".to_string()));
    }

    #[test]
    fn test_substitute_value_array() {
        let mut ctx = test_context();
        let value = Value::Array(vec![
            Value::String("${provider}".to_string()),
            Value::String("${model}".to_string()),
        ]);
        let result = substitute_value(&value, &mut ctx);
        assert_eq!(
            result,
            Value::Array(vec![
                Value::String("openai".to_string()),
                Value::String("gpt-4".to_string()),
            ])
        );
    }

    #[test]
    fn test_substitute_value_object() {
        let mut ctx = test_context();
        let mut obj = serde_json::Map::new();
        obj.insert(
            "provider".to_string(),
            Value::String("${provider}".to_string()),
        );
        obj.insert(
            "request".to_string(),
            Value::String("${request_id}".to_string()),
        );

        let value = Value::Object(obj);
        let result = substitute_value(&value, &mut ctx);

        let result_obj = result.as_object().unwrap();
        assert_eq!(
            result_obj.get("provider").unwrap(),
            &Value::String("openai".to_string())
        );
        assert_eq!(
            result_obj.get("request").unwrap(),
            &Value::String("req-123".to_string())
        );
    }

    #[test]
    fn test_substitute_value_nested() {
        let mut ctx = test_context();
        let value = serde_json::json!({
            "metadata": {
                "provider": "${provider}",
                "request_id": "${request_id}",
                "items": ["${model}", "${timestamp}"]
            }
        });

        let result = substitute_value(&value, &mut ctx);

        assert_eq!(result["metadata"]["provider"], "openai");
        assert_eq!(result["metadata"]["request_id"], "req-123");
        assert_eq!(result["metadata"]["items"][0], "gpt-4");
        assert_eq!(result["metadata"]["items"][1], "2025-10-04T00:00:00Z");
    }

    #[test]
    fn test_substitute_value_non_string() {
        let mut ctx = test_context();

        // Numbers, booleans, null should pass through unchanged
        assert_eq!(
            substitute_value(&Value::Number(42.into()), &mut ctx),
            Value::Number(42.into())
        );
        assert_eq!(
            substitute_value(&Value::Bool(true), &mut ctx),
            Value::Bool(true)
        );
        assert_eq!(substitute_value(&Value::Null, &mut ctx), Value::Null);
    }

    #[test]
    fn test_substitute_headers() {
        let mut ctx = test_context();
        let mut headers = HashMap::new();
        headers.insert("X-Request-ID".to_string(), "${request_id}".to_string());
        headers.insert("X-Provider".to_string(), "${provider}".to_string());
        headers.insert("X-Static".to_string(), "static-value".to_string());

        let result = substitute_headers(&headers, &mut ctx);

        assert_eq!(result.get("X-Request-ID").unwrap(), "req-123");
        assert_eq!(result.get("X-Provider").unwrap(), "openai");
        assert_eq!(result.get("X-Static").unwrap(), "static-value");
    }

    #[test]
    fn test_context_builder() {
        let ctx = TemplateContext::new(
            "req-1".to_string(),
            "anthropic".to_string(),
            "claude-3".to_string(),
        )
        .with_session_id("sess-1".to_string())
        .with_client_ip("10.0.0.1".to_string())
        .with_user_agent("test/1.0".to_string())
        .with_cached(false);

        assert_eq!(ctx.request_id, "req-1");
        assert_eq!(ctx.provider, "anthropic");
        assert_eq!(ctx.model, "claude-3");
        assert_eq!(ctx.session_id, Some("sess-1".to_string()));
        assert_eq!(ctx.client_ip, Some("10.0.0.1".to_string()));
        assert_eq!(ctx.user_agent, Some("test/1.0".to_string()));
        assert_eq!(ctx.cached, Some(false));
    }

    #[test]
    fn test_boolean_variable_substitution() {
        let mut ctx = test_context();
        let result = substitute_string("Cached: ${cached}", &mut ctx);
        assert_eq!(result, "Cached: true");
    }

    #[test]
    fn test_special_characters_in_template() {
        let mut ctx = test_context();
        let result = substitute_string(
            "URL: https://api.com?id=${request_id}&model=${model}",
            &mut ctx,
        );
        assert_eq!(result, "URL: https://api.com?id=req-123&model=gpt-4");
    }
}
