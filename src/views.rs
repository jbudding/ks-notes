use std::collections::HashMap;

use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::{Date, Duration, Month, OffsetDateTime};

use crate::db::memos::LinkTarget;
use crate::models::{Memo, MemoState, ResourceMeta, User, Visibility};

/// Template-ready memo: markdown pre-rendered, times pre-formatted,
/// permissions resolved against the viewer.
pub struct MemoView {
    pub id: i64,
    pub uid: String,
    pub uuid: String,
    /// The `{{memo:UUID}}` markup that links to this note, for the "Ref" copy button.
    pub link_token: String,
    pub username: String,
    pub raw: String,
    pub html: String,
    pub visibility: &'static str,
    pub pinned: bool,
    pub archived: bool,
    pub created_iso: String,
    pub created_display: String,
    pub can_edit: bool,
    pub section_id: Option<i64>,
    pub is_imported: bool,
}

/// Human-readable attachment size limit for the composer's paperclip tooltip.
/// `0` means uncapped (see `Config::max_upload_mb`).
pub fn upload_limit_label(max_upload_mb: usize) -> String {
    if max_upload_mb == 0 {
        "no size limit".into()
    } else {
        format!("max {max_upload_mb} MB")
    }
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

/// Whether a viewer may see a link target's title, mirroring `db::memos::can_view`
/// but working from the bare visibility + author id we resolved for the link.
fn link_viewable(visibility: Visibility, author_id: i64, viewer: Option<&User>) -> bool {
    match visibility {
        Visibility::Public => true,
        Visibility::Protected => viewer.is_some(),
        Visibility::Private => viewer.map(|u| u.id == author_id).unwrap_or(false),
    }
}

pub fn memo_view(
    memo: &Memo,
    viewer: Option<&User>,
    attachments: Vec<ResourceMeta>,
    link_targets: &HashMap<(i64, String), LinkTarget>,
) -> MemoView {
    let (created_iso, created_display) = format_times(memo.created_at);
    let inline: Vec<crate::markdown::InlineAttachment> = attachments
        .into_iter()
        .map(|r| crate::markdown::InlineAttachment {
            uid: r.uid,
            filename: r.filename,
            is_image: r.content_type.starts_with("image/"),
        })
        .collect();
    // Resolve `{{memo:UUID}}` tokens against this author's notebook. Unresolved
    // refs are dropped here and rendered as broken placeholders by the markdown pass.
    let links: Vec<crate::markdown::InlineMemoLink> = crate::markdown::extract_memo_refs(&memo.content)
        .into_iter()
        .filter_map(|uuid| {
            link_targets.get(&(memo.user_id, uuid.clone())).map(|t| {
                let viewable = link_viewable(t.visibility, memo.user_id, viewer);
                crate::markdown::InlineMemoLink {
                    uuid,
                    uid: t.uid.clone(),
                    title: if viewable { crate::markdown::excerpt(&t.content, 60) } else { String::new() },
                    viewable,
                }
            })
        })
        .collect();
    MemoView {
        id: memo.id,
        uid: memo.uid.clone(),
        uuid: memo.uuid.clone(),
        link_token: format!("{{{{memo:{}}}}}", memo.uuid),
        username: memo.username.clone(),
        raw: memo.content.clone(),
        html: crate::markdown::render_with_inlines(&memo.content, &inline, &links),
        visibility: memo.visibility.as_str(),
        pinned: memo.pinned,
        archived: memo.state == MemoState::Archived,
        created_iso,
        created_display,
        can_edit: viewer.map(|u| u.id == memo.user_id).unwrap_or(false),
        section_id: memo.section_id,
        is_imported: memo.origin == crate::models::MemoOrigin::Imported,
    }
}

// ---------- Activity heatmap ----------

/// One day square in the GitHub-style contribution grid.
pub struct ActivityCell {
    /// `YYYY-MM-DD`, or empty for trailing future-day padding cells.
    pub date: String,
    pub count: i64,
    /// Intensity bucket 0–4, mapped to colour in CSS.
    pub level: u8,
    /// Hover tooltip text (empty for padding cells).
    pub label: String,
}

pub struct MonthLabel {
    /// Three-letter abbreviation at a column where a new month begins, else empty.
    pub name: &'static str,
}

pub struct ActivityGrid {
    /// Column-major: one week per column, 7 days (Sun→Sat) each.
    pub cells: Vec<ActivityCell>,
    /// One entry per week column, aligned with `cells`.
    pub months: Vec<MonthLabel>,
    pub weeks: usize,
    pub total: i64,
}

fn level_of(count: i64) -> u8 {
    match count {
        0 => 0,
        1 => 1,
        2..=3 => 2,
        4..=6 => 3,
        _ => 4,
    }
}

fn month_abbr(m: Month) -> &'static str {
    use Month::*;
    match m {
        January => "Jan",
        February => "Feb",
        March => "Mar",
        April => "Apr",
        May => "May",
        June => "Jun",
        July => "Jul",
        August => "Aug",
        September => "Sep",
        October => "Oct",
        November => "Nov",
        December => "Dec",
    }
}

