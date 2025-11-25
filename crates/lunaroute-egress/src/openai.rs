//! OpenAI egress connector

use crate::{
    EgressError, Result,
    client::{HttpClientConfig, create_client, with_retry},
};
use async_trait::async_trait;
use futures::Stream;
use lunaroute_core::{
    normalized::{
        ContentPart, Delta, FinishReason, FunctionCall, FunctionCallDelta, Message, MessageContent,
        NormalizedRequest, NormalizedResponse, NormalizedStreamEvent, Role, ToolCall, ToolChoice,
        Usage,
    },
    provider::{Provider, ProviderCapabilities},
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use tracing::{debug, instrument, warn};

/// Codex authentication configuration
#[derive(Debug, Clone)]
pub struct CodexAuthConfig {
    /// Enable Codex authentication
    pub enabled: bool,

    /// Path to Codex auth file (default: ~/.codex/auth.json)
    pub auth_file: PathBuf,

    /// JSON field path for access token (default: "tokens.access_token")
    /// Supports nested paths using dot notation (e.g., "tokens.access_token")
    pub token_field: String,

    /// Optional account ID to send as chatgpt-account-id header
    /// If set, will override client's chatgpt-account-id header
    /// If not set, client's header will pass through unchanged
    pub account_id: Option<String>,
}

impl Default for CodexAuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            auth_file: PathBuf::from("~/.codex/auth.json"),
            token_field: "tokens.access_token".to_string(),
            account_id: None,
        }
    }
}

/// OpenAI connector configuration
#[derive(Debug, Clone)]
pub struct OpenAIConfig {
    /// API key for authentication
    pub api_key: String,

    /// Base URL for OpenAI API (default: https://api.openai.com/v1)
    pub base_url: String,

    /// Organization ID (optional)
    pub organization: Option<String>,

    /// HTTP client configuration
    pub client_config: HttpClientConfig,

    /// Custom request headers (supports template variables)
    pub custom_headers: Option<std::collections::HashMap<String, String>>,

    /// Request body modifications
    pub request_body_config: Option<RequestBodyModConfig>,

    /// Response body modifications
    pub response_body_config: Option<ResponseBodyModConfig>,

    /// Codex authentication configuration
    pub codex_auth: Option<CodexAuthConfig>,

    /// Optional custom notification message when this provider is used as alternative
    pub switch_notification_message: Option<String>,
}

/// Request body modification configuration
#[derive(Debug, Clone)]
pub struct RequestBodyModConfig {
    /// Fields to set only if missing
    pub defaults: Option<serde_json::Value>,
    /// Fields to always override
    pub overrides: Option<serde_json::Value>,
    /// Messages to prepend
    pub prepend_messages: Option<Vec<serde_json::Value>>,
}

/// Response body modification configuration
#[derive(Debug, Clone)]
pub struct ResponseBodyModConfig {
    /// Whether enabled
    pub enabled: bool,
    /// Namespace for metadata
    pub metadata_namespace: String,
    /// Metadata fields
    pub fields: Option<std::collections::HashMap<String, String>>,
    /// Extension fields (alternative)
    pub extension_fields: Option<std::collections::HashMap<String, String>>,
}

impl OpenAIConfig {
    /// Create a new OpenAI configuration
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: "https://api.openai.com/v1".to_string(),
            organization: None,
            client_config: HttpClientConfig::default(),
            custom_headers: None,
            request_body_config: None,
            response_body_config: None,
            codex_auth: None,
            switch_notification_message: None,
        }
    }

    /// Set the base URL (for custom endpoints)
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Set the organization ID
    pub fn with_organization(mut self, organization: impl Into<String>) -> Self {
        self.organization = Some(organization.into());
        self
    }
}

/// OpenAI connector
pub struct OpenAIConnector {
    config: OpenAIConfig,
    client: Client,
    codex_token_cache: Option<Arc<RwLock<Option<String>>>>,
}

impl OpenAIConnector {
    /// Create a new OpenAI connector
    ///
    /// If Codex authentication is enabled, reads and caches the access_token
    /// from auth.json at startup.
    pub async fn new(config: OpenAIConfig) -> Result<Self> {
        let client = create_client(&config.client_config)?;

        // Initialize token cache if Codex auth is enabled
        let codex_token_cache = if let Some(ref codex_auth) = config.codex_auth {
            if codex_auth.enabled {
                let cache = Arc::new(RwLock::new(None));

                // Read access_token from auth.json and cache it
                debug!("Codex auth enabled, reading access_token from auth.json");

                match crate::codex_auth::read_codex_token(
                    &codex_auth.auth_file,
                    "tokens.access_token",
                ) {
                    Ok(Some(access_token)) => {
                        debug!("Successfully read access_token from auth.json");
                        if let Ok(mut cache_guard) = cache.write() {
                            *cache_guard = Some(access_token);
                            debug!("Cached access_token for use in requests");
                        }
                    }
                    Ok(None) => {
                        debug!(
                            "No access_token found in auth.json, will use configured token_field: {}",
                            codex_auth.token_field
                        );
                        // Cache remains None, will read from token_field on first request
                    }
                    Err(e) => {
                        warn!(
                            "Failed to read access_token from auth.json: {}. Will use configured token_field: {}",
                            e, codex_auth.token_field
                        );
                        // Cache remains None, will fall back to token_field
                    }
                }

                Some(cache)
            } else {
                None
            }
        } else {
            None
        };

        Ok(Self {
            config,
            client,
            codex_token_cache,
        })
    }

    /// Get Codex authentication token (with caching)
    ///
    /// Reads token from configured Codex auth file and caches it in memory.
    /// Returns None if Codex auth is disabled or token cannot be read.
    fn get_codex_token(&self) -> Option<String> {
        // Check if Codex auth is enabled
        let codex_auth = self.config.codex_auth.as_ref()?;
        if !codex_auth.enabled {
            return None;
        }

        let cache = self.codex_token_cache.as_ref()?;

        // Try to read from cache first
        if let Ok(cache_guard) = cache.read()
            && let Some(ref token) = *cache_guard
        {
            debug!("Using cached Codex token");
            return Some(token.clone());
        }

        // Cache miss - read from file
        match crate::codex_auth::read_codex_token(&codex_auth.auth_file, &codex_auth.token_field) {
            Ok(Some(token)) => {
                // Cache the token
                if let Ok(mut cache_guard) = cache.write() {
                    *cache_guard = Some(token.clone());
                }
                debug!("Codex token read from file and cached");
                Some(token)
            }
            Ok(None) => {
                debug!("Codex auth file exists but no valid token found");
                None
            }
            Err(e) => {
                warn!("Failed to read Codex token: {}", e);
                None
            }
        }
    }

    /// Get Codex account ID to use for chatgpt-account-id header
    ///
    /// Priority order:
    /// 1. Configured account_id (if set explicitly)
    /// 2. Account ID from auth.json file (if available)
    ///
    /// Returns None if Codex auth is disabled or no account_id is available.
    fn get_codex_account_id(&self) -> Option<String> {
        // Check if Codex auth is enabled
        let codex_auth = self.config.codex_auth.as_ref()?;
        if !codex_auth.enabled {
            return None;
        }

        // 1. Check if account_id is explicitly configured
        if let Some(ref account_id) = codex_auth.account_id {
            debug!("Using configured account_id: {}", account_id);
            return Some(account_id.clone());
        }

        // 2. Try to read account_id from the auth file (tokens.account_id)
        match crate::codex_auth::read_codex_token(&codex_auth.auth_file, "tokens.account_id") {
            Ok(Some(account_id)) => {
                debug!(
                    "Successfully read account_id from auth.json: {}",
                    account_id
                );
                Some(account_id)
            }
            Ok(None) => {
                debug!("Codex auth file exists but no account_id found");
                None
            }
            Err(e) => {
                warn!("Failed to read Codex account_id: {}", e);
                None
            }
        }
    }

