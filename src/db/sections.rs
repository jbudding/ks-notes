use deadpool_sqlite::Pool;
use rusqlite::params;

use crate::error::AppError;
use crate::models::Section;

/// Where a note is being moved, parsed from the editor's Section select.
#[derive(Debug, Clone, Copy)]
pub enum MoveDest {
    /// Home: local, no section.
    Home,
    /// A specific section.
    Section(i64),
    /// Leave an imported note where it is.
    KeepImported,
}

impl MoveDest {
    pub fn parse(s: &str) -> Self {
        match s {
            "" | "home" => MoveDest::Home,
            "imported" => MoveDest::KeepImported,
            other => other.parse::<i64>().map(MoveDest::Section).unwrap_or(MoveDest::Home),
        }
    }
}

/// The user's sections with their active-note counts, ordered for the sidebar.
pub async fn list(pool: &Pool, user_id: i64) -> Result<Vec<Section>, AppError> {
    crate::db::run(pool, move |conn| {
        let mut stmt = conn.prepare(
            "SELECT s.id, s.name, COUNT(m.id)
             FROM sections s
             LEFT JOIN memos m ON m.section_id = s.id AND m.state = 'normal'
             WHERE s.user_id = ?1
             GROUP BY s.id, s.name
             ORDER BY s.position, s.name COLLATE NOCASE",
        )?;
        let rows = stmt
            .query_map([user_id], |r| {
                Ok(Section { id: r.get(0)?, name: r.get(1)?, count: r.get(2)?, active: false })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
    .await
}

/// Name of a section the user owns (for the section page header).
pub async fn name(pool: &Pool, user_id: i64, id: i64) -> Result<String, AppError> {
    crate::db::run(pool, move |conn| {
        conn.query_row(
            "SELECT name FROM sections WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
            |r| r.get::<_, String>(0),
        )
        .map_err(|_| AppError::NotFound)
    })
    .await
}

pub async fn create(pool: &Pool, user_id: i64, name: String) -> Result<i64, AppError> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest("Section name can't be empty".into()));
    }
    crate::db::run(pool, move |conn| {
        conn.execute(
            "INSERT INTO sections (user_id, name) VALUES (?1, ?2)",
            params![user_id, name],
        )
        .map_err(|_| AppError::BadRequest("A section with that name already exists".into()))?;
        Ok(conn.last_insert_rowid())
    })
    .await
}

/// Delete a section; its notes fall back to Home via ON DELETE SET NULL.
pub async fn delete(pool: &Pool, user_id: i64, id: i64) -> Result<(), AppError> {
    crate::db::run(pool, move |conn| {
        conn.execute(
            "DELETE FROM sections WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
        )?;
        Ok(())
    })
    .await
}

/// Move a note between Home / a section, converting an imported note to local
/// when it lands in a local bucket (guarded so it can't duplicate a uuid).
pub async fn set_note_section(
    pool: &Pool,
    user_id: i64,
    memo_id: i64,
    dest: MoveDest,
) -> Result<(), AppError> {
    crate::db::run(pool, move |conn| {
        let (origin, uuid): (String, String) = conn
            .query_row(
                "SELECT origin, uuid FROM memos WHERE id = ?1 AND user_id = ?2",
                params![memo_id, user_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|_| AppError::NotFound)?;

        let target_section: Option<i64> = match dest {
            MoveDest::KeepImported => return Ok(()),
            MoveDest::Home => None,
            MoveDest::Section(sid) => {
                let owned = conn
                    .query_row(
                        "SELECT 1 FROM sections WHERE id = ?1 AND user_id = ?2",
                        params![sid, user_id],
                        |_| Ok(()),
                    )
                    .is_ok();
                if !owned {
                    return Err(AppError::BadRequest("Unknown section".into()));
                }
                Some(sid)
            }
        };

        // Converting an imported note to local must not collide with an existing
        // local note carrying the same uuid.
        if origin == "imported" {
            let dup: i64 = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM memos
                   WHERE user_id = ?1 AND uuid = ?2 AND origin = 'local' AND id != ?3)",
                params![user_id, uuid, memo_id],
                |r| r.get(0),
            )?;
            if dup != 0 {
                return Err(AppError::BadRequest(
                    "A local note with this id already exists".into(),
                ));
            }
        }

        conn.execute(
            "UPDATE memos SET origin = 'local', section_id = ?1 WHERE id = ?2 AND user_id = ?3",
            params![target_section, memo_id, user_id],
        )?;
        Ok(())
    })
    .await
}
