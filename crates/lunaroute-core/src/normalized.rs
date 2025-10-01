//! Normalized request and response types

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedRequest {
    // TODO: Implement normalized request structure
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedResponse {
    // TODO: Implement normalized response structure
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NormalizedStreamEvent {
    Start,
    Delta,
    ToolCall,
    Usage,
    End,
    Error,
}
