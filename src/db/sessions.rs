use deadpool_sqlite::Pool;
use rusqlite::params;

use crate::auth::{SESSION_TTL_SECS, SessionUser};
use crate::error::AppError;
use crate::models::{Role, User};

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

pub async fn create(
    pool: &Pool,
    user_id: i64,
    token_hash: String,
    csrf_token: String,
) -> Result<(), AppError> {
    crate::db::run(pool, move |conn| {
        // Opportunistic purge of expired sessions.
        conn.execute("DELETE FROM sessions WHERE expires_at < unixepoch()", [])?;
        conn.execute(
            "INSERT INTO sessions (token_hash, user_id, csrf_token, expires_at)
             VALUES (?1, ?2, ?3, unixepoch() + ?4)",
            params![token_hash, user_id, csrf_token, SESSION_TTL_SECS],
        )?;
        Ok(())
    })
    .await
}

pub async fn lookup(pool: &Pool, token_hash: String) -> Result<Option<SessionUser>, AppError> {
    crate::db::run(pool, move |conn| {
        let row = conn.query_row(
            "SELECT s.id, s.csrf_token, s.expires_at, u.id, u.username, u.role
             FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = ?1 AND s.expires_at > unixepoch()",
            [&token_hash],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    User {
                        id: r.get(3)?,
                        username: r.get(4)?,
                        role: r.get::<_, Role>(5)?,
                    },
                ))
            },
        );
        match row {
            Ok((session_id, csrf_token, expires_at, user)) => {
                // Sliding expiry: extend once less than half the TTL remains.
                if expires_at - now() < SESSION_TTL_SECS / 2 {
                    conn.execute(
                        "UPDATE sessions SET expires_at = unixepoch() + ?1 WHERE id = ?2",
                        params![SESSION_TTL_SECS, session_id],
                    )?;
                }
                Ok(Some(SessionUser { user, csrf_token }))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    })
    .await
}

pub async fn delete_by_hash(pool: &Pool, token_hash: String) -> Result<(), AppError> {
    crate::db::run(pool, move |conn| {
        conn.execute("DELETE FROM sessions WHERE token_hash = ?1", [&token_hash])?;
        Ok(())
    })
    .await
}

/// After a password change: sign out everywhere else.
pub async fn delete_all_for_user_except(
    pool: &Pool,
    user_id: i64,
    keep_token_hash: String,
) -> Result<(), AppError> {
    crate::db::run(pool, move |conn| {
        conn.execute(
            "DELETE FROM sessions WHERE user_id = ?1 AND token_hash != ?2",
            params![user_id, keep_token_hash],
        )?;
        Ok(())
    })
    .await
}
