use deadpool_sqlite::Pool;
use rusqlite::types::Value;
use rusqlite::{Connection, params, params_from_iter};

use crate::error::AppError;
use data_encoding::BASE64;

use crate::models::{ExportAttachment, ExportNote, Memo, MemoState, NoteCounts, TagCount, Visibility};

const MEMO_COLS: &str = "m.id, m.uid, m.user_id, u.username, m.content, m.visibility,
                         m.pinned, m.state, m.created_at, m.updated_at, m.uuid, m.origin";

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
        uuid: r.get(10)?,
        origin: r.get(11)?,
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
) -> Result<Memo, AppError> {
    let uid = crate::auth::new_uid();
    let uuid = crate::auth::new_uuid();
    crate::db::run(pool, move |conn| {
        let resource_uids = crate::markdown::extract_attachment_refs(&content);
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO memos (uid, uuid, user_id, content, visibility) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![uid, uuid, user_id, content, visibility],
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
) -> Result<Memo, AppError> {
    crate::db::run(pool, move |conn| {
        let resource_uids = crate::markdown::extract_attachment_refs(&content);
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
    /// The owner's own authored timeline (all visibilities, local origin).
    Own(i64),
    /// The owner's archived memos.
    Archive(i64),
    /// Everyone's public memos (+ protected ones for signed-in viewers).
    Explore { signed_in: bool },
    /// The owner's imported memos (origin='imported'), kept out of every other feed.
    Imported(i64),
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
                wheres.push("m.user_id = ? AND m.state = 'normal' AND m.origin = 'local'".into());
                binds.push(Value::Integer(*user_id));
            }
            Feed::Archive(user_id) => {
                wheres.push("m.user_id = ? AND m.state = 'archived'".into());
                binds.push(Value::Integer(*user_id));
            }
            Feed::Explore { signed_in } => {
                // Imported notes are never surfaced in the public square.
                if *signed_in {
                    wheres.push(
                        "m.state = 'normal' AND m.origin = 'local' AND m.visibility IN ('public','protected')".into(),
                    );
                } else {
                    wheres.push(
                        "m.state = 'normal' AND m.origin = 'local' AND m.visibility = 'public'".into(),
                    );
                }
            }
            Feed::Imported(user_id) => {
                wheres.push("m.user_id = ? AND m.state = 'normal' AND m.origin = 'imported'".into());
                binds.push(Value::Integer(*user_id));
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
                 WHERE m.user_id = ?1 AND m.state = 'normal' AND m.origin = 'local' AND m.pinned = 1
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
             WHERE m.user_id = ?1 AND m.state = 'normal' AND m.origin = 'local'
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

/// Active (non-archived) note counts for the Home and Imported nav items.
pub async fn note_counts(pool: &Pool, user_id: i64) -> Result<NoteCounts, AppError> {
    crate::db::run(pool, move |conn| {
        conn.query_row(
            "SELECT
               COALESCE(SUM(CASE WHEN origin='local'    AND state='normal' THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN origin='imported' AND state='normal' THEN 1 ELSE 0 END), 0)
             FROM memos WHERE user_id = ?1",
            [user_id],
            |r| Ok(NoteCounts { home: r.get(0)?, imported: r.get(1)? }),
        )
        .map_err(AppError::from)
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

// ---------- Export / import ----------

/// The user's own active (local) notes carrying any of `tags`, as export rows.
/// Tags are matched lowercased; an empty `tags` yields nothing.
pub async fn export_by_tags(
    pool: &Pool,
    user_id: i64,
    tags: Vec<String>,
) -> Result<Vec<ExportNote>, AppError> {
    crate::db::run(pool, move |conn| {
        if tags.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; tags.len()].join(",");
        let sql = format!(
            "SELECT m.id, m.uuid, m.content, m.visibility, m.created_at, m.updated_at
             FROM memos m
             WHERE m.user_id = ? AND m.state = 'normal' AND m.origin = 'local'
               AND m.id IN (SELECT memo_id FROM memo_tags WHERE tag IN ({placeholders}))
             ORDER BY m.created_at DESC, m.id DESC"
        );
        let mut binds: Vec<Value> = vec![Value::Integer(user_id)];
        binds.extend(tags.iter().map(|t| Value::Text(t.to_ascii_lowercase())));
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt
            .query_map(params_from_iter(binds), |r| {
                Ok((r.get::<_, i64>(0)?, ExportNote {
                    uuid: r.get(1)?,
                    content: r.get(2)?,
                    visibility: r.get(3)?,
                    created_at: r.get(4)?,
                    updated_at: r.get(5)?,
                    attachments: Vec::new(),
                }))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        // Pull each note's attachments (with blob data) and base64 them inline.
        // `uid` lets import remap the note's {{attach:UID}} tokens to new ids.
        let mut att_stmt = conn.prepare(
            "SELECT uid, filename, content_type, created_at, data
             FROM resources WHERE memo_id = ?1 ORDER BY id",
        )?;
        for (id, note) in &mut rows {
            note.attachments = att_stmt
                .query_map([*id], |r| {
                    Ok(ExportAttachment {
                        uid: r.get(0)?,
                        filename: r.get(1)?,
                        content_type: r.get(2)?,
                        created_at: r.get(3)?,
                        data: BASE64.encode(&r.get::<_, Vec<u8>>(4)?),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
        }

        Ok(rows.into_iter().map(|(_, note)| note).collect())
    })
    .await
}

/// Insert one imported attachment row under `uid`, attached to `memo_id` and
/// owned by `user_id`. Returns whether the base64 decoded successfully.
fn insert_imported_attachment(
    conn: &Connection,
    memo_id: i64,
    user_id: i64,
    uid: &str,
    att: &ExportAttachment,
) -> Result<bool, AppError> {
    let Ok(bytes) = BASE64.decode(att.data.as_bytes()) else {
        return Ok(false);
    };
    let size = bytes.len() as i64;
    conn.execute(
        "INSERT INTO resources (uid, user_id, memo_id, filename, content_type, size, data, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            uid,
            user_id,
            memo_id,
            att.filename,
            att.content_type,
            size,
            bytes,
            att.created_at
        ],
    )?;
    Ok(true)
}

/// Outcome of an import run, surfaced to the user.
#[derive(Debug, Default, Clone, Copy)]
pub struct ImportSummary {
    pub inserted: usize,
    pub updated: usize,
    pub skipped: usize,
}

/// Import notes under `user_id`, deduping by `uuid`:
/// - matches an existing **imported** note → overwrite if `overwrite` is set or
///   the incoming copy is strictly newer, else skip;
/// - matches one of the user's **local** (authored) notes → skip (already present);
/// - no match → insert as a new imported note, preserving timestamps.
/// Each note's tags are re-derived from its content.
pub async fn import_notes(
    pool: &Pool,
    user_id: i64,
    notes: Vec<ExportNote>,
    overwrite: bool,
) -> Result<ImportSummary, AppError> {
    crate::db::run(pool, move |conn| {
        let tx = conn.transaction()?;
        let mut summary = ImportSummary::default();
        for note in &notes {
            let content = note.content.trim();
            if content.is_empty() || note.uuid.trim().is_empty() {
                summary.skipped += 1;
                continue;
            }

            // Attachments get fresh uids here, so rewrite the note's tokens to
            // match. A v1/v2 attachment (no uid / no token) gets a token appended.
            let mut final_content = content.to_string();
            let mut planned: Vec<(String, &ExportAttachment)> = Vec::new();
            for att in &note.attachments {
                let new_uid = crate::auth::new_uid();
                let new_token = format!("{{{{attach:{new_uid}}}}}");
                let old_token = format!("{{{{attach:{}}}}}", att.uid);
                if !att.uid.is_empty() && final_content.contains(&old_token) {
                    final_content = final_content.replace(&old_token, &new_token);
                } else {
                    final_content.push_str(&format!("\n\n{new_token}"));
                }
                planned.push((new_uid, att));
            }

            // Existing note with this uuid, if any (origin + freshness decide the action).
            let existing: Option<(i64, String, i64)> = tx
                .query_row(
                    "SELECT id, origin, updated_at FROM memos WHERE user_id = ?1 AND uuid = ?2",
                    params![user_id, note.uuid],
                    |r| Ok((r.get(0)?, r.get::<_, String>(1)?, r.get(2)?)),
                )
                .ok();

            match existing {
                Some((_, origin, _)) if origin == "local" => {
                    summary.skipped += 1;
                }
                Some((id, _, existing_updated)) => {
                    if overwrite || note.updated_at > existing_updated {
                        tx.execute(
                            "UPDATE memos SET content = ?1, visibility = ?2,
                                 created_at = ?3, updated_at = ?4
                             WHERE id = ?5",
                            params![
                                final_content,
                                note.visibility,
                                note.created_at,
                                note.updated_at,
                                id
                            ],
                        )?;
                        sync_tags(&tx, id, &final_content)?;
                        // Replace attachments wholesale with the incoming set.
                        tx.execute("DELETE FROM resources WHERE memo_id = ?1", params![id])?;
                        for (uid, att) in &planned {
                            insert_imported_attachment(&tx, id, user_id, uid, att)?;
                        }
                        summary.updated += 1;
                    } else {
                        summary.skipped += 1;
                    }
                }
                None => {
                    tx.execute(
                        "INSERT INTO memos
                           (uid, uuid, user_id, content, visibility, origin, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, 'imported', ?6, ?7)",
                        params![
                            crate::auth::new_uid(),
                            note.uuid,
                            user_id,
                            final_content,
                            note.visibility,
                            note.created_at,
                            note.updated_at
                        ],
                    )?;
                    let id = tx.last_insert_rowid();
                    sync_tags(&tx, id, &final_content)?;
                    for (uid, att) in &planned {
                        insert_imported_attachment(&tx, id, user_id, uid, att)?;
                    }
                    summary.inserted += 1;
                }
            }
        }
        tx.commit()?;
        Ok(summary)
    })
    .await
}
