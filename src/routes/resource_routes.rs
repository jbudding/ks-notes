use askama::Template;
use axum::extract::{Multipart, Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::response::{Html, IntoResponse, Response};

use crate::auth::{CsrfGuard, MaybeUser};
use crate::db;
use crate::error::AppError;
use crate::state::AppState;

#[derive(Template)]
#[template(path = "partials/upload_chip.html")]
struct UploadChip {
    uid: String,
    filename: String,
}

fn sanitize_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("file");
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '"' | ';'))
        .take(120)
        .collect();
    if cleaned.trim().is_empty() {
        "file".into()
    } else {
        cleaned
    }
}

/// Content types we let the browser render inline. Everything else downloads
/// as an attachment so user uploads (e.g. HTML/SVG) can't run scripts here.
fn inline_safe(content_type: &str) -> bool {
    matches!(
        content_type,
        "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "image/avif"
            | "video/mp4"
            | "video/webm"
            | "audio/mpeg"
            | "audio/ogg"
            | "audio/wav"
            | "application/pdf"
            | "text/plain"
    )
}

/// Composer/editor upload: stores each file and returns one chip fragment per
/// file; app.js folds the chip uids into the form's hidden `resources` field.
pub async fn upload(
    State(state): State<AppState>,
    CsrfGuard(session): CsrfGuard,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    let mut chips = String::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("upload: {e}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let filename = sanitize_filename(field.file_name().unwrap_or("file"));
        let content_type = field
            .content_type()
            .unwrap_or("application/octet-stream")
            .to_string();
        let data = field
            .bytes()
            .await
            .map_err(|e| AppError::BadRequest(format!("upload: {e}")))?
            .to_vec();
        if data.is_empty() {
            continue;
        }
        let meta =
            db::resources::insert(&state.pool, session.user.id, filename, content_type, data)
                .await?;
        chips.push_str(
            &UploadChip {
                uid: meta.uid,
                filename: meta.filename,
            }
            .render()?,
        );
    }
    if chips.is_empty() {
        return Err(AppError::BadRequest("No files received".into()));
    }
    Ok(Html(chips).into_response())
}

/// Serve a stored blob. Access follows the owning memo's visibility; an
/// unattached upload is visible only to its uploader.
pub async fn serve(
    State(state): State<AppState>,
    MaybeUser(maybe): MaybeUser,
    Path(uid): Path<String>,
) -> Result<Response, AppError> {
    let blob = db::resources::get_blob(&state.pool, uid)
        .await?
        .ok_or(AppError::NotFound)?;
    let viewer = maybe.as_ref().map(|s| &s.user);
    let allowed = match blob.memo_id {
        Some(memo_id) => {
            let memo = db::memos::get(&state.pool, memo_id).await?;
            db::memos::can_view_considering_state(&memo, viewer)
        }
        None => viewer.map(|u| u.id == blob.owner_id).unwrap_or(false),
    };
    if !allowed {
        return Err(AppError::NotFound);
    }

    let (content_type, disposition) = if inline_safe(&blob.meta.content_type) {
        (
            blob.meta.content_type.clone(),
            format!("inline; filename=\"{}\"", blob.meta.filename),
        )
    } else {
        (
            "application/octet-stream".to_string(),
            format!("attachment; filename=\"{}\"", blob.meta.filename),
        )
    };
    Ok((
        [
            (CONTENT_TYPE, content_type),
            (CONTENT_DISPOSITION, disposition),
            (CACHE_CONTROL, "private, max-age=86400".to_string()),
            (
                axum::http::header::X_CONTENT_TYPE_OPTIONS,
                "nosniff".to_string(),
            ),
        ],
        blob.data,
    )
        .into_response())
}
