//! Path classification for routing bypass
//!
//! This module provides a classifier to determine whether a request path should
//! be intercepted by the routing engine or bypassed for direct proxying.

use std::collections::HashSet;

/// Classifies API paths as intercepted (routed) or bypassed (direct proxy)
#[derive(Debug, Clone)]
pub struct PathClassifier {
    intercepted_paths: HashSet<String>,
    bypass_enabled: bool,
}

impl PathClassifier {
    /// Create a new PathClassifier with default intercepted paths
    ///
    /// # Arguments
    /// * `bypass_enabled` - Whether bypass is enabled for non-intercepted paths
    ///
    /// # Intercepted Paths (Use Routing Engine)
    /// - `/v1/chat/completions` (OpenAI)
    /// - `/v1/messages` (Anthropic)
    /// - `/v1/models` (Both)
    /// - `/healthz`, `/readyz`, `/metrics` (Observability)
    ///
    /// # Bypassed Paths (Direct Proxy)
    /// - `/v1/embeddings`
    /// - `/v1/audio/*`
    /// - `/v1/images/*`
    /// - `/v1/files/*`
    /// - `/v1/fine-tuning/*`
    /// - `/v1/assistants/*`
    /// - `/v1/threads/*`
    /// - Any other unknown paths
    pub fn new(bypass_enabled: bool) -> Self {
        let intercepted_paths = HashSet::from([
            "/v1/chat/completions".to_string(),
            "/v1/messages".to_string(),
            "/v1/models".to_string(),
            "/healthz".to_string(),
            "/readyz".to_string(),
            "/metrics".to_string(),
        ]);

        Self {
            intercepted_paths,
            bypass_enabled,
        }
    }

    /// Check if a path should be bypassed (proxied directly without routing)
    ///
    /// Returns `true` if:
    /// - Bypass is enabled AND
    /// - Path is not in the intercepted list
    ///
    /// # Examples
    /// ```
    /// use lunaroute_routing::PathClassifier;
    ///
    /// let classifier = PathClassifier::new(true);
    ///
    /// // Intercepted paths - use routing engine
    /// assert!(!classifier.should_bypass("/v1/chat/completions"));
    /// assert!(!classifier.should_bypass("/v1/messages"));
    ///
    /// // Unknown paths - bypass to direct proxy
    /// assert!(classifier.should_bypass("/v1/embeddings"));
    /// assert!(classifier.should_bypass("/v1/audio/transcriptions"));
    /// ```
    pub fn should_bypass(&self, path: &str) -> bool {
        self.bypass_enabled && !self.intercepted_paths.contains(path)
    }

    /// Check if a path is in the intercepted list
    ///
    /// Returns `true` if the path should use the routing engine
    pub fn is_intercepted(&self, path: &str) -> bool {
        self.intercepted_paths.contains(path)
    }

    /// Check if bypass is enabled
    pub fn is_bypass_enabled(&self) -> bool {
        self.bypass_enabled
    }

    /// Get the list of intercepted paths
    pub fn intercepted_paths(&self) -> Vec<String> {
        self.intercepted_paths.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bypass_enabled_with_intercepted_path() {
        let classifier = PathClassifier::new(true);

        // Intercepted paths should NOT be bypassed
        assert!(!classifier.should_bypass("/v1/chat/completions"));
        assert!(!classifier.should_bypass("/v1/messages"));
        assert!(!classifier.should_bypass("/v1/models"));
        assert!(!classifier.should_bypass("/healthz"));
        assert!(!classifier.should_bypass("/readyz"));
        assert!(!classifier.should_bypass("/metrics"));
    }

    #[test]
    fn test_bypass_enabled_with_unknown_path() {
        let classifier = PathClassifier::new(true);

        // Unknown paths should be bypassed
        assert!(classifier.should_bypass("/v1/embeddings"));
        assert!(classifier.should_bypass("/v1/audio/transcriptions"));
        assert!(classifier.should_bypass("/v1/images/generations"));
        assert!(classifier.should_bypass("/v1/files"));
        assert!(classifier.should_bypass("/v1/fine-tuning/jobs"));
        assert!(classifier.should_bypass("/v1/assistants"));
        assert!(classifier.should_bypass("/v1/threads"));
        assert!(classifier.should_bypass("/unknown/path"));
    }

    #[test]
    fn test_bypass_disabled() {
        let classifier = PathClassifier::new(false);

        // With bypass disabled, nothing should be bypassed
        assert!(!classifier.should_bypass("/v1/chat/completions"));
        assert!(!classifier.should_bypass("/v1/messages"));
        assert!(!classifier.should_bypass("/v1/embeddings"));
        assert!(!classifier.should_bypass("/v1/audio/transcriptions"));
        assert!(!classifier.should_bypass("/unknown/path"));
    }

    #[test]
    fn test_is_intercepted() {
        let classifier = PathClassifier::new(true);

        // Check intercepted paths
        assert!(classifier.is_intercepted("/v1/chat/completions"));
        assert!(classifier.is_intercepted("/v1/messages"));
        assert!(classifier.is_intercepted("/v1/models"));
        assert!(classifier.is_intercepted("/healthz"));

        // Check non-intercepted paths
        assert!(!classifier.is_intercepted("/v1/embeddings"));
        assert!(!classifier.is_intercepted("/unknown/path"));
    }

    #[test]
    fn test_is_bypass_enabled() {
        let classifier_enabled = PathClassifier::new(true);
        assert!(classifier_enabled.is_bypass_enabled());

        let classifier_disabled = PathClassifier::new(false);
        assert!(!classifier_disabled.is_bypass_enabled());
    }

    #[test]
    fn test_intercepted_paths_list() {
        let classifier = PathClassifier::new(true);
        let paths = classifier.intercepted_paths();

        assert_eq!(paths.len(), 6);
        assert!(paths.contains(&"/v1/chat/completions".to_string()));
        assert!(paths.contains(&"/v1/messages".to_string()));
        assert!(paths.contains(&"/v1/models".to_string()));
        assert!(paths.contains(&"/healthz".to_string()));
        assert!(paths.contains(&"/readyz".to_string()));
        assert!(paths.contains(&"/metrics".to_string()));
    }
}
