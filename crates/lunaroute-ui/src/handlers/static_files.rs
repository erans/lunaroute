//! Static file handlers - embedded in binary

use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};

/// Serve embedded CSS
pub async fn serve_css() -> Response {
    let css = include_str!("../static/css/style.css");
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        css,
    )
        .into_response()
}

/// Serve embedded app.js
pub async fn serve_app_js() -> Response {
    let js = include_str!("../static/js/app.js");
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        js,
    )
        .into_response()
}

/// Serve embedded charts.js
pub async fn serve_charts_js() -> Response {
    let js = include_str!("../static/js/charts.js");
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        js,
    )
        .into_response()
}
