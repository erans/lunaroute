//! Dashboard page handler

use askama::Template;
use axum::{extract::State, response::Html};
use crate::AppState;

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {
    refresh_interval: u64,
}

pub async fn dashboard(State(state): State<AppState>) -> Html<String> {
    let template = DashboardTemplate {
        refresh_interval: state.config.refresh_interval,
    };
    Html(template.render().unwrap())
}
