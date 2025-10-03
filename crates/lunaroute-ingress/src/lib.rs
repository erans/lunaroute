//! LunaRoute Ingress Adapters
//!
//! This crate provides HTTP ingress adapters for various LLM API formats:
//! - OpenAI-compatible endpoints
//! - Anthropic-compatible endpoints

pub mod anthropic;
pub mod async_stream_parser;
pub mod middleware;
pub mod openai;
pub mod streaming_metrics;
pub mod types;

pub use middleware::CorsConfig;
pub use types::{
    IngressError, IngressResult, RequestId, RequestMetadata, StreamEvent, TraceContext,
};