/// Build ~53 weeks of activity squares ending on `today`, indexed by `YYYY-MM-DD`
/// counts. Weeks run Sunday→Saturday; the final week is padded with empty future
/// cells so every column has 7 rows.
pub fn activity_grid(counts: &HashMap<String, i64>, today: Date) -> ActivityGrid {
    // Step back ~a year, then to the Sunday that starts that week.
    let approx_start = today - Duration::weeks(52);
    let back = approx_start.weekday().number_days_from_sunday() as i64;
    let mut week_start = approx_start - Duration::days(back);

    let mut cells = Vec::new();
    let mut months = Vec::new();
    let mut total = 0i64;
    let mut last_month: Option<Month> = None;

    loop {
        // Label this column only when its week introduces a new month.
        let m = week_start.month();
        months.push(MonthLabel {
            name: if last_month != Some(m) {
                last_month = Some(m);
                month_abbr(m)
            } else {
                ""
            },
        });

        for i in 0..7 {
            let day = week_start + Duration::days(i);
            if day > today {
                cells.push(ActivityCell {
                    date: String::new(),
                    count: 0,
                    level: 0,
                    label: String::new(),
                });
                continue;
            }
            let key = format!("{:04}-{:02}-{:02}", day.year(), u8::from(day.month()), day.day());
            let count = counts.get(&key).copied().unwrap_or(0);
            total += count;
            let label = format!(
                "{count} {} on {key}",
                if count == 1 { "note" } else { "notes" }
            );
            cells.push(ActivityCell {
                date: key,
                count,
                level: level_of(count),
                label,
            });
        }

        week_start += Duration::days(7);
        if week_start > today {
            break;
        }
    }

    let weeks = months.len();
    ActivityGrid {
        cells,
        months,
        weeks,
        total,
    }
}

/// Collect every `(author_user_id, uuid)` note-link pair referenced across `memos`.
fn link_ref_pairs(memos: &[Memo]) -> Vec<(i64, String)> {
    memos
        .iter()
        .flat_map(|m| crate::markdown::extract_memo_refs(&m.content).into_iter().map(|u| (m.user_id, u)))
        .collect()
}

/// Batch-build views for a feed page, fetching all attachments and note-link
/// targets in one query each.
pub async fn memo_views(
    pool: &deadpool_sqlite::Pool,
    memos: &[Memo],
    viewer: Option<&User>,
) -> Result<Vec<MemoView>, crate::error::AppError> {
    let ids: Vec<i64> = memos.iter().map(|m| m.id).collect();
    let mut attachments: HashMap<i64, Vec<ResourceMeta>> =
        crate::db::resources::for_memos(pool, ids).await?;
    let targets = crate::db::memos::link_targets(pool, link_ref_pairs(memos)).await?;
    Ok(memos
        .iter()
        .map(|m| memo_view(m, viewer, attachments.remove(&m.id).unwrap_or_default(), &targets))
        .collect())
}

/// Build a single memo's view, fetching its attachments and resolving its note links.
pub async fn single_memo_view(
    pool: &deadpool_sqlite::Pool,
    memo: &Memo,
    viewer: Option<&User>,
) -> Result<MemoView, crate::error::AppError> {
    let attachments = crate::db::resources::for_memos(pool, vec![memo.id])
        .await?
        .remove(&memo.id)
        .unwrap_or_default();
    let targets = crate::db::memos::link_targets(pool, link_ref_pairs(std::slice::from_ref(memo))).await?;
    Ok(memo_view(memo, viewer, attachments, &targets))
}
