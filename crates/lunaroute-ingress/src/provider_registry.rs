use lunaroute_egress::{anthropic::AnthropicConnector, openai::OpenAIConnector};
use std::collections::HashMap;
use std::sync::Arc;

/// Which dialect a provider speaks
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderType {
    OpenAI,
    Anthropic,
}

/// A named provider entry in the registry
#[derive(Clone)]
pub struct ProviderEntry {
    pub connector_type: ProviderType,
    pub openai_connector: Option<Arc<OpenAIConnector>>,
    pub anthropic_connector: Option<Arc<AnthropicConnector>>,
    pub model_override: Option<String>,
}

impl std::fmt::Debug for ProviderEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderEntry")
            .field("connector_type", &self.connector_type)
            .field("openai_connector", &self.openai_connector.is_some())
            .field("anthropic_connector", &self.anthropic_connector.is_some())
            .field("model_override", &self.model_override)
            .finish()
    }
}

/// Registry of all named providers, built at startup from config
pub type ProviderRegistry = HashMap<String, ProviderEntry>;
