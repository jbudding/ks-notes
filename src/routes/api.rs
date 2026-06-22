use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::ApiUser;
use crate::db;
use crate::db::memos::{Feed, MemoQuery};
use crate::error::AppError;
use crate::models::{Memo, MemoState, Visibility};
use crate::state::AppState;

/// JSON-flavored error wrapper: same statuses as AppError, `{"error": …}` body.
pub struct ApiError(AppError);

impl From<AppError> for ApiError {
    fn from(e: AppError) -> Self {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if let AppError::Internal(detail) = &self.0 {
            tracing::error!(detail, "internal error (api)");
        }
        (
            self.0.status(),
            Json(json!({"error": self.0.public_message()})),
        )
            .into_response()
    }
}

#[derive(Serialize)]
pub struct ApiMemo {
    uid: String,
    content: String,
    visibility: &'static str,
    pinned: bool,
    state: &'static str,
    created_at: i64,
    updated_at: i64,
}

impl From<Memo> for ApiMemo {
    fn from(m: Memo) -> Self {
        ApiMemo {
            uid: m.uid,
            content: m.content,
            visibility: m.visibility.as_str(),
            pinned: m.pinned,
            state: m.state.as_str(),
            created_at: m.created_at,
            updated_at: m.updated_at,
        }
    }
}

pub async fn me(ApiUser(user): ApiUser) -> Json<serde_json::Value> {
    Json(json!({
        "id": user.id,
        "username": user.username,
        "role": user.role.as_str(),
    }))
}

#[derive(Deserialize)]
pub struct ListParams {
    q: Option<String>,
    tag: Option<String>,
    state: Option<String>,
    limit: Option<i64>,
    before: Option<String>,
}

pub async fn list_memos(
    State(state): State<AppState>,
    ApiUser(user): ApiUser,
    Query(p): Query<ListParams>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let feed = match p.state.as_deref() {
        Some("archived") => Feed::Archive(user.id),
        _ => Feed::Own(user.id),
    };
    let before = p.before.as_deref().and_then(|s| {
        let (ts, id) = s.split_once(',')?;
        Some((ts.parse().ok()?, id.parse().ok()?))
    });
    let limit = p.limit.unwrap_or(20).clamp(1, 100);
    let page = db::memos::list(
        &state.pool,
        MemoQuery {
            feed,
            tag: p.tag.filter(|t| !t.is_empty()),
            search: p.q.filter(|q| !q.is_empty()),
            created_range: None,
            before,
            limit,
        },
    )
    .await?;
    let has_more = page.has_more;
    let next_before = if has_more {
        page.memos
            .last()
            .map(|m| format!("{},{}", m.created_at, m.id))
    } else {
        None
    };
    // The API has no pinned block; merge pinned rows into one list.
    let memos: Vec<ApiMemo> = page
        .pinned
        .into_iter()
        .chain(page.memos)
        .map(ApiMemo::from)
        .collect();
    Ok(Json(json!({"memos": memos, "has_more": has_more, "next_before": next_before})))
}

#[derive(Deserialize)]
pub struct CreateBody {
    content: String,
    visibility: Option<String>,
}

pub async fn create_memo(
    State(state): State<AppState>,
    ApiUser(user): ApiUser,
    Json(body): Json<CreateBody>,
) -> Result<Response, ApiError> {
    let content = body.content.trim().to_string();
    if content.is_empty() {
        return Err(AppError::BadRequest("content is required".into()).into());
    }
    let visibility = body
        .visibility
        .as_deref()
        .and_then(Visibility::parse)
        .unwrap_or(Visibility::Private);
    let memo = db::memos::create(&state.pool, user.id, content, visibility).await?;
    Ok((StatusCode::CREATED, Json(ApiMemo::from(memo))).into_response())
}

async fn own_memo_by_uid(
    state: &AppState,
    uid: String,
    user_id: i64,
) -> Result<Memo, AppError> {
    let memo = db::memos::get_by_uid(&state.pool, uid).await?;
    if memo.user_id != user_id {
        return Err(AppError::NotFound);
    }
    Ok(memo)
}

pub async fn get_memo(
    State(state): State<AppState>,
    ApiUser(user): ApiUser,
    Path(uid): Path<String>,
) -> Result<Json<ApiMemo>, ApiError> {
    Ok(Json(own_memo_by_uid(&state, uid, user.id).await?.into()))
}

#[derive(Deserialize)]
pub struct PatchBody {
    content: Option<String>,
    visibility: Option<String>,
    pinned: Option<bool>,
    state: Option<String>,
}

pub async fn patch_memo(
    State(state): State<AppState>,
    ApiUser(user): ApiUser,
    Path(uid): Path<String>,
    Json(body): Json<PatchBody>,
) -> Result<Json<ApiMemo>, ApiError> {
    let memo = own_memo_by_uid(&state, uid, user.id).await?;

    if body.content.is_some() || body.visibility.is_some() {
        let content = match body.content {
            Some(c) => {
                let c = c.trim().to_string();
                if c.is_empty() {
                    return Err(AppError::BadRequest("content can't be empty".into()).into());
                }
                c
            }
            None => memo.content.clone(),
        };
        let visibility = body
            .visibility
            .as_deref()
            .and_then(Visibility::parse)
            .unwrap_or(memo.visibility);
        db::memos::update(&state.pool, memo.id, user.id, content, visibility).await?;
    }
    if let Some(pinned) = body.pinned
        && pinned != memo.pinned
    {
        db::memos::toggle_pin(&state.pool, memo.id, user.id).await?;
    }
    if let Some(target) = body.state.as_deref().and_then(MemoState::parse)
        && target != memo.state
    {
        db::memos::toggle_archived(&state.pool, memo.id, user.id).await?;
    }

    Ok(Json(db::memos::get(&state.pool, memo.id).await?.into()))
}

pub async fn delete_memo(
    State(state): State<AppState>,
    ApiUser(user): ApiUser,
    Path(uid): Path<String>,
) -> Result<StatusCode, ApiError> {
    let memo = own_memo_by_uid(&state, uid, user.id).await?;
    db::memos::delete(&state.pool, memo.id, user.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn tags(
    State(state): State<AppState>,
    ApiUser(user): ApiUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let tags = db::memos::tag_counts(&state.pool, user.id, crate::models::MemoOrigin::Local).await?;
    let items: Vec<_> = tags
        .into_iter()
        .map(|t| json!({"tag": t.tag, "count": t.count}))
        .collect();
    Ok(Json(json!({"tags": items})))
}
