use std::collections::HashMap;

use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::{Date, Duration, Month, OffsetDateTime};

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
