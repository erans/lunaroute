//! LunaRoute Core Types and Traits
//!
//! This crate provides the fundamental types and traits used throughout LunaRoute:
//! - Normalized request/response types
//! - Provider trait abstractions
//! - Core error types

pub mod error;
pub mod normalized;
pub mod provider;

pub use error::{Error, Result};
