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
    pub user_id: i64,
    pub username: String,
    pub content: String,
    pub visibility: Visibility,
    pub pinned: bool,
    pub state: MemoState,
    pub created_at: i64,
    pub updated_at: i64,
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
