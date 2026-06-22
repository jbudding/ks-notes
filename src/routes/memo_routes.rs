use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;

use crate::auth::{AuthUser, CsrfGuard};
use crate::db;
use crate::error::{AppError, render};
use crate::db::sections::MoveDest;
use crate::models::{Memo, Section, TagCount, Visibility};
use crate::state::AppState;
use crate::views::{MemoView, memo_view};

#[derive(Template)]
#[template(path = "partials/memo_card.html")]
struct MemoCard {
    m: MemoView,
}

#[derive(Template)]
#[template(path = "partials/memo_edit_form.html")]
struct MemoEditForm {
    m: MemoView,
    upload_limit: String,
    sections: Vec<Section>,
}

#[derive(Template)]
#[template(path = "partials/tag_sidebar.html")]
struct TagSidebarOob {
    tags: Vec<TagCount>,
    tags_label: String,
    tags_path: String,
    tag_filter: Option<String>,
    oob: bool,
}

#[derive(Deserialize)]
pub struct MemoForm {
    content: String,
    visibility: Option<String>,
    /// Set by the section-view composer so new notes land in that section.
    section_id: Option<i64>,
    /// Set by the editor's Section select to move an existing note.
    section: Option<String>,
}

/// Attachments are derived from `{{attach:uid}}` tokens in the content itself,
/// so there's no separate resource list to parse here.
fn parse_form(form: &MemoForm) -> Result<(String, Visibility), AppError> {
    let content = form.content.trim().to_string();
    if content.is_empty() {
        return Err(AppError::BadRequest("Memo content can't be empty".into()));
    }
    if content.len() > 100_000 {
        return Err(AppError::BadRequest("Memo is too long".into()));
    }
    let visibility = form
        .visibility
        .as_deref()
        .and_then(Visibility::parse)
        .unwrap_or(Visibility::Private);
    Ok((content, visibility))
}

async fn card_view(state: &AppState, memo: &Memo, viewer: &crate::models::User) -> Result<MemoView, AppError> {
    let attachments = db::resources::for_memos(&state.pool, vec![memo.id])
        .await?
        .remove(&memo.id)
        .unwrap_or_default();
    Ok(memo_view(memo, Some(viewer), attachments))
}

/// Rendered card plus an out-of-band refresh of the tag sidebar.
async fn card_with_sidebar(
    state: &AppState,
    memo: &Memo,
    viewer: &crate::models::User,
) -> Result<Response, AppError> {
    let card = MemoCard {
        m: card_view(state, memo, viewer).await?,
    };
    // Refresh the tag list for the bucket this note belongs to, so the sidebar
    // (Home / a section / Imported) stays consistent after a create or edit.
    let (scope, label, path) = match (memo.origin, memo.section_id) {
        (crate::models::MemoOrigin::Imported, _) => {
            (db::memos::TagScope::Imported, "Imported".to_string(), "/imported".to_string())
        }
        (_, Some(sid)) => {
            let name = db::sections::name(&state.pool, viewer.id, sid).await.unwrap_or_default();
            (db::memos::TagScope::Section(sid), name, format!("/s/{sid}"))
        }
        _ => (db::memos::TagScope::Home, "Home".to_string(), "/".to_string()),
    };
    let sidebar = TagSidebarOob {
        tags: db::memos::tag_counts(&state.pool, viewer.id, scope).await?,
        tags_label: label,
        tags_path: path,
        tag_filter: None,
        oob: true,
    };
    Ok(Html(format!("{}{}", card.render()?, sidebar.render()?)).into_response())
}

pub async fn create(
    State(state): State<AppState>,
    CsrfGuard(session): CsrfGuard,
    Form(form): Form<MemoForm>,
) -> Result<Response, AppError> {
    let section_id = form.section_id;
    let (content, visibility) = parse_form(&form)?;
    let memo =
        db::memos::create(&state.pool, session.user.id, content, visibility, section_id).await?;
    card_with_sidebar(&state, &memo, &session.user).await
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    CsrfGuard(session): CsrfGuard,
    Form(form): Form<MemoForm>,
) -> Result<Response, AppError> {
    let move_to = form.section.clone();
    let (content, visibility) = parse_form(&form)?;
    db::memos::update(&state.pool, id, session.user.id, content, visibility).await?;
    if let Some(dest) = move_to {
        db::sections::set_note_section(&state.pool, session.user.id, id, MoveDest::parse(&dest))
            .await?;
    }
    // Re-fetch so the card + sidebar reflect any section/origin change.
    let memo = db::memos::get(&state.pool, id).await?;
    card_with_sidebar(&state, &memo, &session.user).await
}

/// Owner-only fetch used by the edit/cancel fragment flows.
async fn own_memo(state: &AppState, id: i64, user_id: i64) -> Result<Memo, AppError> {
    let memo = db::memos::get(&state.pool, id).await?;
    if memo.user_id != user_id {
        return Err(AppError::NotFound);
    }
    Ok(memo)
}

pub async fn edit_form(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    AuthUser(session): AuthUser,
) -> Result<Response, AppError> {
    let memo = own_memo(&state, id, session.user.id).await?;
    let mut sections = db::sections::list(&state.pool, session.user.id).await?;
    for s in sections.iter_mut() {
        s.active = memo.section_id == Some(s.id);
    }
    render(&MemoEditForm {
        m: card_view(&state, &memo, &session.user).await?,
        upload_limit: crate::views::upload_limit_label(state.config.max_upload_mb),
        sections,
    })
}

pub async fn card(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    AuthUser(session): AuthUser,
) -> Result<Response, AppError> {
    let memo = own_memo(&state, id, session.user.id).await?;
    render(&MemoCard {
        m: card_view(&state, &memo, &session.user).await?,
    })
}

pub async fn toggle_pin(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    CsrfGuard(session): CsrfGuard,
) -> Result<Response, AppError> {
    let memo = db::memos::toggle_pin(&state.pool, id, session.user.id).await?;
    render(&MemoCard {
        m: card_view(&state, &memo, &session.user).await?,
    })
}

/// Toggling archive removes the card from whichever feed it's in.
pub async fn toggle_archived(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    CsrfGuard(session): CsrfGuard,
) -> Result<Response, AppError> {
    db::memos::toggle_archived(&state.pool, id, session.user.id).await?;
    Ok(StatusCode::OK.into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    CsrfGuard(session): CsrfGuard,
) -> Result<Response, AppError> {
    db::memos::delete(&state.pool, id, session.user.id).await?;
    Ok(StatusCode::OK.into_response())
}
