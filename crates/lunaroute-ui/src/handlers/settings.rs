//! Settings page handler

use crate::AppState;
use askama::Template;
use axum::{extract::State, response::Html};

#[derive(Template)]
#[template(path = "settings.html")]
struct SettingsTemplate {
    refresh_interval: u64,
    export_enabled: bool,
    delete_enabled: bool,
}

pub async fn settings(State(state): State<AppState>) -> Html<String> {
    let template = SettingsTemplate {
        refresh_interval: state.config.refresh_interval,
        export_enabled: state.config.export_enabled,
        delete_enabled: state.config.delete_enabled,
    };
    Html(template.render().unwrap())
}
