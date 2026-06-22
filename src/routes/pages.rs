use std::collections::HashMap;

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use serde::Deserialize;
use time::{Duration, OffsetDateTime};

use crate::auth::{AuthUser, MaybeUser, SessionUser};
use crate::db;
use crate::db::memos::{Feed, MemoQuery};
use crate::error::{AppError, render};
use crate::models::TagCount;
use crate::state::AppState;
use crate::views::{ActivityGrid, MemoView, activity_grid, memo_view, memo_views};

pub const PAGE_SIZE: i64 = 20;

#[derive(Template)]
#[template(path = "index.html")]
struct IndexPage {
    username: String,
    is_admin: bool,
    csrf_token: String,
    nav_active: &'static str,
    counts: crate::models::NoteCounts,
    sections: Vec<crate::models::Section>,
    tags: Vec<TagCount>,
    tags_label: String,
    tags_path: String,
    tag_filter: Option<String>,
    q: Option<String>,
    show_composer: bool,
    composer_section: Option<i64>,
    upload_limit: String,
    feed_title: String,
    feed_path: String,
    date_filter: Option<String>,
    activity: Option<ActivityGrid>,
    pinned: Vec<MemoView>,
    memos: Vec<MemoView>,
    next_before: Option<String>,
}

#[derive(Template)]
#[template(path = "partials/memo_list.html")]
struct MemoListFrag {
    pinned: Vec<MemoView>,
    memos: Vec<MemoView>,
    next_before: Option<String>,
    tag_filter: Option<String>,
    q: Option<String>,
    date_filter: Option<String>,
    feed_path: String,
}

#[derive(Template)]
#[template(path = "partials/memo_feed_items.html")]
struct FeedItemsFrag {
    memos: Vec<MemoView>,
    next_before: Option<String>,
    tag_filter: Option<String>,
    q: Option<String>,
    date_filter: Option<String>,
    feed_path: String,
}

#[derive(Template)]
#[template(path = "memo_page.html")]
struct SingleMemoPage {
    m: MemoView,
    page_title: String,
}

#[derive(Deserialize)]
pub struct FeedParams {
    tag: Option<String>,
    q: Option<String>,
    /// `YYYY-MM` (month picker) or `YYYY-MM-DD` (heatmap cell) date filter.
    date: Option<String>,
    before: Option<String>,
}

struct FeedCfg {
    nav: &'static str,
    title: String,
    path: String,
    composer: bool,
    /// Show the activity heatmap + month picker (the owner's own timeline only).
    activity: bool,
    /// Which bucket's tags the sidebar lists, plus its label and link base.
    tag_scope: db::memos::TagScope,
    tags_label: String,
    tags_path: String,
    /// Set on a section feed, for nav highlight + seeding the composer.
    active_section: Option<i64>,
}

fn parse_before(s: &str) -> Option<(i64, i64)> {
    let (ts, id) = s.split_once(',')?;
    Some((ts.parse().ok()?, id.parse().ok()?))
}

/// Turn a `YYYY-MM` or `YYYY-MM-DD` filter into a half-open `[start, end)`
/// created_at window in unix seconds (UTC). Returns `None` if malformed.
fn parse_date_range(s: &str) -> Option<(i64, i64)> {
    use time::{Date, Month, Time};

    let mut parts = s.split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month = Month::try_from(parts.next()?.parse::<u8>().ok()?).ok()?;
    let day_part = parts.next();
    if parts.next().is_some() {
        return None;
    }

    let (start, end) = match day_part {
        None => {
            let start = Date::from_calendar_date(year, month, 1).ok()?;
            let (ny, nm) = match month {
                Month::December => (year + 1, Month::January),
                other => (year, other.next()),
            };
            let end = Date::from_calendar_date(ny, nm, 1).ok()?;
            (start, end)
        }
        Some(d) => {
            let start = Date::from_calendar_date(year, month, d.parse().ok()?).ok()?;
            (start, start + time::Duration::days(1))
        }
    };

    let to_ts = |d: Date| d.with_time(Time::MIDNIGHT).assume_utc().unix_timestamp();
    Some((to_ts(start), to_ts(end)))
}

