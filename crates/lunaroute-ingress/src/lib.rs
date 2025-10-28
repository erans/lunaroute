//! LunaRoute Ingress Adapters
//!
//! This crate provides HTTP ingress adapters for various LLM API formats:
//! - OpenAI-compatible endpoints
//! - Anthropic-compatible endpoints
//! - Dual-dialect mode (both OpenAI and Anthropic endpoints)
//! - Bypass proxy for unknown paths

pub mod anthropic;
pub mod async_stream_parser;
pub mod bypass;
pub mod middleware;
pub mod multi_dialect;
pub mod openai;
pub mod streaming_metrics;
pub mod types;

pub use bypass::{BypassError, BypassProvider, proxy_request, with_bypass};
pub use middleware::CorsConfig;
pub use types::{
    IngressError, IngressResult, RequestId, RequestMetadata, StreamEvent, TraceContext,
};
