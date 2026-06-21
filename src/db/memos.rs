use deadpool_sqlite::Pool;
use rusqlite::types::Value;
use rusqlite::{Connection, params, params_from_iter};

use crate::error::AppError;
use crate::models::{Memo, MemoState, TagCount, Visibility};

const MEMO_COLS: &str = "m.id, m.uid, m.user_id, u.username, m.content, m.visibility,
                         m.pinned, m.state, m.created_at, m.updated_at";

fn memo_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Memo> {
    Ok(Memo {
        id: r.get(0)?,
        uid: r.get(1)?,
        user_id: r.get(2)?,
        username: r.get(3)?,
        content: r.get(4)?,
        visibility: r.get(5)?,
        pinned: r.get::<_, i64>(6)? != 0,
        state: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

fn get_by_id_sync(conn: &Connection, id: i64) -> Result<Memo, AppError> {
    conn.query_row(
        &format!("SELECT {MEMO_COLS} FROM memos m JOIN users u ON u.id = m.user_id WHERE m.id = ?1"),
        [id],
        memo_from_row,
    )
    .map_err(AppError::from)
}

/// Re-derive the memo_tags rows from content. Caller wraps in a transaction.
fn sync_tags(conn: &Connection, memo_id: i64, content: &str) -> Result<(), AppError> {
    conn.execute("DELETE FROM memo_tags WHERE memo_id = ?1", [memo_id])?;
    let mut stmt =
        conn.prepare("INSERT OR IGNORE INTO memo_tags (memo_id, tag) VALUES (?1, ?2)")?;
    for tag in crate::markdown::extract_tags(content) {
        stmt.execute(params![memo_id, tag])?;
    }
    Ok(())
}

/// Claim still-unattached resources (uploaded from the composer) for this memo.
/// Owner-scoped so users can't attach other people's uploads.
fn attach_resources(
    conn: &Connection,
    memo_id: i64,
    user_id: i64,
    resource_uids: &[String],
) -> Result<(), AppError> {
    let mut stmt = conn.prepare(
        "UPDATE resources SET memo_id = ?1
         WHERE uid = ?2 AND user_id = ?3 AND memo_id IS NULL",
    )?;
    for uid in resource_uids {
        stmt.execute(params![memo_id, uid, user_id])?;
    }
    Ok(())
}

/// Reconcile a memo's attachments to exactly `resource_uids`: claim newly
/// uploaded resources and delete the ones the editor dropped. Owner-scoped, so
/// only this user's resources on this memo are ever removed.
fn set_resources(
    conn: &Connection,
    memo_id: i64,
    user_id: i64,
    resource_uids: &[String],
) -> Result<(), AppError> {
    attach_resources(conn, memo_id, user_id, resource_uids)?;

    if resource_uids.is_empty() {
        conn.execute(
            "DELETE FROM resources WHERE memo_id = ?1 AND user_id = ?2",
            params![memo_id, user_id],
        )?;
    } else {
        let placeholders = vec!["?"; resource_uids.len()].join(",");
        let sql = format!(
            "DELETE FROM resources WHERE memo_id = ? AND user_id = ? AND uid NOT IN ({placeholders})"
        );
        let mut binds: Vec<&dyn rusqlite::ToSql> = vec![&memo_id, &user_id];
        binds.extend(resource_uids.iter().map(|u| u as &dyn rusqlite::ToSql));
        conn.execute(&sql, params_from_iter(binds))?;
    }
    Ok(())
}

pub async fn create(
    pool: &Pool,
    user_id: i64,
    content: String,
    visibility: Visibility,
    resource_uids: Vec<String>,
) -> Result<Memo, AppError> {
    let uid = crate::auth::new_uid();
    crate::db::run(pool, move |conn| {
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO memos (uid, user_id, content, visibility) VALUES (?1, ?2, ?3, ?4)",
            params![uid, user_id, content, visibility],
        )?;
        let id = tx.last_insert_rowid();
        sync_tags(&tx, id, &content)?;
        attach_resources(&tx, id, user_id, &resource_uids)?;
        let memo = get_by_id_sync(&tx, id)?;
        tx.commit()?;
        Ok(memo)
    })
    .await
}

/// Owner-checked update of content/visibility; re-syncs tags and claims new uploads.
pub async fn update(
    pool: &Pool,
    memo_id: i64,
    user_id: i64,
    content: String,
    visibility: Visibility,
    resource_uids: Vec<String>,
) -> Result<Memo, AppError> {
    crate::db::run(pool, move |conn| {
        let tx = conn.transaction()?;
        let changed = tx.execute(
            "UPDATE memos SET content = ?1, visibility = ?2, updated_at = unixepoch()
             WHERE id = ?3 AND user_id = ?4",
            params![content, visibility, memo_id, user_id],
        )?;
        if changed == 0 {
            return Err(AppError::NotFound);
        }
        sync_tags(&tx, memo_id, &content)?;
        set_resources(&tx, memo_id, user_id, &resource_uids)?;
        let memo = get_by_id_sync(&tx, memo_id)?;
        tx.commit()?;
        Ok(memo)
    })
    .await
}

pub async fn get(pool: &Pool, memo_id: i64) -> Result<Memo, AppError> {
    crate::db::run(pool, move |conn| get_by_id_sync(conn, memo_id)).await
}

pub async fn get_by_uid(pool: &Pool, uid: String) -> Result<Memo, AppError> {
    crate::db::run(pool, move |conn| {
        conn.query_row(
            &format!(
                "SELECT {MEMO_COLS} FROM memos m JOIN users u ON u.id = m.user_id WHERE m.uid = ?1"
            ),
            [&uid],
            memo_from_row,
        )
        .map_err(AppError::from)
    })
    .await
}

pub async fn toggle_pin(pool: &Pool, memo_id: i64, user_id: i64) -> Result<Memo, AppError> {
    crate::db::run(pool, move |conn| {
        let changed = conn.execute(
            "UPDATE memos SET pinned = 1 - pinned, updated_at = unixepoch()
             WHERE id = ?1 AND user_id = ?2",
            params![memo_id, user_id],
        )?;
        if changed == 0 {
            return Err(AppError::NotFound);
        }
        get_by_id_sync(conn, memo_id)
    })
    .await
}

pub async fn toggle_archived(pool: &Pool, memo_id: i64, user_id: i64) -> Result<Memo, AppError> {
    crate::db::run(pool, move |conn| {
        let changed = conn.execute(
            "UPDATE memos SET
               state = CASE state WHEN 'archived' THEN 'normal' ELSE 'archived' END,
               pinned = 0,
               updated_at = unixepoch()
             WHERE id = ?1 AND user_id = ?2",
            params![memo_id, user_id],
        )?;
        if changed == 0 {
            return Err(AppError::NotFound);
        }
        get_by_id_sync(conn, memo_id)
    })
    .await
}

pub async fn delete(pool: &Pool, memo_id: i64, user_id: i64) -> Result<(), AppError> {
    crate::db::run(pool, move |conn| {
        let changed = conn.execute(
            "DELETE FROM memos WHERE id = ?1 AND user_id = ?2",
            params![memo_id, user_id],
        )?;
        if changed == 0 {
            return Err(AppError::NotFound);
        }
        Ok(())
    })
    .await
}

// ---------- Feed queries ----------

#[derive(Debug, Clone)]
pub enum Feed {
    /// The owner's own timeline (all visibilities).
    Own(i64),
    /// The owner's archived memos.
    Archive(i64),
    /// Everyone's public memos (+ protected ones for signed-in viewers).
    Explore { signed_in: bool },
}

#[derive(Debug, Clone)]
pub struct MemoQuery {
    pub feed: Feed,
    pub tag: Option<String>,
    pub search: Option<String>,
    /// Half-open `[start, end)` created_at window (unix seconds) from the date filter.
    pub created_range: Option<(i64, i64)>,
    pub before: Option<(i64, i64)>,
    pub limit: i64,
}

pub struct MemoPage {
    /// Pinned block, only populated for an unfiltered first page of `Own`.
    pub pinned: Vec<Memo>,
    pub memos: Vec<Memo>,
    pub has_more: bool,
}

/// Quote each search term so user input can't hit FTS5 query syntax.
fn fts_query(input: &str) -> String {
    input
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

pub async fn list(pool: &Pool, query: MemoQuery) -> Result<MemoPage, AppError> {
    crate::db::run(pool, move |conn| {
        let mut wheres: Vec<String> = Vec::new();
        let mut binds: Vec<Value> = Vec::new();

        match &query.feed {
            Feed::Own(user_id) => {
                wheres.push("m.user_id = ? AND m.state = 'normal'".into());
                binds.push(Value::Integer(*user_id));
            }
            Feed::Archive(user_id) => {
                wheres.push("m.user_id = ? AND m.state = 'archived'".into());
                binds.push(Value::Integer(*user_id));
            }
            Feed::Explore { signed_in } => {
                if *signed_in {
                    wheres.push(
                        "m.state = 'normal' AND m.visibility IN ('public','protected')".into(),
                    );
                } else {
                    wheres.push("m.state = 'normal' AND m.visibility = 'public'".into());
                }
            }
        }
        if let Some(tag) = &query.tag {
            wheres.push("m.id IN (SELECT memo_id FROM memo_tags WHERE tag = ?)".into());
            binds.push(Value::Text(tag.to_ascii_lowercase()));
        }
        if let Some(search) = &query.search {
            wheres.push("m.id IN (SELECT rowid FROM memos_fts WHERE memos_fts MATCH ?)".into());
            binds.push(Value::Text(fts_query(search)));
        }
        if let Some((start, end)) = query.created_range {
            wheres.push("m.created_at >= ? AND m.created_at < ?".into());
            binds.push(Value::Integer(start));
            binds.push(Value::Integer(end));
        }
        if let Some((ts, id)) = query.before {
            wheres.push("(m.created_at, m.id) < (?, ?)".into());
            binds.push(Value::Integer(ts));
            binds.push(Value::Integer(id));
        }

        // The pinned block lives above the feed on the unfiltered first page;
        // exclude pinned rows from that feed so they don't appear twice.
        let unfiltered_own = matches!(query.feed, Feed::Own(_))
            && query.tag.is_none()
            && query.search.is_none()
            && query.created_range.is_none();
        let pinned = if unfiltered_own && query.before.is_none() {
            let Feed::Own(user_id) = query.feed else {
                unreachable!()
            };
            let mut stmt = conn.prepare(&format!(
                "SELECT {MEMO_COLS} FROM memos m JOIN users u ON u.id = m.user_id
                 WHERE m.user_id = ?1 AND m.state = 'normal' AND m.pinned = 1
                 ORDER BY m.created_at DESC, m.id DESC"
            ))?;
            stmt.query_map([user_id], memo_from_row)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };
        if unfiltered_own {
            wheres.push("m.pinned = 0".into());
        }

        let sql = format!(
            "SELECT {MEMO_COLS} FROM memos m JOIN users u ON u.id = m.user_id
             WHERE {} ORDER BY m.created_at DESC, m.id DESC LIMIT {}",
            wheres.join(" AND "),
            query.limit + 1,
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut memos = stmt
            .query_map(params_from_iter(binds), memo_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        let has_more = memos.len() as i64 > query.limit;
        memos.truncate(query.limit as usize);

        Ok(MemoPage {
            pinned,
            memos,
            has_more,
        })
    })
    .await
}

/// Tag counts for the sidebar — the viewer's own active memos.
pub async fn tag_counts(pool: &Pool, user_id: i64) -> Result<Vec<TagCount>, AppError> {
    crate::db::run(pool, move |conn| {
        let mut stmt = conn.prepare(
            "SELECT t.tag, COUNT(*) FROM memo_tags t
             JOIN memos m ON m.id = t.memo_id
             WHERE m.user_id = ?1 AND m.state = 'normal'
             GROUP BY t.tag ORDER BY COUNT(*) DESC, t.tag",
        )?;
        let rows = stmt
            .query_map([user_id], |r| {
                Ok(TagCount {
                    tag: r.get(0)?,
                    count: r.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
    .await
}

/// Per-day note counts for the activity heatmap — the viewer's own active memos
/// created since `since` (unix seconds). Days are bucketed in UTC, returned as
/// `("YYYY-MM-DD", count)` rows; empty days are simply absent.
pub async fn activity_since(
    pool: &Pool,
    user_id: i64,
    since: i64,
) -> Result<Vec<(String, i64)>, AppError> {
    crate::db::run(pool, move |conn| {
        let mut stmt = conn.prepare(
            "SELECT strftime('%Y-%m-%d', m.created_at, 'unixepoch') AS d, COUNT(*)
             FROM memos m
             WHERE m.user_id = ?1 AND m.state = 'normal' AND m.created_at >= ?2
             GROUP BY d",
        )?;
        let rows = stmt
            .query_map(params![user_id, since], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
    .await
}

/// Visibility check for a single memo page (`/m/:uid`) and resource serving.
pub fn can_view(memo: &Memo, viewer: Option<&crate::models::User>) -> bool {
    match memo.visibility {
        Visibility::Public => true,
        Visibility::Protected => viewer.is_some(),
        Visibility::Private => viewer.map(|u| u.id == memo.user_id).unwrap_or(false),
    }
}

/// Archived memos are only ever visible to their owner, regardless of visibility.
pub fn can_view_considering_state(memo: &Memo, viewer: Option<&crate::models::User>) -> bool {
    if memo.state == MemoState::Archived {
        return viewer.map(|u| u.id == memo.user_id).unwrap_or(false);
    }
    can_view(memo, viewer)
}
