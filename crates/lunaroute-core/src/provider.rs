//! Provider trait definitions

use crate::{
    Result,
    normalized::{NormalizedRequest, NormalizedResponse, NormalizedStreamEvent},
};
use futures::Stream;

#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// Send a non-streaming request
    async fn send(&self, request: NormalizedRequest) -> Result<NormalizedResponse>;

    /// Send a streaming request
    async fn stream(
        &self,
        request: NormalizedRequest,
    ) -> Result<Box<dyn Stream<Item = Result<NormalizedStreamEvent>> + Send + Unpin>>;

    /// Get provider capabilities
    fn capabilities(&self) -> ProviderCapabilities;

    /// Get custom notification message for when this provider is used as alternative
    fn get_notification_message(&self) -> Option<&str> {
        None // Default implementation
    }
}

#[derive(Debug, Clone)]
pub struct ProviderCapabilities {
    pub supports_streaming: bool,
    pub supports_tools: bool,
    pub supports_vision: bool,
}

#[cfg(test)]
mod tests;