    /// Check if we should override client's chatgpt-account-id header
    /// Returns true if account_id is explicitly configured (overrides client header)
    /// Returns false otherwise (client header passes through)
    fn should_override_account_id(&self) -> bool {
        if let Some(ref codex_auth) = self.config.codex_auth {
            codex_auth.enabled && codex_auth.account_id.is_some()
        } else {
            false
        }
    }

    /// Get the authorization header value to use as fallback (when client doesn't provide one)
    ///
    /// Priority order:
    /// 1. Codex auth token (if enabled and available)
    /// 2. Configured API key (if not empty)
    /// 3. None (no fallback available)
    fn get_fallback_auth_header(&self) -> Option<String> {
        // 1. Try Codex auth first
        if let Some(token) = self.get_codex_token() {
            debug!("Using Codex authentication as fallback");
            return Some(format!("Bearer {}", token));
        }

        // 2. Fall back to configured API key
        if !self.config.api_key.is_empty() {
            debug!("Using configured API key as fallback");
            return Some(format!("Bearer {}", self.config.api_key));
        }

        // 3. No fallback auth available
        debug!("No fallback auth available");
        None
    }

    /// Check if proxy has a configured API key that should override client auth
    /// (Note: Codex auth is a fallback, not an override - client auth takes precedence)
    fn has_override_auth(&self) -> bool {
        !self.config.api_key.is_empty()
    }

    /// Send a raw JSON request directly to OpenAI (passthrough mode)
    /// This skips normalization for OpenAI→OpenAI routing, preserving 100% API fidelity.
    /// Still parses the response to extract metrics (tokens, model, etc.)
    #[instrument(skip(self, request_json))]
    pub async fn send_passthrough(
        &self,
        request_json: serde_json::Value,
        headers: std::collections::HashMap<String, String>,
    ) -> Result<(serde_json::Value, std::collections::HashMap<String, String>)> {
        self.send_passthrough_to_endpoint("chat/completions", request_json, headers)
            .await
    }

    /// Send with context for custom headers and body modifications
    #[instrument(skip(self, request_json))]
    pub async fn send_passthrough_with_context(
        &self,
        mut request_json: serde_json::Value,
        mut headers: std::collections::HashMap<String, String>,
        request_id: &str,
        session_id: Option<&str>,
    ) -> Result<(serde_json::Value, std::collections::HashMap<String, String>)> {
        // Extract model from request for context (clone to own it)
        let model = request_json
            .get("model")
            .and_then(|m| m.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unknown".to_string());

        // Apply request body modifications
        self.apply_request_body_modifications(&mut request_json);

        // Apply custom headers with template substitution
        self.apply_custom_headers(&mut headers, request_id, &model, session_id);

        // Send the request
        let (mut response_json, response_headers) = self
            .send_passthrough_to_endpoint("chat/completions", request_json, headers)
            .await?;

        // Apply response body modifications
        self.apply_response_body_modifications(&mut response_json, request_id, &model, session_id);

        Ok((response_json, response_headers))
    }

    /// Send a raw JSON request to a specific OpenAI endpoint (passthrough mode)
    #[instrument(skip(self, request_json))]
    pub async fn send_passthrough_to_endpoint(
        &self,
        endpoint: &str,
        request_json: serde_json::Value,
        headers: std::collections::HashMap<String, String>,
    ) -> Result<(serde_json::Value, std::collections::HashMap<String, String>)> {
        debug!(
            "Sending passthrough request to OpenAI {} endpoint (JSON mode)",
            endpoint
        );

        let max_retries = self.config.client_config.max_retries;
        let result = with_retry(max_retries, || {
            let request_json = request_json.clone();
            let headers = headers.clone();
            let config = self.config.clone();
            let endpoint = endpoint.to_string();
            async move {
                let mut request_builder = self
                    .client
                    .post(format!("{}/{}", config.base_url, endpoint));

                // Check if we have configured API key that should override client auth
                let has_override_auth = self.has_override_auth();
                let should_override_account_id = self.should_override_account_id();
                let mut client_provided_auth = false;

                // Forward headers from client, skipping only if configured API key should override
                for (name, value) in &headers {
                    let name_lower = name.to_lowercase();

                    // Track if client provided authorization
                    if name_lower == "authorization" {
                        client_provided_auth = true;
                        if has_override_auth {
                            debug!("Skipping client Authorization header (configured API key will override)");
                            continue;
                        }
                    }

                    // Skip client's chatgpt-account-id header if we have configured account_id
                    if should_override_account_id && name_lower == "chatgpt-account-id" {
                        debug!("Skipping client chatgpt-account-id header (configured account_id will override)");
                        continue;
                    }

                    request_builder = request_builder.header(name, value);
                }

                // Use fallback auth only if client didn't provide auth
                if !client_provided_auth
                    && let Some(auth_header) = self.get_fallback_auth_header() {
                        request_builder = request_builder.header("Authorization", auth_header);
                    }

                // Add chatgpt-account-id header if configured or available from auth.json
                if let Some(account_id) = self.get_codex_account_id() {
                    debug!("Adding chatgpt-account-id header: {}", account_id);
                    request_builder = request_builder.header("chatgpt-account-id", account_id);
                }

                // Send raw JSON body without .json() to avoid modifying headers
                let json_string = serde_json::to_string(&request_json)?;

                let response = request_builder.body(json_string).send().await?;

                debug!("┌─────────────────────────────────────────────────────────");
                debug!("│ OpenAI Passthrough Response Headers");
                debug!("├─────────────────────────────────────────────────────────");
                debug!("│ Status: {}", response.status());
                for (name, value) in response.headers() {
                    if let Ok(val_str) = value.to_str() {
                        debug!("│ {}: {}", name, val_str);
                    }
                }
                debug!("└─────────────────────────────────────────────────────────");

                self.handle_openai_passthrough_response(response).await
            }
        })
        .await?;

        Ok(result)
    }

    /// Send raw bytes to a specific OpenAI endpoint (true passthrough mode - no parsing/re-serializing)
    #[instrument(skip(self, body))]
    pub async fn send_passthrough_to_endpoint_bytes(
        &self,
        endpoint: &str,
        body: bytes::Bytes,
        headers: std::collections::HashMap<String, String>,
    ) -> Result<(serde_json::Value, std::collections::HashMap<String, String>)> {
        debug!(
            "Sending passthrough request to OpenAI {} endpoint (true passthrough - raw bytes)",
            endpoint
        );

        let max_retries = self.config.client_config.max_retries;
        let result = with_retry(max_retries, || {
            let body = body.clone();
            let headers = headers.clone();
            let config = self.config.clone();
            let endpoint = endpoint.to_string();
            async move {
                let mut request_builder = self
                    .client
                    .post(format!("{}/{}", config.base_url, endpoint));

                // Check if we have configured API key that should override client auth
                let has_override_auth = self.has_override_auth();
                let should_override_account_id = self.should_override_account_id();
                let mut client_provided_auth = false;

                // Forward headers from client, skipping only if configured API key should override
                for (name, value) in &headers {
                    let name_lower = name.to_lowercase();

                    // Track if client provided authorization
                    if name_lower == "authorization" {
                        client_provided_auth = true;
                        if has_override_auth {
                            debug!("Skipping client Authorization header (configured API key will override)");
                            continue;
                        }
                    }

                    // Skip client's chatgpt-account-id header if we have configured account_id
                    if should_override_account_id && name_lower == "chatgpt-account-id" {
                        debug!("Skipping client chatgpt-account-id header (configured account_id will override)");
                        continue;
                    }

                    request_builder = request_builder.header(name, value);
                }

                // Use fallback auth only if client didn't provide auth
                if !client_provided_auth
                    && let Some(auth_header) = self.get_fallback_auth_header() {
                        request_builder = request_builder.header("Authorization", auth_header);
                    }

                // Add chatgpt-account-id header if configured or available from auth.json
                if let Some(account_id) = self.get_codex_account_id() {
                    debug!("Adding chatgpt-account-id header: {}", account_id);
                    request_builder = request_builder.header("chatgpt-account-id", account_id);
                }

                // In passthrough mode, do NOT apply organization header - use only client headers
                // request_builder = request_builder.apply_organization_header(&config);

                // Send raw body bytes without any parsing/re-serialization
                let response = request_builder.body(body).send().await?;

                // Log response headers at debug level
                debug!("┌─────────────────────────────────────────────────────────");
                debug!("│ OpenAI Passthrough Response Headers");
                debug!("├─────────────────────────────────────────────────────────");
                debug!("│ Status: {}", response.status());
                for (name, value) in response.headers() {
                    if let Ok(val_str) = value.to_str() {
                        debug!("│ {}: {}", name, val_str);
                    }
                }
                debug!("└─────────────────────────────────────────────────────────");

                self.handle_openai_passthrough_response(response).await
            }
        })
        .await?;

        Ok(result)
    }

    /// Handle passthrough response (for send_passthrough)
    /// Returns (json_body, headers_map)
    async fn handle_openai_passthrough_response(
        &self,
        response: reqwest::Response,
    ) -> Result<(serde_json::Value, std::collections::HashMap<String, String>)> {
        let status = response.status();

        // Capture retry-after header before consuming response
        let retry_after_secs = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(crate::parse_retry_after);

        if !status.is_success() {
            let status_code = status.as_u16();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());

            return Err(if status_code == 429 {
                debug!(
                    retry_after_secs = ?retry_after_secs,
                    "OpenAI rate limit exceeded"
                );
                EgressError::RateLimitExceeded { retry_after_secs }
            } else {
                EgressError::ProviderError {
                    status_code,
                    message: body,
                }
            });
        }

        // Capture response headers before consuming the response
        let mut headers_map = std::collections::HashMap::new();
        for (name, value) in response.headers() {
            if let Ok(value_str) = value.to_str() {
                headers_map.insert(name.to_string(), value_str.to_string());
            }
        }

        let json_body = response.json::<serde_json::Value>().await.map_err(|e| {
            EgressError::ParseError(format!(
                "Failed to parse OpenAI passthrough response: {}",
                e
            ))
        })?;

        Ok((json_body, headers_map))
    }

