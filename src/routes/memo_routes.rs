use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;

use crate::auth::{AuthUser, CsrfGuard};
use crate::db;
use crate::error::{AppError, render};
use crate::models::{Memo, TagCount, Visibility};
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
}

#[derive(Template)]
#[template(path = "partials/tag_sidebar.html")]
struct TagSidebarOob {
    tags: Vec<TagCount>,
    tag_filter: Option<String>,
    oob: bool,
}

#[derive(Deserialize)]
pub struct MemoForm {
    content: String,
    visibility: Option<String>,
    /// Space-separated resource uids accumulated by the composer.
    resources: Option<String>,
}

fn parse_form(form: &MemoForm) -> Result<(String, Visibility, Vec<String>), AppError> {
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
    let resources = form
        .resources
        .as_deref()
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    Ok((content, visibility, resources))
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
    let sidebar = TagSidebarOob {
        tags: db::memos::tag_counts(&state.pool, viewer.id).await?,
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
    let (content, visibility, resources) = parse_form(&form)?;
    let memo =
        db::memos::create(&state.pool, session.user.id, content, visibility, resources).await?;
    card_with_sidebar(&state, &memo, &session.user).await
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    CsrfGuard(session): CsrfGuard,
    Form(form): Form<MemoForm>,
) -> Result<Response, AppError> {
    let (content, visibility, resources) = parse_form(&form)?;
    let memo = db::memos::update(
        &state.pool,
        id,
        session.user.id,
        content,
        visibility,
        resources,
    )
    .await?;
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
    render(&MemoEditForm {
        m: card_view(&state, &memo, &session.user).await?,
        upload_limit: crate::views::upload_limit_label(state.config.max_upload_mb),
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
