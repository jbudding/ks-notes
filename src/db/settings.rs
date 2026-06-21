use deadpool_sqlite::Pool;
use rusqlite::{Connection, OptionalExtension, params};

use crate::error::AppError;

pub const ALLOW_REGISTRATION: &str = "allow_registration";

pub fn get_sync(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
        r.get(0)
    })
    .optional()
}

pub async fn set(pool: &Pool, key: &'static str, value: String) -> Result<(), AppError> {
    crate::db::run(pool, move |conn| {
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    })
    .await
}

pub fn allow_registration_sync(conn: &Connection) -> rusqlite::Result<bool> {
    Ok(get_sync(conn, ALLOW_REGISTRATION)?.as_deref() == Some("true"))
}

/// Registration is open when explicitly enabled, or when no users exist yet
/// (first-run onboarding).
pub async fn registration_open(pool: &Pool) -> Result<bool, AppError> {
    crate::db::run(pool, move |conn| {
        let users: i64 = conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
        Ok(users == 0 || allow_registration_sync(conn)?)
    })
    .await
}