    /// Stream raw OpenAI request (passthrough mode - no normalization)
    /// Returns raw response for direct SSE forwarding
    #[instrument(skip(self, request_json, headers))]
    pub async fn stream_passthrough(
        &self,
        request_json: serde_json::Value,
        headers: std::collections::HashMap<String, String>,
    ) -> Result<reqwest::Response> {
        self.stream_passthrough_to_endpoint("chat/completions", request_json, headers)
            .await
    }

    /// Stream a raw JSON request to a specific OpenAI endpoint (passthrough mode)
    pub async fn stream_passthrough_to_endpoint(
        &self,
        endpoint: &str,
        request_json: serde_json::Value,
        headers: std::collections::HashMap<String, String>,
    ) -> Result<reqwest::Response> {
        debug!(
            "Sending passthrough streaming request to OpenAI {} endpoint (JSON mode)",
            endpoint
        );

        let mut request_builder = self
            .client
            .post(format!("{}/{}", self.config.base_url, endpoint));

        // Check if we have configured API key that should override client auth
        let has_override_auth = self.has_override_auth();
        let should_override_account_id = self.should_override_account_id();
        let mut client_provided_auth = false;

        // Forward headers from client, skipping only if configured API key should override
        for (name, value) in &headers {
            let name_lower = name.to_lowercase();

            // Track if client provided authorization
            if name_lower == "authorization" {
                client_provided_auth = true;
                if has_override_auth {
                    debug!(
                        "Skipping client Authorization header (configured API key will override)"
                    );
                    continue;
                }
            }

            // Skip client's chatgpt-account-id header if we have configured account_id
            if should_override_account_id && name_lower == "chatgpt-account-id" {
                debug!(
                    "Skipping client chatgpt-account-id header (configured account_id will override)"
                );
                continue;
            }

            request_builder = request_builder.header(name, value);
        }

        // Use fallback auth only if client didn't provide auth
        if !client_provided_auth && let Some(auth_header) = self.get_fallback_auth_header() {
            request_builder = request_builder.header("Authorization", auth_header);
        }

        // Add chatgpt-account-id header if configured or available from auth.json
        if let Some(account_id) = self.get_codex_account_id() {
            debug!("Adding chatgpt-account-id header: {}", account_id);
            request_builder = request_builder.header("chatgpt-account-id", account_id);
        }

        // Send raw JSON body without .json() to avoid modifying headers
        let json_string = serde_json::to_string(&request_json)?;

        let response = request_builder
            .body(json_string)
            .send()
            .await
            .map_err(EgressError::from)?;

        debug!("┌─────────────────────────────────────────────────────────");
        debug!("│ OpenAI Streaming Passthrough Response Headers");
        debug!("├─────────────────────────────────────────────────────────");
        debug!("│ Status: {}", response.status());
        for (name, value) in response.headers() {
            if let Ok(val_str) = value.to_str() {
                debug!("│ {}: {}", name, val_str);
            }
        }
        debug!("└─────────────────────────────────────────────────────────");

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());
            return Err(EgressError::ProviderError {
                status_code: status,
                message: body,
            });
        }

        Ok(response)
    }

    /// Send a GET request to a specific OpenAI endpoint (passthrough mode)
    pub async fn get_passthrough(
        &self,
        endpoint: &str,
        headers: std::collections::HashMap<String, String>,
    ) -> Result<serde_json::Value> {
        debug!(
            "Sending GET passthrough request to OpenAI {} endpoint",
            endpoint
        );

        let mut request_builder = self
            .client
            .get(format!("{}/{}", self.config.base_url, endpoint));

        // Check if we have configured API key that should override client auth
        let has_override_auth = self.has_override_auth();
        let should_override_account_id = self.should_override_account_id();
        let mut client_provided_auth = false;

        // Forward headers from client, skipping only if configured API key should override
        for (name, value) in &headers {
            let name_lower = name.to_lowercase();

            // Track if client provided authorization
            if name_lower == "authorization" {
                client_provided_auth = true;
                if has_override_auth {
                    debug!(
                        "Skipping client Authorization header (configured API key will override)"
                    );
                    continue;
                }
            }

            // Skip client's chatgpt-account-id header if we have configured account_id
            if should_override_account_id && name_lower == "chatgpt-account-id" {
                debug!(
                    "Skipping client chatgpt-account-id header (configured account_id will override)"
                );
                continue;
            }

            request_builder = request_builder.header(name, value);
        }

        // Use fallback auth only if client didn't provide auth
        if !client_provided_auth && let Some(auth_header) = self.get_fallback_auth_header() {
            request_builder = request_builder.header("Authorization", auth_header);
        }

        // Add chatgpt-account-id header if configured or available from auth.json
        if let Some(account_id) = self.get_codex_account_id() {
            debug!("Adding chatgpt-account-id header: {}", account_id);
            request_builder = request_builder.header("chatgpt-account-id", account_id);
        }

        let response = request_builder.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(EgressError::ProviderError {
                status_code: status.as_u16(),
                message: error_text,
            });
        }

        let json = response
            .json()
            .await
            .map_err(|e| EgressError::ParseError(e.to_string()))?;

        Ok(json)
    }

    /// Stream raw bytes to a specific OpenAI endpoint (true passthrough mode - no parsing/re-serializing)
    pub async fn stream_passthrough_to_endpoint_bytes(
        &self,
        endpoint: &str,
        body: bytes::Bytes,
        headers: std::collections::HashMap<String, String>,
    ) -> Result<reqwest::Response> {
        debug!(
            "Sending passthrough streaming request to OpenAI {} endpoint (true passthrough - raw bytes)",
            endpoint
        );

        let mut request_builder = self
            .client
            .post(format!("{}/{}", self.config.base_url, endpoint));

        // Check if we have configured API key that should override client auth
        let has_override_auth = self.has_override_auth();
        let should_override_account_id = self.should_override_account_id();
        let mut client_provided_auth = false;

        // Forward headers from client, skipping only if configured API key should override
        for (name, value) in &headers {
            let name_lower = name.to_lowercase();

            // Track if client provided authorization
            if name_lower == "authorization" {
                client_provided_auth = true;
                if has_override_auth {
                    debug!(
                        "Skipping client Authorization header (configured API key will override): {} chars",
                        value.len()
                    );
                    continue;
                }
            }

            // Skip client's chatgpt-account-id header if we have configured account_id
            if should_override_account_id && name_lower == "chatgpt-account-id" {
                debug!(
                    "Skipping client chatgpt-account-id header (configured account_id will override)"
                );
                continue;
            }

            request_builder = request_builder.header(name, value);
        }

        // Use fallback auth only if client didn't provide auth
        if !client_provided_auth && let Some(auth_header) = self.get_fallback_auth_header() {
            debug!("Using fallback authentication for OpenAI");
            request_builder = request_builder.header("Authorization", auth_header);
        }

        // Add chatgpt-account-id header if configured or available from auth.json
        if let Some(account_id) = self.get_codex_account_id() {
            debug!("Adding chatgpt-account-id header: {}", account_id);
            request_builder = request_builder.header("chatgpt-account-id", account_id);
        }

        debug!("=== ALL HEADERS BEING SENT TO OPENAI ===");
        // Show actual headers being sent (after filtering)
        for (name, _value) in request_builder
            .try_clone()
            .unwrap()
            .build()
            .unwrap()
            .headers()
        {
            if name.as_str().to_lowercase() == "authorization" {
                debug!("  authorization: [REDACTED]");
            } else if let Some(value) = headers.get(name.as_str()) {
                debug!("  {}: {}", name, value);
            }
        }

        // In passthrough mode, do NOT apply organization header - use only client headers
        // request_builder = request_builder.apply_organization_header(&self.config);

        // Send raw body bytes without any parsing/re-serialization
        let response = request_builder
            .body(body)
            .send()
            .await
            .map_err(EgressError::from)?;

        // Log response headers at debug level
        debug!("┌─────────────────────────────────────────────────────────");
        debug!("│ OpenAI Streaming Passthrough Response Headers");
        debug!("├─────────────────────────────────────────────────────────");
        debug!("│ Status: {}", response.status());
        for (name, value) in response.headers() {
            if let Ok(val_str) = value.to_str() {
                debug!("│ {}: {}", name, val_str);
            }
        }
        debug!("└─────────────────────────────────────────────────────────");

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());
            return Err(EgressError::ProviderError {
                status_code: status,
                message: body,
            });
        }

        Ok(response)
    }

    /// Apply custom headers with template variable substitution
    fn apply_custom_headers(
        &self,
        headers: &mut std::collections::HashMap<String, String>,
        request_id: &str,
        model: &str,
        session_id: Option<&str>,
    ) {
        if let Some(custom_headers) = &self.config.custom_headers {
            // Create template context
            let mut context = lunaroute_core::template::TemplateContext::new(
                request_id.to_string(),
                "openai".to_string(),
                model.to_string(),
            );
            if let Some(sid) = session_id {
                context = context.with_session_id(sid.to_string());
            }

            // Substitute and merge custom headers
            let substituted =
                lunaroute_core::template::substitute_headers(custom_headers, &mut context);
            headers.extend(substituted);
        }
    }

    /// Deep merge two JSON values (used for defaults/overrides)
    fn deep_merge(base: &mut serde_json::Value, overlay: &serde_json::Value) {
        if let (Some(base_obj), Some(overlay_obj)) = (base.as_object_mut(), overlay.as_object()) {
            for (key, value) in overlay_obj {
                if let Some(base_value) = base_obj.get_mut(key) {
                    if base_value.is_object() && value.is_object() {
                        Self::deep_merge(base_value, value);
                    } else {
                        *base_value = value.clone();
                    }
                } else {
                    base_obj.insert(key.clone(), value.clone());
                }
            }
        } else {
            *base = overlay.clone();
        }
    }

    /// Apply request body modifications (defaults, overrides, prepend)
    fn apply_request_body_modifications(&self, request_json: &mut serde_json::Value) {
        if let Some(body_config) = &self.config.request_body_config {
            // Apply defaults (only if field missing)
            if let Some(defaults) = &body_config.defaults {
                let mut base = defaults.clone();
                Self::deep_merge(&mut base, request_json);
                *request_json = base;
            }

            // Apply overrides (always replace)
            if let Some(overrides) = &body_config.overrides {
                Self::deep_merge(request_json, overrides);
            }

            // Prepend messages to messages array
            if let Some(prepend_msgs) = &body_config.prepend_messages
                && let Some(messages) = request_json.get_mut("messages")
                && let Some(messages_array) = messages.as_array_mut()
            {
                // Prepend in reverse order to maintain order
                for msg in prepend_msgs.iter().rev() {
                    messages_array.insert(0, msg.clone());
                }
            }
        }
    }

    /// Apply response body modifications (metadata injection)
    fn apply_response_body_modifications(
        &self,
        response_json: &mut serde_json::Value,
        request_id: &str,
        model: &str,
        session_id: Option<&str>,
    ) {
        if let Some(body_config) = &self.config.response_body_config {
            if !body_config.enabled {
                return;
            }

            let mut context = lunaroute_core::template::TemplateContext::new(
                request_id.to_string(),
                "openai".to_string(),
                model.to_string(),
            );
            if let Some(sid) = session_id {
                context = context.with_session_id(sid.to_string());
            }

            // Inject metadata object
            if let Some(fields) = &body_config.fields
                && let Some(response_obj) = response_json.as_object_mut()
            {
                let mut metadata = serde_json::Map::new();
                for (key, template) in fields {
                    let value = lunaroute_core::template::substitute_string(template, &mut context);
                    metadata.insert(key.clone(), serde_json::Value::String(value));
                }
                response_obj.insert(
                    body_config.metadata_namespace.clone(),
                    serde_json::Value::Object(metadata),
                );
            }

            // Alternative: inject extension fields at top level
            if let Some(ext_fields) = &body_config.extension_fields
                && let Some(response_obj) = response_json.as_object_mut()
            {
                for (key, template) in ext_fields {
                    let value = lunaroute_core::template::substitute_string(template, &mut context);
                    response_obj.insert(key.clone(), serde_json::Value::String(value));
                }
            }
        }
    }
}

