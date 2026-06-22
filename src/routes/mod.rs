pub mod api;
pub mod auth_routes;
pub mod export_routes;
pub mod memo_routes;
pub mod pages;
pub mod resource_routes;
pub mod sections_routes;
pub mod settings_routes;
pub mod statics;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    // 0 means "no cap" — uploads are bounded only by memory and SQLite's blob limit.
    let max_body = match state.config.max_upload_mb {
        0 => usize::MAX,
        mb => mb * 1024 * 1024,
    };
    Router::new()
        .route("/", get(pages::home))
        .route("/explore", get(pages::explore))
        .route("/archive", get(pages::archive))
        .route("/imported", get(pages::imported))
        .route("/s/{id}", get(pages::section))
        .route("/sections", post(sections_routes::create))
        .route("/sections/{id}/delete", post(sections_routes::delete))
        .route("/m/{uid}", get(pages::memo_page))
        .route("/memos", post(memo_routes::create))
        .route("/memos/{id}", put(memo_routes::update).delete(memo_routes::delete))
        .route("/memos/{id}/edit", get(memo_routes::edit_form))
        .route("/memos/{id}/card", get(memo_routes::card))
        .route("/memos/{id}/pin", post(memo_routes::toggle_pin))
        .route("/memos/{id}/archive", post(memo_routes::toggle_archived))
        .route("/resources", post(resource_routes::upload))
        .route("/r/{uid}", get(resource_routes::serve))
        .route("/export", get(export_routes::page).post(export_routes::download))
        .route("/import", post(export_routes::import))
        .route("/import/stream", post(export_routes::import_stream))
        .route("/login", get(auth_routes::login_page).post(auth_routes::login_submit))
        .route(
            "/register",
            get(auth_routes::register_page).post(auth_routes::register_submit),
        )
        .route("/logout", post(auth_routes::logout))
        .route("/settings", get(settings_routes::settings))
        .route("/settings/password", post(settings_routes::change_password))
        .route("/settings/tokens", post(settings_routes::create_token))
        .route(
            "/settings/tokens/{id}/delete",
            post(settings_routes::revoke_token),
        )
        .route("/admin", get(settings_routes::admin))
        .route("/admin/registration", post(settings_routes::set_registration))
        .route("/api/v1/me", get(api::me))
        .route("/api/v1/memos", get(api::list_memos).post(api::create_memo))
        .route(
            "/api/v1/memos/{uid}",
            get(api::get_memo).patch(api::patch_memo).delete(api::delete_memo),
        )
        .route("/api/v1/tags", get(api::tags))
        .route("/healthz", get(healthz))
        .route("/static/{*file}", get(statics::serve))
        // RequestBodyLimitLayer caps the whole stream, but axum's Multipart
        // extractor independently enforces DefaultBodyLimit (2 MiB by default)
        // per field — so raise that to match the configured cap too.
        .layer(RequestBodyLimitLayer::new(max_body))
        .layer(DefaultBodyLimit::max(max_body))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn healthz(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<&'static str, StatusCode> {
    crate::db::run(&state.pool, |conn| {
        conn.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
            .map_err(crate::error::AppError::from)
    })
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok("ok")
}
