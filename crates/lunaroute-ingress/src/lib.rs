//! LunaRoute Ingress Adapters
//!
//! This crate provides HTTP ingress adapters for various LLM API formats:
//! - OpenAI-compatible endpoints
//! - Anthropic-compatible endpoints

pub mod anthropic;
pub mod middleware;
pub mod openai;