#[async_trait]
impl Provider for OpenAIConnector {
    #[instrument(skip(self, request), fields(model = %request.model))]
    async fn send(&self, request: NormalizedRequest) -> lunaroute_core::Result<NormalizedResponse> {
        debug!("Sending non-streaming request to OpenAI");

        let openai_req = to_openai_request(request)?;

        // Check if we need custom headers or body modifications
        let needs_modifications =
            self.config.custom_headers.is_some() || self.config.request_body_config.is_some();

        if needs_modifications {
            // Path with modifications: serialize to JSON, modify, then send
            let mut request_json = serde_json::to_value(&openai_req).map_err(|e| {
                EgressError::ParseError(format!("Failed to serialize request: {}", e))
            })?;

            // Apply request body modifications (defaults, overrides, prepend messages)
            self.apply_request_body_modifications(&mut request_json);

            // Prepare headers with template substitution if custom headers are configured
            let mut headers_to_apply = std::collections::HashMap::new();
            if let Some(ref custom_headers) = self.config.custom_headers {
                // Create template context for header substitution
                use lunaroute_core::template::TemplateContext;
                let request_id = uuid::Uuid::new_v4().to_string();
                let model = openai_req.model.clone();

                let mut template_ctx =
                    TemplateContext::new(request_id, "openai".to_string(), model);

                // Substitute templates in headers
                headers_to_apply =
                    lunaroute_core::template::substitute_headers(custom_headers, &mut template_ctx);
            }

            // Log request headers at debug level
            debug!("┌─────────────────────────────────────────────────────────");
            debug!("│ OpenAI Request Headers");
            debug!("├─────────────────────────────────────────────────────────");
            debug!("│ Authorization: Bearer <api_key>");
            debug!("│ Content-Type: application/json");
            if let Some(ref org) = self.config.organization {
                debug!("│ OpenAI-Organization: {}", org);
            }

            // Log custom headers if present
            for (name, value) in &headers_to_apply {
                debug!("│ {}: {}", name, value);
            }
            debug!("└─────────────────────────────────────────────────────────");

            let max_retries = self.config.client_config.max_retries;
            let result = with_retry(max_retries, || {
                let request_json = request_json.clone();
                let headers_to_apply = headers_to_apply.clone();
                async move {
                    let mut request_builder = self
                        .client
                        .post(format!("{}/chat/completions", self.config.base_url))
                        .header("Content-Type", "application/json")
                        .apply_organization_header(&self.config);

                    // Apply fallback authentication (Codex auth → Configured API key)
                    if let Some(auth_header) = self.get_fallback_auth_header() {
                        request_builder = request_builder.header("Authorization", auth_header);
                    }

                    // Apply custom headers with templates already substituted
                    for (name, value) in headers_to_apply {
                        request_builder = request_builder.header(name, value);
                    }

                    let response = request_builder.json(&request_json).send().await?;

                    // Log response headers at debug level
                    debug!("┌─────────────────────────────────────────────────────────");
                    debug!("│ OpenAI Response Headers");
                    debug!("├─────────────────────────────────────────────────────────");
                    debug!("│ Status: {}", response.status());
                    for (name, value) in response.headers() {
                        if let Ok(val_str) = value.to_str() {
                            debug!("│ {}: {}", name, val_str);
                        }
                    }
                    debug!("└─────────────────────────────────────────────────────────");

                    response.handle_openai_response().await
                }
            })
            .await?;

            let normalized = from_openai_response(result)?;
            Ok(normalized)
        } else {
            // Legacy path without modifications: use original struct-based approach
            debug!("┌─────────────────────────────────────────────────────────");
            debug!("│ OpenAI Request Headers");
            debug!("├─────────────────────────────────────────────────────────");
            debug!("│ Authorization: Bearer <api_key>");
            debug!("│ Content-Type: application/json");
            if let Some(ref org) = self.config.organization {
                debug!("│ OpenAI-Organization: {}", org);
            }
            debug!("└─────────────────────────────────────────────────────────");

            let max_retries = self.config.client_config.max_retries;
            let result = with_retry(max_retries, || {
                let openai_req = openai_req.clone();
                async move {
                    let mut request_builder = self
                        .client
                        .post(format!("{}/chat/completions", self.config.base_url))
                        .header("Content-Type", "application/json")
                        .apply_organization_header(&self.config);

                    // Apply fallback authentication (Codex auth → Configured API key)
                    if let Some(auth_header) = self.get_fallback_auth_header() {
                        request_builder = request_builder.header("Authorization", auth_header);
                    }

                    let response = request_builder.json(&openai_req).send().await?;

                    debug!("┌─────────────────────────────────────────────────────────");
                    debug!("│ OpenAI Response Headers");
                    debug!("├─────────────────────────────────────────────────────────");
                    debug!("│ Status: {}", response.status());
                    for (name, value) in response.headers() {
                        if let Ok(val_str) = value.to_str() {
                            debug!("│ {}: {}", name, val_str);
                        }
                    }
                    debug!("└─────────────────────────────────────────────────────────");

                    response.handle_openai_response().await
                }
            })
            .await?;

            let normalized = from_openai_response(result)?;
            Ok(normalized)
        }
    }

