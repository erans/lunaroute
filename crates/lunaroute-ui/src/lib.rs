//! LunaRoute Web UI
//!
//! Embedded web dashboard for visualizing session data from SQLite.
//! All HTML templates and custom JS/CSS are compiled into the binary.
//! Third-party libraries (Chart.js, etc.) are loaded from CDN.

pub mod handlers;
pub mod models;
pub mod queries;
pub mod server;
pub mod stats;

pub use server::{UiConfig, UiServer};

use sqlx::SqlitePool;
use std::sync::Arc;

/// Shared application state for the UI server
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<SqlitePool>,
    pub config: UiConfig,
}
