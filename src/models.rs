use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use serde::{Deserialize, Serialize};

macro_rules! text_enum {
    ($name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name {
            $(#[serde(rename = $text)] $variant),+
        }

        impl $name {
            pub fn as_str(&self) -> &'static str {
                match self { $($name::$variant => $text),+ }
            }

            pub fn parse(s: &str) -> Option<Self> {
                match s { $($text => Some($name::$variant)),+, _ => None }
            }
        }

        impl FromSql for $name {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                let s = value.as_str()?;
                Self::parse(s).ok_or(FromSqlError::InvalidType)
            }
        }

        impl ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                Ok(self.as_str().into())
            }
        }
    };
}

text_enum!(Role {
    Admin => "admin",
    User => "user",
});

text_enum!(Visibility {
    Private => "private",
    Protected => "protected",
    Public => "public",
});

text_enum!(MemoState {
    Normal => "normal",
    Archived => "archived",
});

text_enum!(MemoOrigin {
    Local => "local",
    Imported => "imported",
});

#[derive(Debug, Clone)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub role: Role,
}

impl User {
    pub fn is_admin(&self) -> bool {
        self.role == Role::Admin
    }
}

#[derive(Debug, Clone)]
pub struct Memo {
    pub id: i64,
    pub uid: String,
    pub uuid: String,
    pub user_id: i64,
    pub username: String,
    pub content: String,
    pub visibility: Visibility,
    pub pinned: bool,
    pub state: MemoState,
    pub origin: MemoOrigin,
    pub section_id: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A user-defined sidebar section, with its active-note count.
#[derive(Debug, Clone)]
pub struct Section {
    pub id: i64,
    pub name: String,
    pub count: i64,
    pub pinned: bool,
    /// Marks the current feed's section (sidebar highlight) or the editing
    /// note's section (editor select) — set by the caller, not the query.
    pub active: bool,
}

/// One attachment in the export/import JSON file. `data` is base64 (standard
/// alphabet). Attachments have no id of their own — they're recreated as
/// children of their note, which is deduped by `uuid`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportAttachment {
    /// The resource uid the note's `{{attach:UID}}` token refers to. Absent in
    /// v1/v2 files (block attachments) — import then appends a fresh token.
    #[serde(default)]
    pub uid: String,
    pub filename: String,
    pub content_type: String,
    pub created_at: i64,
    pub data: String,
}

/// One note in the export/import JSON file. `uuid` is the cross-instance
/// identity used to dedup on import; tags are re-derived from `content`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportNote {
    pub uuid: String,
    pub content: String,
    pub visibility: Visibility,
    pub created_at: i64,
    pub updated_at: i64,
    /// Absent in v1 export files — defaults to no attachments.
    #[serde(default)]
    pub attachments: Vec<ExportAttachment>,
}

/// Top-level shape of an export file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportFile {
    pub version: u32,
    pub notes: Vec<ExportNote>,
}

#[derive(Debug, Clone)]
pub struct ResourceMeta {
    pub uid: String,
    pub filename: String,
    pub content_type: String,
    pub size: i64,
}

#[derive(Debug, Clone)]
pub struct TagCount {
    pub tag: String,
    pub count: i64,
}

/// Active-note counts shown next to the Home and Imported nav items.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoteCounts {
    pub home: i64,
    pub imported: i64,
}
