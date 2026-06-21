pub mod memos;
pub mod resources;
pub mod sessions;
pub mod settings;
pub mod tokens;
pub mod users;

use std::path::Path;

use deadpool_sqlite::{Config as DbConfig, Pool, Runtime};
use rusqlite::Connection;

use crate::error::AppError;

/// Each entry runs once, in order, inside its own transaction.
/// `PRAGMA user_version` tracks progress. Append-only — never edit old entries.
const MIGRATIONS: &[&str] = &[
    // 001 — full MVP schema
    r#"
CREATE TABLE users (
  id            INTEGER PRIMARY KEY,
  username      TEXT NOT NULL UNIQUE COLLATE NOCASE,
  password_hash TEXT NOT NULL,
  role          TEXT NOT NULL DEFAULT 'user' CHECK (role IN ('admin','user')),
  created_at    INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at    INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE settings (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE sessions (
  id         INTEGER PRIMARY KEY,
  token_hash TEXT NOT NULL UNIQUE,
  user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  csrf_token TEXT NOT NULL,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  expires_at INTEGER NOT NULL
);
CREATE INDEX idx_sessions_user ON sessions(user_id);

CREATE TABLE api_tokens (
  id           INTEGER PRIMARY KEY,
  user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  name         TEXT NOT NULL,
  token_hash   TEXT NOT NULL UNIQUE,
  created_at   INTEGER NOT NULL DEFAULT (unixepoch()),
  last_used_at INTEGER
);

CREATE TABLE memos (
  id         INTEGER PRIMARY KEY,
  uid        TEXT NOT NULL UNIQUE,
  user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  content    TEXT NOT NULL,
  visibility TEXT NOT NULL DEFAULT 'private'
             CHECK (visibility IN ('private','protected','public')),
  pinned     INTEGER NOT NULL DEFAULT 0,
  state      TEXT NOT NULL DEFAULT 'normal' CHECK (state IN ('normal','archived')),
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX idx_memos_user_timeline ON memos(user_id, state, created_at DESC);
CREATE INDEX idx_memos_visibility    ON memos(visibility, state, created_at DESC);

CREATE TABLE memo_tags (
  memo_id INTEGER NOT NULL REFERENCES memos(id) ON DELETE CASCADE,
  tag     TEXT NOT NULL,
  PRIMARY KEY (memo_id, tag)
) WITHOUT ROWID;
CREATE INDEX idx_memo_tags_tag ON memo_tags(tag, memo_id);

CREATE TABLE resources (
  id           INTEGER PRIMARY KEY,
  uid          TEXT NOT NULL UNIQUE,
  user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  memo_id      INTEGER REFERENCES memos(id) ON DELETE CASCADE,
  filename     TEXT NOT NULL,
  content_type TEXT NOT NULL,
  size         INTEGER NOT NULL,
  data         BLOB NOT NULL,
  created_at   INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX idx_resources_memo ON resources(memo_id);

CREATE VIRTUAL TABLE memos_fts USING fts5(
  content,
  content='memos', content_rowid='id',
  tokenize='unicode61 remove_diacritics 2'
);
CREATE TRIGGER memos_fts_ai AFTER INSERT ON memos BEGIN
  INSERT INTO memos_fts(rowid, content) VALUES (new.id, new.content);
END;
CREATE TRIGGER memos_fts_ad AFTER DELETE ON memos BEGIN
  INSERT INTO memos_fts(memos_fts, rowid, content) VALUES('delete', old.id, old.content);
END;
CREATE TRIGGER memos_fts_au AFTER UPDATE OF content ON memos BEGIN
  INSERT INTO memos_fts(memos_fts, rowid, content) VALUES('delete', old.id, old.content);
  INSERT INTO memos_fts(rowid, content) VALUES (new.id, new.content);
END;
"#,
];

/// Open the database directly (pre-pool), enable WAL, and apply pending migrations.
pub fn migrate(path: &Path) -> Result<(), AppError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::Internal(format!("creating db dir: {e}")))?;
    }
    let mut conn = Connection::open(path)?;
    // WAL is persistent (stored in the db header), so setting it once here is enough.
    let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (idx, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (idx + 1) as i64)?;
        tx.commit()?;
        tracing::info!(migration = idx + 1, "applied migration");
    }
    Ok(())
}

pub fn create_pool(path: &Path) -> Result<Pool, AppError> {
    DbConfig::new(path)
        .create_pool(Runtime::Tokio1)
        .map_err(|e| AppError::Internal(format!("creating db pool: {e}")))
}

/// Run a closure against a pooled connection on the blocking pool, with
/// per-connection pragmas applied. All DB access goes through here.
pub async fn run<F, T>(pool: &Pool, f: F) -> Result<T, AppError>
where
    F: FnOnce(&mut Connection) -> Result<T, AppError> + Send + 'static,
    T: Send + 'static,
{
    let obj = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(format!("db pool: {e}")))?;
    obj.interact(move |conn| {
        conn.pragma_update(None, "foreign_keys", 1)?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        f(conn)
    })
    .await
    .map_err(|e| AppError::Internal(format!("db interact: {e}")))?
}
