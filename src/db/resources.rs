use std::collections::HashMap;

use deadpool_sqlite::Pool;
use rusqlite::{OptionalExtension, params};

use crate::error::AppError;
use crate::models::ResourceMeta;

pub async fn insert(
    pool: &Pool,
    user_id: i64,
    filename: String,
    content_type: String,
    data: Vec<u8>,
) -> Result<ResourceMeta, AppError> {
    let uid = crate::auth::new_uid();
    crate::db::run(pool, move |conn| {
        let size = data.len() as i64;
        conn.execute(
            "INSERT INTO resources (uid, user_id, filename, content_type, size, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![uid, user_id, filename, content_type, size, data],
        )?;
        Ok(ResourceMeta {
            uid,
            filename,
            content_type,
            size,
        })
    })
    .await
}

pub struct ResourceBlob {
    pub meta: ResourceMeta,
    pub data: Vec<u8>,
    pub owner_id: i64,
    /// None while still unattached (composer upload not yet saved).
    pub memo_id: Option<i64>,
}

pub async fn get_blob(pool: &Pool, uid: String) -> Result<Option<ResourceBlob>, AppError> {
    crate::db::run(pool, move |conn| {
        conn.query_row(
            "SELECT uid, filename, content_type, size, data, user_id, memo_id
             FROM resources WHERE uid = ?1",
            [&uid],
            |r| {
                Ok(ResourceBlob {
                    meta: ResourceMeta {
                        uid: r.get(0)?,
                        filename: r.get(1)?,
                        content_type: r.get(2)?,
                        size: r.get(3)?,
                    },
                    data: r.get(4)?,
                    owner_id: r.get(5)?,
                    memo_id: r.get(6)?,
                })
            },
        )
        .optional()
        .map_err(AppError::from)
    })
    .await
}

/// Attachment metadata for a set of memos, keyed by memo id.
pub async fn for_memos(
    pool: &Pool,
    memo_ids: Vec<i64>,
) -> Result<HashMap<i64, Vec<ResourceMeta>>, AppError> {
    if memo_ids.is_empty() {
        return Ok(HashMap::new());
    }
    crate::db::run(pool, move |conn| {
        let placeholders = vec!["?"; memo_ids.len()].join(",");
        let mut stmt = conn.prepare(&format!(
            "SELECT memo_id, uid, filename, content_type, size
             FROM resources WHERE memo_id IN ({placeholders}) ORDER BY id"
        ))?;
        let params = rusqlite::params_from_iter(memo_ids.iter());
        let mut map: HashMap<i64, Vec<ResourceMeta>> = HashMap::new();
        let rows = stmt.query_map(params, |r| {
            Ok((
                r.get::<_, i64>(0)?,
                ResourceMeta {
                    uid: r.get(1)?,
                    filename: r.get(2)?,
                    content_type: r.get(3)?,
                    size: r.get(4)?,
                },
            ))
        })?;
        for row in rows {
            let (memo_id, meta) = row?;
            map.entry(memo_id).or_default().push(meta);
        }
        Ok(map)
    })
    .await
}

/// Delete leftovers that were uploaded but never attached to a saved memo.
pub async fn purge_orphans(pool: &Pool, older_than_secs: i64) -> Result<usize, AppError> {
    crate::db::run(pool, move |conn| {
        let n = conn.execute(
            "DELETE FROM resources WHERE memo_id IS NULL
             AND created_at < unixepoch() - ?1",
            [older_than_secs],
        )?;
        Ok(n)
    })
    .await
}
