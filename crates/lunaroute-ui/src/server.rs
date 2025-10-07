//! Web UI server implementation

use crate::handlers;
use crate::AppState;
use axum::{
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::net::SocketAddr;
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
}

fn default_enabled() -> bool { true }
fn default_host() -> String { "127.0.0.1".to_string() }
fn default_port() -> u16 { 8082 }
fn default_refresh_interval() -> u64 { 5 }
fn default_export_enabled() -> bool { true }

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            host: default_host(),
            port: default_port(),
            refresh_interval: default_refresh_interval(),
            export_enabled: default_export_enabled(),
            delete_enabled: false,
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
    pub fn new(config: UiConfig, db: Arc<SqlitePool>) -> Self {
        Self { config, db }
    }

    /// Build the Axum router with all routes
    fn build_router(&self) -> Router {
        let state = AppState {
            db: self.db.clone(),
            config: self.config.clone(),
        };

        Router::new()
            // HTML pages
            .route("/", get(handlers::dashboard::dashboard))
            .route("/sessions", get(handlers::sessions::sessions_list))
            .route("/sessions/:id", get(handlers::sessions::session_detail))
            .route("/analytics", get(handlers::analytics::analytics))
            .route("/settings", get(handlers::settings::settings))

            // Static assets (embedded in binary)
            .route("/static/css/style.css", get(handlers::static_files::serve_css))
            .route("/static/js/app.js", get(handlers::static_files::serve_app_js))
            .route("/static/js/charts.js", get(handlers::static_files::serve_charts_js))

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
            .route("/api/sessions/:id", get(handlers::api::session_detail))
            .route("/api/sessions/:id/timeline", get(handlers::api::session_timeline))
            .route("/api/sessions/:id/tools", get(handlers::api::session_tool_stats))
            .route("/api/user-agents", get(handlers::api::user_agents_list))
            .route("/api/models", get(handlers::api::models_list))

            .layer(TraceLayer::new_for_http())
            .with_state(state)
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