pub async fn home(
    State(state): State<AppState>,
    AuthUser(session): AuthUser,
    headers: HeaderMap,
    Query(p): Query<FeedParams>,
) -> Result<Response, AppError> {
    let feed = Feed::Own(session.user.id);
    feed_response(
        &state,
        &session,
        &headers,
        p,
        feed,
        FeedCfg {
            nav: "home",
            title: "Home".into(),
            path: "/".into(),
            composer: true,
            activity: true,
            tag_scope: db::memos::TagScope::Home,
            tags_label: "Home".into(),
            tags_path: "/".into(),
            active_section: None,
        },
    )
    .await
}

pub async fn explore(
    State(state): State<AppState>,
    AuthUser(session): AuthUser,
    headers: HeaderMap,
    Query(p): Query<FeedParams>,
) -> Result<Response, AppError> {
    feed_response(
        &state,
        &session,
        &headers,
        p,
        Feed::Explore { signed_in: true },
        FeedCfg {
            nav: "explore",
            title: "Explore".into(),
            path: "/explore".into(),
            composer: false,
            activity: false,
            tag_scope: db::memos::TagScope::Home,
            tags_label: "Home".into(),
            tags_path: "/".into(),
            active_section: None,
        },
    )
    .await
}

pub async fn archive(
    State(state): State<AppState>,
    AuthUser(session): AuthUser,
    headers: HeaderMap,
    Query(p): Query<FeedParams>,
) -> Result<Response, AppError> {
    let feed = Feed::Archive(session.user.id);
    feed_response(
        &state,
        &session,
        &headers,
        p,
        feed,
        FeedCfg {
            nav: "archive",
            title: "Archive".into(),
            path: "/archive".into(),
            composer: false,
            activity: false,
            tag_scope: db::memos::TagScope::Home,
            tags_label: "Home".into(),
            tags_path: "/".into(),
            active_section: None,
        },
    )
    .await
}

pub async fn imported(
    State(state): State<AppState>,
    AuthUser(session): AuthUser,
    headers: HeaderMap,
    Query(p): Query<FeedParams>,
) -> Result<Response, AppError> {
    let feed = Feed::Imported(session.user.id);
    feed_response(
        &state,
        &session,
        &headers,
        p,
        feed,
        FeedCfg {
            nav: "imported",
            title: "Imported".into(),
            path: "/imported".into(),
            composer: false,
            activity: false,
            tag_scope: db::memos::TagScope::Imported,
            tags_label: "Imported".into(),
            tags_path: "/imported".into(),
            active_section: None,
        },
    )
    .await
}

pub async fn section(
    State(state): State<AppState>,
    AuthUser(session): AuthUser,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(p): Query<FeedParams>,
) -> Result<Response, AppError> {
    // 404 if the section isn't the viewer's.
    let name = db::sections::name(&state.pool, session.user.id, id).await?;
    let feed = Feed::Section { user_id: session.user.id, section_id: id };
    feed_response(
        &state,
        &session,
        &headers,
        p,
        feed,
        FeedCfg {
            nav: "section",
            title: name.clone(),
            path: format!("/s/{id}"),
            composer: true,
            activity: false,
            tag_scope: db::memos::TagScope::Section(id),
            tags_label: name,
            tags_path: format!("/s/{id}"),
            active_section: Some(id),
        },
    )
    .await
}

