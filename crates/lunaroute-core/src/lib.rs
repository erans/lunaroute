//! LunaRoute Core Types and Traits
//!
//! This crate provides the fundamental types and traits used throughout LunaRoute:
//!
//! ## Multi-Tenancy Support (NEW)
//! - [`config_store`]: Configuration storage abstraction
//! - [`session_store`]: Session data storage abstraction
//! - [`tenant`]: Tenant types and context
//! - [`events`]: Session event types (to be moved from lunaroute-session)
//! - [`types`]: Shared types for stores
//!
//! ## Existing Core Types
//! - [`normalized`]: Normalized request/response types
//! - [`provider`]: Provider trait abstractions
//! - [`error`]: Core error types
//! - [`template`]: Template engine for variable substitution
//!
//! # Multi-Tenancy Architecture
//!
//! LunaRoute supports both single-tenant (local) and multi-tenant (hosted) deployments
//! using trait-based abstractions:
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │         LunaRoute Application           │
//! └─────────────────┬───────────────────────┘
//!                   │
//!         ┌─────────┴─────────┐
//!         │  TenantContext    │
//!         │  tenant_id: Option│
//!         └─────────┬─────────┘
//!                   │
//!      ┌────────────┴────────────┐
//!      │                         │
//!      ▼                         ▼
//! ┌──────────┐            ┌──────────┐
//! │ConfigStore│            │SessionStore│
//! └──────────┘            └──────────┘
//!      │                         │
//!   ┌──┴──┐                   ┌──┴──┐
//!   │     │                   │     │
//!   ▼     ▼                   ▼     ▼
//! File  Postgres           SQLite  Timescale
//! (local) (cloud)         (local)  (cloud)
//! ```
//!
//! See [`docs/multi-tenancy-architecture.md`](../../../docs/multi-tenancy-architecture.md)
//! for the complete architecture documentation.

// Multi-tenancy modules
pub mod config_store;
pub mod events;
pub mod session_store;
pub mod tenant;
pub mod types;

// Existing modules
pub mod error;
pub mod normalized;
pub mod provider;
pub mod template;

// Re-exports
pub use config_store::ConfigStore;
pub use error::{Error, Result};
pub use session_store::SessionStore;
pub use tenant::{TenantContext, TenantId};
