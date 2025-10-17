//! Shared types for multi-tenancy support
//!
//! TODO: This module will contain proper type definitions for:
//! - SearchQuery
//! - SearchResults
//! - RetentionPolicy
//! - CleanupStats
//! - TimeRange
//! - AggregateStats
//! - Session
//!
//! For now, these are defined as placeholder types (serde_json::Value)
//! in session_store.rs to allow the trait to compile.

// This is intentionally minimal for now.
// We'll add proper struct definitions as we refactor the session crate.