async fn feed_response(
    state: &AppState,
    session: &SessionUser,
    headers: &HeaderMap,
    p: FeedParams,
    feed: Feed,
    cfg: FeedCfg,
) -> Result<Response, AppError> {
    let tag = p
        .tag
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty());
    let q = p
        .q
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    // Keep the date filter only if it parses, so the UI never echoes garbage.
    let date = p
        .date
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && parse_date_range(s).is_some());
    let created_range = date.as_deref().and_then(parse_date_range);
    let before = p.before.as_deref().and_then(parse_before);

    let page = db::memos::list(
        &state.pool,
        MemoQuery {
            feed,
            tag: tag.clone(),
            search: q.clone(),
            created_range,
            before,
            limit: PAGE_SIZE,
        },
    )
    .await?;

    let next_before = if page.has_more {
        page.memos
            .last()
            .map(|m| format!("{},{}", m.created_at, m.id))
    } else {
        None
    };
    let viewer = Some(&session.user);
    let memos = memo_views(&state.pool, &page.memos, viewer).await?;
    let pinned = memo_views(&state.pool, &page.pinned, viewer).await?;

    let is_fragment = headers.contains_key("hx-request") && !headers.contains_key("hx-boosted");
    if is_fragment && before.is_some() {
        // Infinite-scroll continuation: just more cards + the next sentinel.
        render(&FeedItemsFrag {
            memos,
            next_before,
            tag_filter: tag,
            q,
            date_filter: date,
            feed_path: cfg.path,
        })
    } else if is_fragment {
        // Live search / filter swap of the whole list.
        render(&MemoListFrag {
            pinned,
            memos,
            next_before,
            tag_filter: tag,
            q,
            date_filter: date,
            feed_path: cfg.path,
        })
    } else {
        let tags = db::memos::tag_counts(&state.pool, session.user.id, cfg.tag_scope).await?;
        let nav_counts = db::memos::note_counts(&state.pool, session.user.id).await?;
        let mut sections = db::sections::list(&state.pool, session.user.id).await?;
        if let Some(active) = cfg.active_section {
            for s in sections.iter_mut() {
                s.active = s.id == active;
            }
        }
        // Heatmap over roughly the last year of the owner's own notes.
        let activity = if cfg.activity {
            let since = (OffsetDateTime::now_utc() - Duration::days(400)).unix_timestamp();
            let counts: HashMap<String, i64> =
                db::memos::activity_since(&state.pool, session.user.id, since)
                    .await?
                    .into_iter()
                    .collect();
            Some(activity_grid(&counts, OffsetDateTime::now_utc().date()))
        } else {
            None
        };
        render(&IndexPage {
            username: session.user.username.clone(),
            is_admin: session.user.is_admin(),
            csrf_token: session.csrf_token.clone(),
            nav_active: cfg.nav,
            counts: nav_counts,
            sections,
            tags,
            tags_label: cfg.tags_label,
            tags_path: cfg.tags_path,
            tag_filter: tag,
            q,
            show_composer: cfg.composer,
            composer_section: cfg.active_section,
            upload_limit: crate::views::upload_limit_label(state.config.max_upload_mb),
            feed_title: cfg.title,
            feed_path: cfg.path,
            date_filter: date,
            activity,
            pinned,
            memos,
            next_before,
        })
    }
}

/// `/m/:uid` — the permalink doubles as the share link. 404 (not 403) when the
/// viewer lacks access, so private memo uids can't be probed.
pub async fn memo_page(
    State(state): State<AppState>,
    MaybeUser(maybe): MaybeUser,
    Path(uid): Path<String>,
) -> Result<Response, AppError> {
    let memo = db::memos::get_by_uid(&state.pool, uid).await?;
    let viewer = maybe.as_ref().map(|s| &s.user);
    if !db::memos::can_view_considering_state(&memo, viewer) {
        return Err(AppError::NotFound);
    }
    let attachments = db::resources::for_memos(&state.pool, vec![memo.id])
        .await?
        .remove(&memo.id)
        .unwrap_or_default();
    // Built with viewer=None: the share page is read-only even for the owner.
    let m = memo_view(&memo, None, attachments);
    let page_title = crate::markdown::excerpt(&memo.content, 60);
    render(&SingleMemoPage { m, page_title })
}
