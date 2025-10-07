//! Analytics page handler

use crate::AppState;
use askama::Template;
use axum::{extract::State, response::Html};

#[derive(Template)]
#[template(path = "analytics.html")]
struct AnalyticsTemplate {
    refresh_interval: u64,
}

pub async fn analytics(State(state): State<AppState>) -> Html<String> {
    let template = AnalyticsTemplate {
        refresh_interval: state.config.refresh_interval * 2, // Analytics refreshes slower
    };
    Html(template.render().unwrap())
}
