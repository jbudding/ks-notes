use std::collections::HashMap;

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use time::macros::format_description;

use crate::models::{Memo, MemoState, ResourceMeta, User};

/// Template-ready memo: markdown pre-rendered, times pre-formatted,
/// permissions resolved against the viewer.
pub struct MemoView {
    pub id: i64,
    pub uid: String,
    pub username: String,
    pub raw: String,
    pub html: String,
    pub visibility: &'static str,
    pub pinned: bool,
    pub archived: bool,
    pub created_iso: String,
    pub created_display: String,
    pub can_edit: bool,
    pub attachments: Vec<AttachmentView>,
}

pub struct AttachmentView {
    pub uid: String,
    pub filename: String,
    pub is_image: bool,
}

pub fn format_times(ts: i64) -> (String, String) {
    let dt = OffsetDateTime::from_unix_timestamp(ts).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    let iso = dt.format(&Rfc3339).unwrap_or_default();
    // UTC fallback shown until app.js rewrites it to the viewer's locale.
    let display = dt
        .format(format_description!("[year]-[month]-[day] [hour]:[minute] UTC"))
        .unwrap_or_default();
    (iso, display)
}

pub fn memo_view(memo: &Memo, viewer: Option<&User>, attachments: Vec<ResourceMeta>) -> MemoView {
    let (created_iso, created_display) = format_times(memo.created_at);
    MemoView {
        id: memo.id,
        uid: memo.uid.clone(),
        username: memo.username.clone(),
        raw: memo.content.clone(),
        html: crate::markdown::render(&memo.content),
        visibility: memo.visibility.as_str(),
        pinned: memo.pinned,
        archived: memo.state == MemoState::Archived,
        created_iso,
        created_display,
        can_edit: viewer.map(|u| u.id == memo.user_id).unwrap_or(false),
        attachments: attachments
            .into_iter()
            .map(|r| AttachmentView {
                uid: r.uid,
                filename: r.filename,
                is_image: r.content_type.starts_with("image/"),
            })
            .collect(),
    }
}

/// Batch-build views for a feed page, fetching all attachments in one query.
pub async fn memo_views(
    pool: &deadpool_sqlite::Pool,
    memos: &[Memo],
    viewer: Option<&User>,
) -> Result<Vec<MemoView>, crate::error::AppError> {
    let ids: Vec<i64> = memos.iter().map(|m| m.id).collect();
    let mut attachments: HashMap<i64, Vec<ResourceMeta>> =
        crate::db::resources::for_memos(pool, ids).await?;
    Ok(memos
        .iter()
        .map(|m| memo_view(m, viewer, attachments.remove(&m.id).unwrap_or_default()))
        .collect())
}
