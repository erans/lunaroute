//! Codex authentication token reading
//!
//! This module provides functionality to read authentication tokens from Codex's
//! auth.json file, with support for file watching and automatic token refresh.

use crate::{EgressError, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Read Codex authentication token from JSON file
///
/// # Arguments
/// * `auth_file` - Path to the auth JSON file (supports ~ for home directory)
/// * `token_field` - JSON field name containing the access token
///
/// # Returns
/// * `Ok(Some(String))` - Token successfully read
/// * `Ok(None)` - File doesn't exist or token field not found
/// * `Err(...)` - Error reading or parsing file
pub fn read_codex_token(auth_file: &Path, token_field: &str) -> Result<Option<String>> {
    // Expand tilde in path
    let expanded_path = expand_tilde(auth_file)?;

    debug!(
        "Reading Codex auth token from: {} (field: {})",
        expanded_path.display(),
        token_field
    );

    // Check if file exists
    if !expanded_path.exists() {
        debug!(
            "Codex auth file does not exist: {}",
            expanded_path.display()
        );
        return Ok(None);
    }

    // Read file contents
    let contents = fs::read_to_string(&expanded_path).map_err(|e| {
        EgressError::ConfigError(format!(
            "Failed to read Codex auth file {}: {}",
            expanded_path.display(),
            e
        ))
    })?;

    // Parse JSON
    let json: Value = serde_json::from_str(&contents).map_err(|e| {
        EgressError::ConfigError(format!(
            "Failed to parse Codex auth JSON from {}: {}",
            expanded_path.display(),
            e
        ))
    })?;

    // Extract token field (supports nested paths like "tokens.access_token")
    let token_value = extract_nested_field(&json, token_field);

    match token_value {
        Some(Value::String(token)) => {
            if token.is_empty() {
                warn!("Codex auth token is empty in {}", expanded_path.display());
                Ok(None)
            } else {
                debug!(
                    "Successfully read Codex token from {} (length: {} chars)",
                    expanded_path.display(),
                    token.len()
                );
                Ok(Some(token.clone()))
            }
        }
        Some(_) => {
            warn!(
                "Codex auth token field '{}' is not a string in {}",
                token_field,
                expanded_path.display()
            );
            Ok(None)
        }
        None => {
            warn!(
                "Codex auth token field '{}' not found in {}",
                token_field,
                expanded_path.display()
            );
            Ok(None)
        }
    }
}

/// Extract a nested field from JSON using dot notation (e.g., "tokens.access_token")
fn extract_nested_field<'a>(json: &'a Value, path: &str) -> Option<&'a Value> {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = json;

    for part in parts {
        current = current.get(part)?;
    }

    Some(current)
}

/// Expand tilde (~) in path to home directory
fn expand_tilde(path: &Path) -> Result<PathBuf> {
    let path_str = path
        .to_str()
        .ok_or_else(|| EgressError::ConfigError("Invalid UTF-8 in path".to_string()))?;

    if let Some(stripped) = path_str.strip_prefix("~/") {
        // Get home directory
        let home = dirs::home_dir().ok_or_else(|| {
            EgressError::ConfigError("Could not determine home directory".to_string())
        })?;

        // Replace ~ with home directory
        let expanded = home.join(stripped);
        Ok(expanded)
    } else if path_str == "~" {
        dirs::home_dir().ok_or_else(|| {
            EgressError::ConfigError("Could not determine home directory".to_string())
        })
    } else {
        Ok(path.to_path_buf())
    }
}

/// Check if a JWT token appears to be expired
///
/// This does basic heuristic checks without actually parsing the JWT:
/// - Returns true if token is too short to be valid
/// - Could be extended to parse expiration claim in the future
///
/// # Arguments
/// * `token` - The JWT token string
///
/// # Returns
/// * `true` if token appears expired or invalid
/// * `false` if token looks valid (does not guarantee it's not expired)
pub fn is_token_likely_expired(token: &str) -> bool {
    // Basic sanity check: JWT should be at least 3 parts separated by dots
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return true;
    }

    // Check each part is non-empty and base64-like
    for part in parts {
        if part.is_empty() || part.len() < 4 {
            return true;
        }
    }

    // TODO: Could parse payload and check 'exp' claim for actual expiration
    // For now, assume token structure is valid means it's not expired
    false
}

