use axum::extract::Path;
use axum::http::StatusCode;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};

/// All assets are embedded at compile time — the binary is fully self-contained.
pub async fn serve(Path(file): Path<String>) -> Response {
    let (bytes, mime): (&'static [u8], &'static str) = match file.as_str() {
        "htmx.min.js" => (
            include_bytes!("../../static/htmx.min.js"),
            "application/javascript",
        ),
        "app.css" => (include_bytes!("../../static/app.css"), "text/css"),
        "app.js" => (include_bytes!("../../static/app.js"), "application/javascript"),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    (
        [
            (CONTENT_TYPE, mime),
            (CACHE_CONTROL, "public, max-age=86400"),
        ],
        bytes,
    )
        .into_response()
}
