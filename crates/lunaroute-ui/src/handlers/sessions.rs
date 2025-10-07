//! Session list and detail handlers

use askama::Template;
use axum::{extract::{Path, State}, response::Html};
use crate::AppState;

#[derive(Template)]
#[template(path = "sessions_list.html")]
struct SessionsListTemplate {
    refresh_interval: u64,
}

pub async fn sessions_list(State(state): State<AppState>) -> Html<String> {
    let template = SessionsListTemplate {
        refresh_interval: state.config.refresh_interval,
    };
    Html(template.render().unwrap())
}

#[derive(Template)]
#[template(path = "session_detail.html")]
struct SessionDetailTemplate {
    session_id: String,
}

pub async fn session_detail(
    Path(session_id): Path<String>,
    State(_state): State<AppState>,
) -> Html<String> {
    let template = SessionDetailTemplate { session_id };
    Html(template.render().unwrap())
}