// Token exchange functionality - currently unused, kept for reference
// The access_token from auth.json can be used directly without exchange
//
// /// OAuth issuer endpoint for OpenAI
// const OAUTH_ISSUER: &str = "https://auth.openai.com";
//
// /// Codex CLI client ID
// const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
//
// /// Extract the default organization ID from an OpenAI id_token JWT
// ///
// /// Parses the JWT payload and extracts the organization_id from the
// /// `https://api.openai.com/auth.organizations` array, looking for the
// /// organization with `is_default: true`.
// ///
// /// # Arguments
// /// * `id_token` - The JWT id_token string
// ///
// /// # Returns
// /// * `Ok(String)` - The organization ID (e.g., "org-...")
// /// * `Err(...)` - Error parsing JWT or extracting organization
// #[allow(dead_code)]
// fn extract_default_org_id(id_token: &str) -> Result<String> {
//     use base64::prelude::*;
//
//     // JWT format: header.payload.signature
//     let parts: Vec<&str> = id_token.split('.').collect();
//     if parts.len() != 3 {
//         return Err(EgressError::ConfigError(
//             "Invalid JWT format: expected 3 parts".to_string(),
//         ));
//     }
//
//     // Decode the payload (second part)
//     let payload_b64 = parts[1];
//     let payload_bytes = BASE64_URL_SAFE_NO_PAD
//         .decode(payload_b64)
//         .map_err(|e| EgressError::ConfigError(format!("Failed to decode JWT payload: {}", e)))?;
//
//     let payload_str = String::from_utf8(payload_bytes)
//         .map_err(|e| EgressError::ConfigError(format!("Invalid UTF-8 in JWT payload: {}", e)))?;
//
//     // Parse JSON payload
//     let payload: Value = serde_json::from_str(&payload_str).map_err(|e| {
//         EgressError::ConfigError(format!("Failed to parse JWT payload JSON: {}", e))
//     })?;
//
//     // Extract organizations array from https://api.openai.com/auth.organizations
//     let organizations = payload
//         .get("https://api.openai.com/auth")
//         .and_then(|auth| auth.get("organizations"))
//         .and_then(|orgs| orgs.as_array())
//         .ok_or_else(|| {
//             EgressError::ConfigError(
//                 "JWT payload missing 'https://api.openai.com/auth.organizations'".to_string(),
//             )
//         })?;
//
//     // Find the default organization
//     for org in organizations {
//         if let Some(is_default) = org.get("is_default").and_then(|v| v.as_bool())
//             && is_default
//                 && let Some(org_id) = org.get("id").and_then(|v| v.as_str()) {
//                     return Ok(org_id.to_string());
//                 }
//     }
//
//     Err(EgressError::ConfigError(
//         "No default organization found in JWT".to_string(),
//     ))
// }
//
// /// Response from OAuth token exchange
// #[derive(Debug, Deserialize, Serialize)]
// struct TokenExchangeResponse {
//     access_token: String,
//     #[serde(default)]
//     token_type: Option<String>,
//     #[serde(default)]
//     expires_in: Option<u64>,
// }
//
// /// Exchange an OpenAI id_token for an OpenAI API key
// ///
// /// Performs OAuth token exchange using the same flow as Codex CLI login.
// /// This exchanges an id_token (from ChatGPT OAuth) for an openai-api-key
// /// that has the necessary scopes for the Responses API.
// ///
// /// # Arguments
// /// * `id_token` - The OpenAI id_token from auth.json (tokens.id_token)
// ///
// /// # Returns
// /// * `Ok(String)` - The exchanged API key
// /// * `Err(...)` - Error performing exchange or parsing response
// ///
// /// # Reference
// /// This mirrors the token exchange logic from:
// /// https://github.com/openai/codex/blob/main/codex-rs/login/src/server.rs
// pub async fn exchange_id_token_for_api_key(id_token: &str) -> Result<String> {
//     debug!("Attempting to exchange id_token for OpenAI API key");
//
//     // Create HTTP client
//     let client = reqwest::Client::builder()
//         .timeout(std::time::Duration::from_secs(30))
//         .build()
//         .map_err(|e| EgressError::ConfigError(format!("Failed to create HTTP client: {}", e)))?;
//
//     // Build form data manually (exactly matches Codex CLI implementation)
//     use serde_urlencoded::to_string as urlencode_pairs;
//
//     let form_params = [
//         ("grant_type", "urn:ietf:params:oauth:grant-type:token-exchange"),
//         ("client_id", CODEX_CLIENT_ID),
//         ("requested_token", "openai-api-key"),
//         ("subject_token", id_token),
//         ("subject_token_type", "urn:ietf:params:oauth:token-type:id_token"),
//     ];
//
//     let body = urlencode_pairs(form_params).map_err(|e| {
//         EgressError::ConfigError(format!("Failed to URL encode form data: {}", e))
//     })?;
//
//     // Perform token exchange
//     let url = format!("{}/oauth/token", OAUTH_ISSUER);
//     debug!("Sending token exchange request to: {}", url);
//
//     let response = client
//         .post(&url)
//         .header("Content-Type", "application/x-www-form-urlencoded")
//         .body(body)
//         .send()
//         .await
//         .map_err(|e| {
//             warn!("Token exchange request failed: {}", e);
//             EgressError::ConfigError(format!("Token exchange request failed: {}", e))
//         })?;
//
//     let status = response.status();
//     if !status.is_success() {
//         let error_body = response
//             .text()
//             .await
//             .unwrap_or_else(|_| "Unable to read error body".to_string());
//         warn!(
//             "Token exchange failed with status {}: {}",
//             status, error_body
//         );
//         return Err(EgressError::ConfigError(format!(
//             "Token exchange failed with status {}: {}",
//             status, error_body
//         )));
//     }
//
//     // Parse response
//     let exchange_response: TokenExchangeResponse = response.json().await.map_err(|e| {
//         warn!("Failed to parse token exchange response: {}", e);
//         EgressError::ConfigError(format!("Failed to parse token exchange response: {}", e))
//     })?;
//
//     debug!("Successfully exchanged id_token for OpenAI API key");
//     Ok(exchange_response.access_token)
// }

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_read_codex_token_success() {
        let temp_dir = TempDir::new().unwrap();
        let auth_file = temp_dir.path().join("auth.json");

        // Write test auth file with nested structure
        let mut file = fs::File::create(&auth_file).unwrap();
        file.write_all(br#"{"tokens": {"access_token": "test-token-123"}}"#)
            .unwrap();

        // Read token with nested path
        let result = read_codex_token(&auth_file, "tokens.access_token").unwrap();
        assert_eq!(result, Some("test-token-123".to_string()));
    }

    #[test]
    fn test_read_codex_token_flat_structure() {
        let temp_dir = TempDir::new().unwrap();
        let auth_file = temp_dir.path().join("auth.json");

        // Write test auth file with flat structure (legacy)
        let mut file = fs::File::create(&auth_file).unwrap();
        file.write_all(br#"{"access_token": "test-token-456"}"#)
            .unwrap();

        // Read token with simple path
        let result = read_codex_token(&auth_file, "access_token").unwrap();
        assert_eq!(result, Some("test-token-456".to_string()));
    }

    #[test]
    fn test_read_codex_token_missing_file() {
        let temp_dir = TempDir::new().unwrap();
        let auth_file = temp_dir.path().join("nonexistent.json");

        // Should return None for missing file
        let result = read_codex_token(&auth_file, "access_token").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_read_codex_token_missing_field() {
        let temp_dir = TempDir::new().unwrap();
        let auth_file = temp_dir.path().join("auth.json");

        // Write auth file without access_token field
        let mut file = fs::File::create(&auth_file).unwrap();
        file.write_all(br#"{"other_field": "value"}"#).unwrap();

        // Should return None for missing field
        let result = read_codex_token(&auth_file, "access_token").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_read_codex_token_empty_token() {
        let temp_dir = TempDir::new().unwrap();
        let auth_file = temp_dir.path().join("auth.json");

        // Write auth file with empty token
        let mut file = fs::File::create(&auth_file).unwrap();
        file.write_all(br#"{"access_token": ""}"#).unwrap();

        // Should return None for empty token
        let result = read_codex_token(&auth_file, "access_token").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_read_codex_token_invalid_json() {
        let temp_dir = TempDir::new().unwrap();
        let auth_file = temp_dir.path().join("auth.json");

        // Write invalid JSON
        let mut file = fs::File::create(&auth_file).unwrap();
        file.write_all(b"not valid json").unwrap();

        // Should return error for invalid JSON
        let result = read_codex_token(&auth_file, "access_token");
        assert!(result.is_err());
    }

    #[test]
    fn test_read_codex_token_non_string_field() {
        let temp_dir = TempDir::new().unwrap();
        let auth_file = temp_dir.path().join("auth.json");

        // Write auth file with non-string token field
        let mut file = fs::File::create(&auth_file).unwrap();
        file.write_all(br#"{"access_token": 123}"#).unwrap();

        // Should return None for non-string field
        let result = read_codex_token(&auth_file, "access_token").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_is_token_likely_expired() {
        // Valid-looking JWT
        assert!(!is_token_likely_expired(
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"
        ));

        // Invalid tokens
        assert!(is_token_likely_expired(""));
        assert!(is_token_likely_expired("not.a.token"));
        assert!(is_token_likely_expired("a.b")); // Only 2 parts
        assert!(is_token_likely_expired("...")); // Empty parts
    }

    #[test]
    fn test_expand_tilde() {
        // Test tilde expansion
        let path = Path::new("~/test/path");
        let expanded = expand_tilde(path).unwrap();
        assert!(!expanded.to_string_lossy().contains('~'));

        // Test non-tilde path
        let path = Path::new("/absolute/path");
        let expanded = expand_tilde(path).unwrap();
        assert_eq!(expanded, PathBuf::from("/absolute/path"));

        // Test just tilde
        let path = Path::new("~");
        let expanded = expand_tilde(path).unwrap();
        assert!(!expanded.to_string_lossy().contains('~'));
    }
}