    async fn stream(
        &self,
        request: NormalizedRequest,
    ) -> lunaroute_core::Result<
        Box<dyn Stream<Item = lunaroute_core::Result<NormalizedStreamEvent>> + Send + Unpin>,
    > {
        debug!("Sending streaming request to OpenAI");

        let mut openai_req = to_openai_request(request)?;
        openai_req.stream = Some(true);

        // Convert to JSON for body modifications
        let mut request_json = serde_json::to_value(&openai_req)
            .map_err(|e| EgressError::ParseError(format!("Failed to serialize request: {}", e)))?;

        // Apply request body modifications (defaults, overrides, prepend messages)
        self.apply_request_body_modifications(&mut request_json);

        // Prepare headers with template substitution if custom headers are configured
        let mut headers_to_apply = std::collections::HashMap::new();
        if let Some(ref custom_headers) = self.config.custom_headers {
            // Create template context for header substitution
            use lunaroute_core::template::TemplateContext;
            let request_id = uuid::Uuid::new_v4().to_string();
            let model = openai_req.model.clone();

            let mut template_ctx = TemplateContext::new(request_id, "openai".to_string(), model);

            // Substitute templates in headers
            headers_to_apply =
                lunaroute_core::template::substitute_headers(custom_headers, &mut template_ctx);
        }

        // Log request headers at debug level
        debug!("┌─────────────────────────────────────────────────────────");
        debug!("│ OpenAI Streaming Request Headers");
        debug!("├─────────────────────────────────────────────────────────");
        debug!("│ Authorization: Bearer <api_key>");
        debug!("│ Content-Type: application/json");
        if let Some(ref org) = self.config.organization {
            debug!("│ OpenAI-Organization: {}", org);
        }

        // Log custom headers with templates substituted
        for (name, value) in &headers_to_apply {
            debug!("│ {}: {}", name, value);
        }
        debug!("└─────────────────────────────────────────────────────────");

        let mut request_builder = self
            .client
            .post(format!("{}/chat/completions", self.config.base_url))
            .header("Content-Type", "application/json")
            .apply_organization_header(&self.config);

        // Apply fallback authentication (Codex auth → Configured API key)
        if let Some(auth_header) = self.get_fallback_auth_header() {
            request_builder = request_builder.header("Authorization", auth_header);
        }

        // Apply custom headers with templates already substituted
        for (name, value) in headers_to_apply {
            request_builder = request_builder.header(name, value);
        }

        let response = request_builder
            .json(&request_json)
            .send()
            .await
            .map_err(EgressError::from)?;

        // Log response headers at debug level
        debug!("┌─────────────────────────────────────────────────────────");
        debug!("│ OpenAI Streaming Response Headers");
        debug!("├─────────────────────────────────────────────────────────");
        debug!("│ Status: {}", response.status());
        for (name, value) in response.headers() {
            if let Ok(val_str) = value.to_str() {
                debug!("│ {}: {}", name, val_str);
            }
        }
        debug!("└─────────────────────────────────────────────────────────");

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());
            return Err(EgressError::ProviderError {
                status_code: status,
                message: body,
            }
            .into());
        }

        let stream = create_openai_stream(response);
        Ok(Box::new(stream))
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_vision: true,
        }
    }

    fn get_notification_message(&self) -> Option<&str> {
        self.config.switch_notification_message.as_deref()
    }
}

