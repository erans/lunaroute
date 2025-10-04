//! LunaRoute Core Types and Traits
//!
//! This crate provides the fundamental types and traits used throughout LunaRoute:
//! - Normalized request/response types
//! - Provider trait abstractions
//! - Core error types
//! - Template engine for variable substitution

pub mod error;
pub mod normalized;
pub mod provider;
pub mod template;

pub use error::{Error, Result};
