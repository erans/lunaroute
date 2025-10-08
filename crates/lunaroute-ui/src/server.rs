//! Web UI server implementation

use crate::handlers;
use crate::AppState;
use axum::{routing::get, Router};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing::info;

/// UI server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    /// Enable the UI server
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Host to bind to (default: 127.0.0.1 for security)
    #[serde(default = "default_host")]
    pub host: String,

    /// Port to listen on (default: 8082)
    #[serde(default = "default_port")]
    pub port: u16,

    /// Auto-refresh interval in seconds (default: 5)
    #[serde(default = "default_refresh_interval")]
    pub refresh_interval: u64,

    /// Enable CSV/JSON export (default: true)
    #[serde(default = "default_export_enabled")]
    pub export_enabled: bool,

    /// Enable session deletion (default: false, dangerous!)
    #[serde(default)]
    pub delete_enabled: bool,

    /// Enable HTTP request logging for UI endpoints (default: false)
    #[serde(default)]
    pub log_requests: bool,

    /// Path to sessions directory (for raw JSONL access)
    #[serde(default = "default_sessions_dir")]
    pub sessions_dir: Option<std::path::PathBuf>,
}

fn default_enabled() -> bool {
    true
}
fn default_host() -> String {
    "127.0.0.1".to_string()
}
fn default_port() -> u16 {
    8082
}
fn default_refresh_interval() -> u64 {
    5
}
fn default_export_enabled() -> bool {
    true
}
fn default_sessions_dir() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from("~/.lunaroute/sessions"))
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            host: default_host(),
            port: default_port(),
            refresh_interval: default_refresh_interval(),
            export_enabled: default_export_enabled(),
            delete_enabled: false,
            log_requests: false,
            sessions_dir: default_sessions_dir(),
        }
    }
}

/// UI Server
pub struct UiServer {
    config: UiConfig,
    db: Arc<SqlitePool>,
}

impl UiServer {
    /// Create a new UI server
    pub fn new(mut config: UiConfig, db: Arc<SqlitePool>) -> Self {
        // Expand tilde in sessions_dir path
        if let Some(ref path) = config.sessions_dir {
            config.sessions_dir = Some(expand_tilde(path));
        }

        Self { config, db }
    }

    /// Build the Axum router with all routes
    fn build_router(&self) -> Router {
        let state = AppState {
            db: self.db.clone(),
            config: self.config.clone(),
        };

        let router = Router::new()
            // HTML pages
            .route("/", get(handlers::dashboard::dashboard))
            .route("/sessions", get(handlers::sessions::sessions_list))
            .route("/sessions/{id}", get(handlers::sessions::session_detail))
            .route("/analytics", get(handlers::analytics::analytics))
            .route("/settings", get(handlers::settings::settings))
            // Static assets (embedded in binary)
            .route(
                "/static/css/style.css",
                get(handlers::static_files::serve_css),
            )
            .route(
                "/static/js/app.js",
                get(handlers::static_files::serve_app_js),
            )
            .route(
                "/static/js/charts.js",
                get(handlers::static_files::serve_charts_js),
            )
            // JSON API endpoints
            .route("/api/stats/overview", get(handlers::api::overview_stats))
            .route("/api/stats/tokens", get(handlers::api::token_stats))
            .route("/api/stats/tools", get(handlers::api::tool_stats))
            .route("/api/stats/costs", get(handlers::api::cost_stats))
            .route("/api/stats/models", get(handlers::api::model_stats))
            .route("/api/stats/hours", get(handlers::api::hour_of_day_stats))
            .route("/api/stats/spending", get(handlers::api::spending_stats))
            .route("/api/sessions", get(handlers::api::sessions_list))
            .route("/api/sessions/recent", get(handlers::api::recent_sessions))
            .route("/api/sessions/{id}", get(handlers::api::session_detail))
            .route(
                "/api/sessions/{id}/timeline",
                get(handlers::api::session_timeline),
            )
            .route(
                "/api/sessions/{id}/tools",
                get(handlers::api::session_tool_stats),
            )
            .route(
                "/api/sessions/{id}/raw/{request_id}",
                get(handlers::api::session_raw_data),
            )
            .route("/api/user-agents", get(handlers::api::user_agents_list))
            .route("/api/models", get(handlers::api::models_list))
            .with_state(state);

        // Conditionally add request logging layer
        if self.config.log_requests {
            router.layer(TraceLayer::new_for_http())
        } else {
            router
        }
    }

    /// Start the UI server
    pub async fn serve(self) -> anyhow::Result<()> {
        if !self.config.enabled {
            info!("📊 UI server disabled in configuration");
            return Ok(());
        }

        let addr: SocketAddr = format!("{}:{}", self.config.host, self.config.port)
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid address: {}", e))?;

        let router = self.build_router();

        info!("📊 LunaRoute UI server starting on http://{}", addr);
        info!("   Dashboard:  http://{}/", addr);
        info!("   Sessions:   http://{}/sessions", addr);
        info!("   Analytics:  http://{}/analytics", addr);

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, router).await?;

        Ok(())
    }
}

/// Expand tilde (~) in a path to the user's home directory
fn expand_tilde(path: &Path) -> PathBuf {
    if let Some(path_str) = path.to_str() {
        if path_str.starts_with("~/") || path_str == "~" {
            // Use dirs::home_dir() for cross-platform compatibility
            if let Some(home) = dirs::home_dir() {
                let expanded = if path_str == "~" {
                    home.clone()
                } else {
                    // Join the home dir with the path after "~/"
                    home.join(&path_str[2..])
                };

                // Try to canonicalize, but if path doesn't exist yet, return expanded
                return expanded.canonicalize().unwrap_or(expanded);
            }
        }
    }
    path.to_path_buf()
}
