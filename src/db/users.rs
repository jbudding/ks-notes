use deadpool_sqlite::Pool;
use rusqlite::{OptionalExtension, params};

use crate::error::AppError;
use crate::models::{Role, User};

pub enum RegisterOutcome {
    Created(User),
    RegistrationClosed,
    UsernameTaken,
}

/// Atomic registration: the count check, the first-user-becomes-admin rule,
/// and the insert all happen in one transaction.
pub async fn register(
    pool: &Pool,
    username: String,
    password_hash: String,
) -> Result<RegisterOutcome, AppError> {
    crate::db::run(pool, move |conn| {
        let tx = conn.transaction()?;
        let count: i64 = tx.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
        if count > 0 && !super::settings::allow_registration_sync(&tx)? {
            return Ok(RegisterOutcome::RegistrationClosed);
        }
        let exists: Option<i64> = tx
            .query_row(
                "SELECT id FROM users WHERE username = ?1 COLLATE NOCASE",
                [&username],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_some() {
            return Ok(RegisterOutcome::UsernameTaken);
        }
        let role = if count == 0 { Role::Admin } else { Role::User };
        tx.execute(
            "INSERT INTO users (username, password_hash, role) VALUES (?1, ?2, ?3)",
            params![username, password_hash, role],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(RegisterOutcome::Created(User { id, username, role }))
    })
    .await
}

/// User + password hash, for login verification.
pub async fn get_for_login(
    pool: &Pool,
    username: String,
) -> Result<Option<(User, String)>, AppError> {
    crate::db::run(pool, move |conn| {
        conn.query_row(
            "SELECT id, username, role, password_hash FROM users WHERE username = ?1 COLLATE NOCASE",
            [&username],
            |r| {
                Ok((
                    User {
                        id: r.get(0)?,
                        username: r.get(1)?,
                        role: r.get(2)?,
                    },
                    r.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(AppError::from)
    })
    .await
}

pub async fn get_password_hash(pool: &Pool, user_id: i64) -> Result<String, AppError> {
    crate::db::run(pool, move |conn| {
        conn.query_row(
            "SELECT password_hash FROM users WHERE id = ?1",
            [user_id],
            |r| r.get(0),
        )
        .map_err(AppError::from)
    })
    .await
}

pub async fn update_password(
    pool: &Pool,
    user_id: i64,
    password_hash: String,
) -> Result<(), AppError> {
    crate::db::run(pool, move |conn| {
        conn.execute(
            "UPDATE users SET password_hash = ?1, updated_at = unixepoch() WHERE id = ?2",
            params![password_hash, user_id],
        )?;
        Ok(())
    })
    .await
}

pub async fn count(pool: &Pool) -> Result<i64, AppError> {
    crate::db::run(pool, |conn| {
        conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
            .map_err(AppError::from)
    })
    .await
}

/// All users with their creation time, for the admin page.
pub async fn list(pool: &Pool) -> Result<Vec<(User, i64)>, AppError> {
    crate::db::run(pool, |conn| {
        let mut stmt =
            conn.prepare("SELECT id, username, role, created_at FROM users ORDER BY id")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    User {
                        id: r.get(0)?,
                        username: r.get(1)?,
                        role: r.get(2)?,
                    },
                    r.get::<_, i64>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
    .await
}