// OpenAI API types (simplified, matching ingress types)

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIChatRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    /// GPT-5 models use max_completion_tokens instead of max_tokens
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAITool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<OpenAIToolChoice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAITool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OpenAIFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIFunction {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum OpenAIToolChoice {
    String(String),
    Object {
        r#type: String,
        function: OpenAIFunctionName,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIFunctionName {
    name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIToolCall {
    id: String,
    #[serde(rename = "type")]
    tool_type: String,
    function: OpenAIFunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIChatResponse {
    id: String,
    model: String,
    choices: Vec<OpenAIChoice>,
    usage: OpenAIUsage,
    created: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIChoice {
    index: u32,
    message: OpenAIMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIStreamChunk {
    id: String,
    choices: Vec<OpenAIStreamChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<OpenAIUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIStreamChoice {
    index: u32,
    delta: OpenAIDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCallDelta>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIToolCallDelta {
    index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function: Option<OpenAIFunctionCallDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIFunctionCallDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    arguments: Option<String>,
}

// Conversion functions

fn to_openai_request(req: NormalizedRequest) -> Result<OpenAIChatRequest> {
    let messages = req
        .messages
        .into_iter()
        .map(|msg| {
            let role = match msg.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            }
            .to_string();

            let content = match msg.content {
                MessageContent::Text(text) => {
                    if text.is_empty() && !msg.tool_calls.is_empty() {
                        None // OpenAI allows null content for tool call messages
                    } else {
                        Some(text)
                    }
                }
                MessageContent::Parts(parts) => {
                    // Extract text from parts
                    let text: String = parts
                        .into_iter()
                        .filter_map(|part| match part {
                            ContentPart::Text { text } => Some(text),
                            ContentPart::Image { .. } => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");

                    if text.is_empty() { None } else { Some(text) }
                }
            };

            let tool_calls = if msg.tool_calls.is_empty() {
                None
            } else {
                Some(
                    msg.tool_calls
                        .into_iter()
                        .map(|tc| OpenAIToolCall {
                            id: tc.id,
                            tool_type: tc.tool_type,
                            function: OpenAIFunctionCall {
                                name: tc.function.name,
                                arguments: tc.function.arguments,
                            },
                        })
                        .collect(),
                )
            };

            OpenAIMessage {
                role,
                content,
                name: msg.name,
                tool_calls,
                tool_call_id: msg.tool_call_id,
            }
        })
        .collect();

    let tools = if req.tools.is_empty() {
        None
    } else {
        Some(
            req.tools
                .into_iter()
                .map(|t| OpenAITool {
                    tool_type: t.tool_type,
                    function: OpenAIFunction {
                        name: t.function.name,
                        description: t.function.description,
                        parameters: t.function.parameters,
                    },
                })
                .collect(),
        )
    };

    let tool_choice = req.tool_choice.map(|tc| match tc {
        ToolChoice::Auto => OpenAIToolChoice::String("auto".to_string()),
        ToolChoice::Required => OpenAIToolChoice::String("required".to_string()),
        ToolChoice::None => OpenAIToolChoice::String("none".to_string()),
        ToolChoice::Specific { name } => OpenAIToolChoice::Object {
            r#type: "function".to_string(),
            function: OpenAIFunctionName { name },
        },
    });

    // GPT-5 models use max_completion_tokens instead of max_tokens
    let is_gpt5 = req.model.starts_with("gpt-5")
        || req.model.starts_with("o1")
        || req.model.starts_with("o3");
    let (max_tokens, max_completion_tokens) = if is_gpt5 {
        (None, req.max_tokens)
    } else {
        (req.max_tokens, None)
    };

    Ok(OpenAIChatRequest {
        model: req.model,
        messages,
        temperature: req.temperature,
        top_p: req.top_p,
        max_tokens,
        max_completion_tokens,
        stream: Some(req.stream),
        stop: if req.stop_sequences.is_empty() {
            None
        } else {
            Some(req.stop_sequences)
        },
        tools,
        tool_choice,
    })
}

fn from_openai_response(resp: OpenAIChatResponse) -> Result<NormalizedResponse> {
    let choices = resp
        .choices
        .into_iter()
        .map(|choice| {
            let role = match choice.message.role.as_str() {
                "assistant" => Role::Assistant,
                "system" => Role::System,
                "user" => Role::User,
                "tool" => Role::Tool,
                _ => Role::Assistant,
            };

            let content = choice
                .message
                .content
                .map(MessageContent::Text)
                .unwrap_or_else(|| MessageContent::Text(String::new()));

            let tool_calls = choice
                .message
                .tool_calls
                .unwrap_or_default()
                .into_iter()
                .map(|tc| ToolCall {
                    id: tc.id,
                    tool_type: tc.tool_type,
                    function: FunctionCall {
                        name: tc.function.name,
                        arguments: tc.function.arguments,
                    },
                })
                .collect();

            let finish_reason = choice.finish_reason.and_then(|fr| match fr.as_str() {
                "stop" => Some(FinishReason::Stop),
                "length" => Some(FinishReason::Length),
                "tool_calls" => Some(FinishReason::ToolCalls),
                "content_filter" => Some(FinishReason::ContentFilter),
                _ => None,
            });

            lunaroute_core::normalized::Choice {
                index: choice.index,
                message: Message {
                    role,
                    content,
                    name: choice.message.name,
                    tool_calls,
                    tool_call_id: choice.message.tool_call_id,
                },
                finish_reason,
            }
        })
        .collect();

    Ok(NormalizedResponse {
        id: resp.id,
        model: resp.model,
        choices,
        usage: Usage {
            prompt_tokens: resp.usage.prompt_tokens,
            completion_tokens: resp.usage.completion_tokens,
            total_tokens: resp.usage.total_tokens,
        },
        created: resp.created,
        metadata: std::collections::HashMap::new(),
    })
}

fn create_openai_stream(
    response: reqwest::Response,
) -> Pin<Box<dyn Stream<Item = lunaroute_core::Result<NormalizedStreamEvent>> + Send>> {
    use futures::StreamExt;

    let byte_stream = response.bytes_stream();
    let event_stream = eventsource_stream::EventStream::new(byte_stream);

    let stream = event_stream
        .map(|result| match result {
            Ok(event) => {
                // [DONE] is just a sentinel - ignore it since End event already sent with finish_reason
                if event.data == "[DONE]" {
                    return None;
                }

                match serde_json::from_str::<OpenAIStreamChunk>(&event.data) {
                    Ok(chunk) => {
                        // Convert chunk to normalized events
                        if let Some(usage) = chunk.usage {
                            return Some(Ok(NormalizedStreamEvent::Usage {
                                usage: Usage {
                                    prompt_tokens: usage.prompt_tokens,
                                    completion_tokens: usage.completion_tokens,
                                    total_tokens: usage.total_tokens,
                                },
                            }));
                        }

                        if let Some(choice) = chunk.choices.first() {
                            if let Some(ref finish_reason) = choice.finish_reason {
                                let reason = match finish_reason.as_str() {
                                    "stop" => FinishReason::Stop,
                                    "length" => FinishReason::Length,
                                    "tool_calls" => FinishReason::ToolCalls,
                                    "content_filter" => FinishReason::ContentFilter,
                                    _ => FinishReason::Stop,
                                };
                                return Some(Ok(NormalizedStreamEvent::End {
                                    finish_reason: reason,
                                }));
                            }

                            if let Some(ref content) = choice.delta.content {
                                return Some(Ok(NormalizedStreamEvent::Delta {
                                    index: choice.index,
                                    delta: Delta {
                                        role: choice.delta.role.as_ref().and_then(|r| {
                                            match r.as_str() {
                                                "assistant" => Some(Role::Assistant),
                                                "user" => Some(Role::User),
                                                "system" => Some(Role::System),
                                                "tool" => Some(Role::Tool),
                                                _ => None,
                                            }
                                        }),
                                        content: Some(content.clone()),
                                    },
                                }));
                            }

                            // Process ALL tool calls, not just the first
                            if let Some(ref tool_calls) = choice.delta.tool_calls {
                                // For now, return first tool call (TODO: need to emit multiple events)
                                // This is a limitation of only being able to return one event per chunk
                                if let Some(tool_call) = tool_calls.first() {
                                    return Some(Ok(NormalizedStreamEvent::ToolCallDelta {
                                        index: choice.index,
                                        tool_call_index: tool_call.index,
                                        id: tool_call.id.clone(),
                                        function: tool_call.function.as_ref().map(|f| {
                                            FunctionCallDelta {
                                                name: f.name.clone(),
                                                arguments: f.arguments.clone(),
                                            }
                                        }),
                                    }));
                                }
                            }
                        }

                        // Skip chunks that don't contain meaningful data (e.g., first chunk with just role)
                        // Log for debugging but don't emit an event
                        tracing::debug!(
                            "Skipping OpenAI chunk with no actionable data: id={}",
                            chunk.id
                        );
                        None
                    }
                    Err(e) => Some(Err(lunaroute_core::Error::Provider(format!(
                        "Failed to parse OpenAI stream chunk: {}",
                        e
                    )))),
                }
            }
            Err(e) => Some(Err(lunaroute_core::Error::Provider(format!(
                "Stream error: {}",
                e
            )))),
        })
        .filter_map(|opt| async move { opt });

    Box::pin(stream)
}

// Helper trait for adding organization header
trait OrganizationHeader {
    fn apply_organization_header(self, config: &OpenAIConfig) -> Self;
}

impl OrganizationHeader for reqwest::RequestBuilder {
    fn apply_organization_header(self, config: &OpenAIConfig) -> Self {
        if let Some(ref org) = config.organization {
            self.header("OpenAI-Organization", org)
        } else {
            self
        }
    }
}

// Helper trait for handling responses
#[async_trait]
trait OpenAIResponseHandler {
    async fn handle_openai_response(self) -> Result<OpenAIChatResponse>;
}

#[async_trait]
impl OpenAIResponseHandler for reqwest::Response {
    async fn handle_openai_response(self) -> Result<OpenAIChatResponse> {
        let status = self.status();

        // Capture retry-after header before consuming response
        let retry_after_secs = self
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(crate::parse_retry_after);

        if !status.is_success() {
            let status_code = status.as_u16();
            let body = self
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());

            return Err(if status_code == 429 {
                debug!(
                    retry_after_secs = ?retry_after_secs,
                    "OpenAI rate limit exceeded"
                );
                EgressError::RateLimitExceeded { retry_after_secs }
            } else {
                EgressError::ProviderError {
                    status_code,
                    message: body,
                }
            });
        }

        self.json::<OpenAIChatResponse>()
            .await
            .map_err(|e| EgressError::ParseError(format!("Failed to parse OpenAI response: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunaroute_core::normalized::{FunctionCall, FunctionDefinition, Tool};

    #[test]
    fn test_openai_config_builder() {
        let config = OpenAIConfig::new("test-key")
            .with_base_url("https://custom.api.com")
            .with_organization("org-123");

        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.base_url, "https://custom.api.com");
        assert_eq!(config.organization, Some("org-123".to_string()));
    }

    #[tokio::test]
    async fn test_connector_creation() {
        let config = OpenAIConfig::new("test-key");
        let connector = OpenAIConnector::new(config).await;
        assert!(connector.is_ok());
    }

    #[tokio::test]
    async fn test_capabilities() {
        let config = OpenAIConfig::new("test-key");
        let connector = OpenAIConnector::new(config).await.unwrap();
        let caps = connector.capabilities();
        assert!(caps.supports_streaming);
        assert!(caps.supports_tools);
        assert!(caps.supports_vision);
    }

    #[test]
    fn test_to_openai_request_basic() {
        let normalized = NormalizedRequest {
            model: "gpt-4".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text("Hello".to_string()),
                name: None,
                tool_calls: vec![],
                tool_call_id: None,
            }],
            system: None,
            temperature: Some(0.7),
            max_tokens: Some(100),
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec![],
            tools: vec![],
            tool_results: vec![],
            tool_choice: None,
            metadata: std::collections::HashMap::new(),
        };

        let openai_req = to_openai_request(normalized).unwrap();
        assert_eq!(openai_req.model, "gpt-4");
        assert_eq!(openai_req.messages.len(), 1);
        assert_eq!(openai_req.messages[0].role, "user");
        assert_eq!(openai_req.messages[0].content, Some("Hello".to_string()));
    }

    #[test]
    fn test_from_openai_response() {
        let openai_resp = OpenAIChatResponse {
            id: "chatcmpl-123".to_string(),
            model: "gpt-4".to_string(),
            choices: vec![OpenAIChoice {
                index: 0,
                message: OpenAIMessage {
                    role: "assistant".to_string(),
                    content: Some("Hello!".to_string()),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: OpenAIUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
            created: 1234567890,
        };

        let normalized = from_openai_response(openai_resp).unwrap();
        assert_eq!(normalized.id, "chatcmpl-123");
        assert_eq!(normalized.choices[0].message.role, Role::Assistant);
        assert_eq!(normalized.usage.total_tokens, 15);
    }

    // Tool conversion tests
    #[test]
    fn test_to_openai_request_with_tools() {
        let normalized = NormalizedRequest {
            model: "gpt-4".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text("What's the weather?".to_string()),
                name: None,
                tool_calls: vec![],
                tool_call_id: None,
            }],
            system: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec![],
            tools: vec![Tool {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: "get_weather".to_string(),
                    description: Some("Get weather info".to_string()),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "location": {"type": "string"}
                        }
                    }),
                },
            }],
            tool_results: vec![],
            tool_choice: Some(ToolChoice::Auto),
            metadata: std::collections::HashMap::new(),
        };

        let openai_req = to_openai_request(normalized).unwrap();
        assert_eq!(openai_req.tools.as_ref().unwrap().len(), 1);
        assert_eq!(
            openai_req.tools.as_ref().unwrap()[0].function.name,
            "get_weather"
        );
        assert!(
            matches!(openai_req.tool_choice, Some(OpenAIToolChoice::String(ref s)) if s == "auto")
        );
    }

    #[test]
    fn test_to_openai_request_tool_choice_variants() {
        // Test Auto
        let req = NormalizedRequest {
            model: "gpt-4".to_string(),
            messages: vec![],
            system: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec![],
            tools: vec![],
            tool_results: vec![],
            tool_choice: Some(ToolChoice::Auto),
            metadata: std::collections::HashMap::new(),
        };
        let openai = to_openai_request(req).unwrap();
        assert!(matches!(openai.tool_choice, Some(OpenAIToolChoice::String(ref s)) if s == "auto"));

        // Test Required
        let req = NormalizedRequest {
            model: "gpt-4".to_string(),
            messages: vec![],
            system: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec![],
            tools: vec![],
            tool_results: vec![],
            tool_choice: Some(ToolChoice::Required),
            metadata: std::collections::HashMap::new(),
        };
        let openai = to_openai_request(req).unwrap();
        assert!(
            matches!(openai.tool_choice, Some(OpenAIToolChoice::String(ref s)) if s == "required")
        );

        // Test None
        let req = NormalizedRequest {
            model: "gpt-4".to_string(),
            messages: vec![],
            system: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec![],
            tools: vec![],
            tool_results: vec![],
            tool_choice: Some(ToolChoice::None),
            metadata: std::collections::HashMap::new(),
        };
        let openai = to_openai_request(req).unwrap();
        assert!(matches!(openai.tool_choice, Some(OpenAIToolChoice::String(ref s)) if s == "none"));

        // Test Specific
        let req = NormalizedRequest {
            model: "gpt-4".to_string(),
            messages: vec![],
            system: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec![],
            tools: vec![],
            tool_results: vec![],
            tool_choice: Some(ToolChoice::Specific {
                name: "my_func".to_string(),
            }),
            metadata: std::collections::HashMap::new(),
        };
        let openai = to_openai_request(req).unwrap();
        match openai.tool_choice {
            Some(OpenAIToolChoice::Object { function, .. }) => {
                assert_eq!(function.name, "my_func");
            }
            _ => panic!("Expected Object variant"),
        }
    }

    #[test]
    fn test_to_openai_request_with_tool_calls() {
        let normalized = NormalizedRequest {
            model: "gpt-4".to_string(),
            messages: vec![
                Message {
                    role: Role::User,
                    content: MessageContent::Text("Get weather".to_string()),
                    name: None,
                    tool_calls: vec![],
                    tool_call_id: None,
                },
                Message {
                    role: Role::Assistant,
                    content: MessageContent::Text("".to_string()),
                    name: None,
                    tool_calls: vec![ToolCall {
                        id: "call_123".to_string(),
                        tool_type: "function".to_string(),
                        function: FunctionCall {
                            name: "get_weather".to_string(),
                            arguments: r#"{"location":"NYC"}"#.to_string(),
                        },
                    }],
                    tool_call_id: None,
                },
            ],
            system: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec![],
            tools: vec![],
            tool_results: vec![],
            tool_choice: None,
            metadata: std::collections::HashMap::new(),
        };

        let openai_req = to_openai_request(normalized).unwrap();
        assert_eq!(openai_req.messages.len(), 2);
        assert!(openai_req.messages[1].tool_calls.is_some());
        assert_eq!(
            openai_req.messages[1].tool_calls.as_ref().unwrap()[0].id,
            "call_123"
        );
        // Content should be None when message has tool calls
        assert_eq!(openai_req.messages[1].content, None);
    }

    #[test]
    fn test_from_openai_response_with_tool_calls() {
        let openai_resp = OpenAIChatResponse {
            id: "chatcmpl-123".to_string(),
            model: "gpt-4".to_string(),
            choices: vec![OpenAIChoice {
                index: 0,
                message: OpenAIMessage {
                    role: "assistant".to_string(),
                    content: None,
                    name: None,
                    tool_calls: Some(vec![OpenAIToolCall {
                        id: "call_123".to_string(),
                        tool_type: "function".to_string(),
                        function: OpenAIFunctionCall {
                            name: "get_weather".to_string(),
                            arguments: r#"{"location":"NYC"}"#.to_string(),
                        },
                    }]),
                    tool_call_id: None,
                },
                finish_reason: Some("tool_calls".to_string()),
            }],
            usage: OpenAIUsage {
                prompt_tokens: 20,
                completion_tokens: 10,
                total_tokens: 30,
            },
            created: 1234567890,
        };

        let normalized = from_openai_response(openai_resp).unwrap();
        assert_eq!(normalized.choices[0].message.tool_calls.len(), 1);
        assert_eq!(
            normalized.choices[0].message.tool_calls[0].function.name,
            "get_weather"
        );
        assert_eq!(
            normalized.choices[0].finish_reason,
            Some(FinishReason::ToolCalls)
        );
    }

    #[test]
    fn test_to_openai_request_multiple_tool_calls() {
        let normalized = NormalizedRequest {
            model: "gpt-4".to_string(),
            messages: vec![Message {
                role: Role::Assistant,
                content: MessageContent::Text("".to_string()),
                name: None,
                tool_calls: vec![
                    ToolCall {
                        id: "call_1".to_string(),
                        tool_type: "function".to_string(),
                        function: FunctionCall {
                            name: "func1".to_string(),
                            arguments: "{}".to_string(),
                        },
                    },
                    ToolCall {
                        id: "call_2".to_string(),
                        tool_type: "function".to_string(),
                        function: FunctionCall {
                            name: "func2".to_string(),
                            arguments: "{}".to_string(),
                        },
                    },
                ],
                tool_call_id: None,
            }],
            system: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec![],
            tools: vec![],
            tool_results: vec![],
            tool_choice: None,
            metadata: std::collections::HashMap::new(),
        };

        let openai_req = to_openai_request(normalized).unwrap();
        assert_eq!(openai_req.messages[0].tool_calls.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_from_openai_response_all_finish_reasons() {
        let finish_reasons = vec![
            ("stop", FinishReason::Stop),
            ("length", FinishReason::Length),
            ("tool_calls", FinishReason::ToolCalls),
            ("content_filter", FinishReason::ContentFilter),
        ];

        for (openai_reason, expected_reason) in finish_reasons {
            let openai_resp = OpenAIChatResponse {
                id: "test".to_string(),
                model: "gpt-4".to_string(),
                choices: vec![OpenAIChoice {
                    index: 0,
                    message: OpenAIMessage {
                        role: "assistant".to_string(),
                        content: Some("test".to_string()),
                        name: None,
                        tool_calls: None,
                        tool_call_id: None,
                    },
                    finish_reason: Some(openai_reason.to_string()),
                }],
                usage: OpenAIUsage {
                    prompt_tokens: 1,
                    completion_tokens: 1,
                    total_tokens: 2,
                },
                created: 0,
            };

            let normalized = from_openai_response(openai_resp).unwrap();
            assert_eq!(normalized.choices[0].finish_reason, Some(expected_reason));
        }
    }

    // Edge case tests
    #[test]
    fn test_to_openai_request_multimodal_content() {
        use lunaroute_core::normalized::{ContentPart, ImageSource};

        let normalized = NormalizedRequest {
            model: "gpt-4-vision".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Parts(vec![
                    ContentPart::Text {
                        text: "First".to_string(),
                    },
                    ContentPart::Image {
                        source: ImageSource::Url {
                            url: "https://example.com/image.jpg".to_string(),
                        },
                    },
                    ContentPart::Text {
                        text: "Second".to_string(),
                    },
                ]),
                name: None,
                tool_calls: vec![],
                tool_call_id: None,
            }],
            system: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec![],
            tools: vec![],
            tool_results: vec![],
            tool_choice: None,
            metadata: std::collections::HashMap::new(),
        };

        let openai_req = to_openai_request(normalized).unwrap();
        // Should extract text and join with newlines, ignoring images
        assert_eq!(
            openai_req.messages[0].content,
            Some("First\nSecond".to_string())
        );
    }

    #[test]
    fn test_to_openai_request_empty_tools() {
        let normalized = NormalizedRequest {
            model: "gpt-4".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text("Hello".to_string()),
                name: None,
                tool_calls: vec![],
                tool_call_id: None,
            }],
            system: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec![],
            tools: vec![],
            tool_results: vec![],
            tool_choice: None,
            metadata: std::collections::HashMap::new(),
        };

        let openai_req = to_openai_request(normalized).unwrap();
        assert!(openai_req.tools.is_none());
    }

    #[test]
    fn test_to_openai_request_with_stop_sequences() {
        let normalized = NormalizedRequest {
            model: "gpt-4".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text("Hello".to_string()),
                name: None,
                tool_calls: vec![],
                tool_call_id: None,
            }],
            system: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            stream: false,
            stop_sequences: vec!["STOP".to_string(), "END".to_string()],
            tools: vec![],
            tool_results: vec![],
            tool_choice: None,
            metadata: std::collections::HashMap::new(),
        };

        let openai_req = to_openai_request(normalized).unwrap();
        assert_eq!(
            openai_req.stop,
            Some(vec!["STOP".to_string(), "END".to_string()])
        );
    }

    #[test]
    fn test_from_openai_response_multiple_choices() {
        let openai_resp = OpenAIChatResponse {
            id: "test".to_string(),
            model: "gpt-4".to_string(),
            choices: vec![
                OpenAIChoice {
                    index: 0,
                    message: OpenAIMessage {
                        role: "assistant".to_string(),
                        content: Some("First".to_string()),
                        name: None,
                        tool_calls: None,
                        tool_call_id: None,
                    },
                    finish_reason: Some("stop".to_string()),
                },
                OpenAIChoice {
                    index: 1,
                    message: OpenAIMessage {
                        role: "assistant".to_string(),
                        content: Some("Second".to_string()),
                        name: None,
                        tool_calls: None,
                        tool_call_id: None,
                    },
                    finish_reason: Some("stop".to_string()),
                },
            ],
            usage: OpenAIUsage {
                prompt_tokens: 10,
                completion_tokens: 10,
                total_tokens: 20,
            },
            created: 0,
        };

        let normalized = from_openai_response(openai_resp).unwrap();
        assert_eq!(normalized.choices.len(), 2);
        assert_eq!(normalized.choices[0].index, 0);
        assert_eq!(normalized.choices[1].index, 1);
    }

    #[test]
    fn test_from_openai_response_empty_content() {
        let openai_resp = OpenAIChatResponse {
            id: "test".to_string(),
            model: "gpt-4".to_string(),
            choices: vec![OpenAIChoice {
                index: 0,
                message: OpenAIMessage {
                    role: "assistant".to_string(),
                    content: None,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: OpenAIUsage {
                prompt_tokens: 10,
                completion_tokens: 0,
                total_tokens: 10,
            },
            created: 0,
        };

        let normalized = from_openai_response(openai_resp).unwrap();
        // Should default to empty string
        match &normalized.choices[0].message.content {
            MessageContent::Text(text) => assert_eq!(text, ""),
            _ => panic!("Expected Text content"),
        }
    }

    #[test]
    fn test_config_with_switch_notification_message() {
        let config = OpenAIConfig {
            api_key: "test-key".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            organization: None,
            client_config: Default::default(),
            custom_headers: None,
            request_body_config: None,
            response_body_config: None,
            codex_auth: None,
            switch_notification_message: Some("Custom switch message".to_string()),
        };

        assert_eq!(
            config.switch_notification_message.unwrap(),
            "Custom switch message"
        );
    }
}
