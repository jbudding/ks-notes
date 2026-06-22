use askama::Template;
use axum::Form;
use axum::extract::{Multipart, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};

use crate::auth::{self, AuthUser, SessionUser};
use crate::db;
use crate::error::{AppError, render};
use crate::models::{ExportFile, TagCount};
use crate::state::AppState;

#[derive(Template)]
#[template(path = "export.html")]
struct ExportPage {
    username: String,
    is_admin: bool,
    csrf_token: String,
    nav_active: &'static str,
    tags: Vec<TagCount>,
    tag_filter: Option<String>,
    message: Option<String>,
    error: Option<String>,
}

async fn export_page_data(
    state: &AppState,
    session: &SessionUser,
    message: Option<String>,
    error: Option<String>,
) -> Result<ExportPage, AppError> {
    Ok(ExportPage {
        username: session.user.username.clone(),
        is_admin: session.user.is_admin(),
        csrf_token: session.csrf_token.clone(),
        nav_active: "export",
        tags: db::memos::tag_counts(&state.pool, session.user.id).await?,
        tag_filter: None,
        message,
        error,
    })
}

pub async fn page(
    State(state): State<AppState>,
    AuthUser(session): AuthUser,
) -> Result<Response, AppError> {
    render(&export_page_data(&state, &session, None, None).await?)
}

/// Stream the user's notes carrying any of the selected tags as a JSON download.
/// A plain form post, so the CSRF token rides in a `csrf_token` field and the
/// repeated `tags` checkboxes are read from the raw key/value pairs.
pub async fn download(
    State(state): State<AppState>,
    AuthUser(session): AuthUser,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let mut csrf = String::new();
    let mut tags = Vec::new();
    for (key, value) in pairs {
        match key.as_str() {
            "csrf_token" => csrf = value,
            "tags" => tags.push(value),
            _ => {}
        }
    }
    auth::require_csrf_field(&session, &csrf)?;
    if tags.is_empty() {
        return Err(AppError::BadRequest("Select at least one hashtag to export".into()));
    }

    let notes = db::memos::export_by_tags(&state.pool, session.user.id, tags).await?;
    let file = ExportFile { version: 3, notes };
    let json = serde_json::to_string_pretty(&file)
        .map_err(|e| AppError::Internal(format!("serializing export: {e}")))?;

    Ok((
        [
            (CONTENT_TYPE, "application/json".to_string()),
            (
                CONTENT_DISPOSITION,
                "attachment; filename=\"ks-notes-export.json\"".to_string(),
            ),
        ],
        json,
    )
        .into_response())
}

/// Import a previously exported JSON file. Multipart upload, so the CSRF token
/// is a `csrf_token` part alongside the `file` part.
pub async fn import(
    State(state): State<AppState>,
    AuthUser(session): AuthUser,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    let mut csrf = String::new();
    let mut data: Vec<u8> = Vec::new();
    let mut overwrite = false;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("import: {e}")))?
    {
        match field.name() {
            Some("csrf_token") => {
                csrf = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("import: {e}")))?;
            }
            Some("overwrite") => {
                overwrite = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("import: {e}")))?
                    == "true";
            }
            Some("file") => {
                data = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("import: {e}")))?
                    .to_vec();
            }
            _ => {}
        }
    }
    auth::require_csrf_field(&session, &csrf)?;

    let parsed: Result<ExportFile, _> = serde_json::from_slice(&data);
    let page = match parsed {
        Ok(file) => {
            let s = db::memos::import_notes(&state.pool, session.user.id, file.notes, overwrite).await?;
            let msg = format!(
                "Imported {} new, updated {}, skipped {}.",
                s.inserted, s.updated, s.skipped
            );
            export_page_data(&state, &session, Some(msg), None).await?
        }
        Err(e) => {
            let err = format!("Couldn't read that file as a ks-notes export: {e}");
            export_page_data(&state, &session, None, Some(err)).await?
        }
    };
    render(&page)
}
