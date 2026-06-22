use askama::Template;
use axum::Form;
use axum::body::{Body, Bytes};
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
    counts: crate::models::NoteCounts,
    tags: Vec<TagCount>,
    tags_scope: &'static str,
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
        counts: db::memos::note_counts(&state.pool, session.user.id).await?,
        tags: db::memos::tag_counts(&state.pool, session.user.id, crate::models::MemoOrigin::Local).await?,
        tags_scope: "home",
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

/// Segmented import: the browser streams the file up (so it never loads the
/// whole thing into memory), and we stream back one NDJSON line per note —
/// `{i, total, uuid, status}` — so the client shows live "n of X" progress with
/// each note's added/merged/skipped result. Notes are processed one at a time.
pub async fn import_stream(
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
                csrf = field.text().await.map_err(|e| AppError::BadRequest(format!("import: {e}")))?;
            }
            Some("overwrite") => {
                overwrite = field.text().await.map_err(|e| AppError::BadRequest(format!("import: {e}")))? == "true";
            }
            Some("file") => {
                data = field.bytes().await.map_err(|e| AppError::BadRequest(format!("import: {e}")))?.to_vec();
            }
            _ => {}
        }
    }
    auth::require_csrf_field(&session, &csrf)?;

    let file: ExportFile = serde_json::from_slice(&data)
        .map_err(|e| AppError::BadRequest(format!("Couldn't read that file as a ks-notes export: {e}")))?;
    let total = file.notes.len();
    let pool = state.pool.clone();
    let uid = session.user.id;

    let stream = futures_util::stream::unfold(
        (pool, file.notes.into_iter(), 0usize),
        move |(pool, mut iter, i)| async move {
            let note = iter.next()?;
            let uuid = note.uuid.clone();
            let status = match db::memos::import_note(&pool, uid, note, overwrite).await {
                Ok(s) => s.as_str(),
                Err(_) => "error",
            };
            let line = format!(
                "{{\"i\":{},\"total\":{},\"uuid\":{},\"status\":\"{}\"}}\n",
                i + 1,
                total,
                serde_json::to_string(&uuid).unwrap_or_else(|_| "\"\"".to_string()),
                status,
            );
            Some((
                Ok::<_, std::io::Error>(Bytes::from(line)),
                (pool, iter, i + 1),
            ))
        },
    );

    Ok((
        [(CONTENT_TYPE, "application/x-ndjson")],
        Body::from_stream(stream),
    )
        .into_response())
}

/// Import a previously exported JSON file. Multipart upload, so the CSRF token
/// is a `csrf_token` part alongside the `file` part. No-JS fallback for the
/// segmented importer; returns the page with a summary.
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
