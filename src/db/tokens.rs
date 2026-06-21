use deadpool_sqlite::Pool;
use rusqlite::{OptionalExtension, params};

use crate::error::AppError;
use crate::models::{Role, User};

pub struct ApiTokenMeta {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

pub async fn create(
    pool: &Pool,
    user_id: i64,
    name: String,
    token_hash: String,
) -> Result<(), AppError> {
    crate::db::run(pool, move |conn| {
        conn.execute(
            "INSERT INTO api_tokens (user_id, name, token_hash) VALUES (?1, ?2, ?3)",
            params![user_id, name, token_hash],
        )?;
        Ok(())
    })
    .await
}

pub async fn list(pool: &Pool, user_id: i64) -> Result<Vec<ApiTokenMeta>, AppError> {
    crate::db::run(pool, move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, name, created_at, last_used_at FROM api_tokens
             WHERE user_id = ?1 ORDER BY id",
        )?;
        let rows = stmt
            .query_map([user_id], |r| {
                Ok(ApiTokenMeta {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    created_at: r.get(2)?,
                    last_used_at: r.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
    .await
}

pub async fn revoke(pool: &Pool, user_id: i64, token_id: i64) -> Result<(), AppError> {
    crate::db::run(pool, move |conn| {
        let changed = conn.execute(
            "DELETE FROM api_tokens WHERE id = ?1 AND user_id = ?2",
            params![token_id, user_id],
        )?;
        if changed == 0 {
            return Err(AppError::NotFound);
        }
        Ok(())
    })
    .await
}

/// Resolve a Bearer token to its user and stamp last_used_at.
pub async fn lookup_user(pool: &Pool, token_hash: String) -> Result<Option<User>, AppError> {
    crate::db::run(pool, move |conn| {
        let found = conn
            .query_row(
                "SELECT t.id, u.id, u.username, u.role
                 FROM api_tokens t JOIN users u ON u.id = t.user_id
                 WHERE t.token_hash = ?1",
                [&token_hash],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        User {
                            id: r.get(1)?,
                            username: r.get(2)?,
                            role: r.get::<_, Role>(3)?,
                        },
                    ))
                },
            )
            .optional()?;
        match found {
            Some((token_id, user)) => {
                // Best-effort usage stamp.
                let _ = conn.execute(
                    "UPDATE api_tokens SET last_used_at = unixepoch() WHERE id = ?1",
                    [token_id],
                );
                Ok(Some(user))
            }
            None => Ok(None),
        }
    })
    .await
}
