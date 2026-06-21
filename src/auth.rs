use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Redirect, Response};
use data_encoding::{BASE64URL_NOPAD, HEXLOWER};
use rand::prelude::*;
use sha2::{Digest, Sha256};

use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;

pub const SESSION_COOKIE: &str = "ks_session";
pub const SESSION_TTL_SECS: i64 = 30 * 24 * 3600;
pub const API_TOKEN_PREFIX: &str = "ksn_";

// ---------- Passwords (CPU-heavy; callers run these inside the blocking pool) ----------

pub fn hash_password(password: &str) -> Result<String, AppError> {
    let mut salt_bytes = [0u8; 16];
    rand::rng().fill(&mut salt_bytes);
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|e| AppError::Internal(format!("salt: {e}")))?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AppError::Internal(format!("hashing password: {e}")))
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

// ---------- Tokens ----------

/// 32 random bytes, base64url — used for session cookies, CSRF, and API tokens.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    BASE64URL_NOPAD.encode(&bytes)
}

/// Only SHA-256 digests of tokens are stored, so a leaked DB can't replay them.
pub fn token_hash(token: &str) -> String {
    HEXLOWER.encode(&Sha256::digest(token.as_bytes()))
}

pub fn new_api_token() -> String {
    format!("{API_TOKEN_PREFIX}{}", random_token())
}

/// 12-char base62 uid for memo/resource URLs.
pub fn new_uid() -> String {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::rng();
    (0..12)
        .map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char)
        .collect()
}

/// A random UUIDv4 string — the stable cross-instance identity stamped on each
/// note at creation so imports can dedup instead of duplicating.
pub fn new_uuid() -> String {
    let mut b = [0u8; 16];
    rand::rng().fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // variant 1
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15],
    )
}

pub fn constant_time_eq(a: &str, b: &str) -> bool {
    use subtle_eq::ct_eq;
    ct_eq(a.as_bytes(), b.as_bytes())
}

mod subtle_eq {
    pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
        if a.len() != b.len() {
            return false;
        }
        let mut diff = 0u8;
        for (x, y) in a.iter().zip(b.iter()) {
            diff |= x ^ y;
        }
        diff == 0
    }
}

// ---------- Cookie helpers ----------

pub fn session_cookie(token: &str, secure: bool, max_age_secs: i64) -> String {
    let mut cookie = format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_secs}"
    );
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

pub fn clear_session_cookie(secure: bool) -> String {
    session_cookie("", secure, 0)
}

pub fn cookie_from_headers(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(axum::http::header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

fn cookie_value(parts: &Parts, name: &str) -> Option<String> {
    cookie_from_headers(&parts.headers, name)
}

// ---------- Extractors ----------

#[derive(Debug, Clone)]
pub struct SessionUser {
    pub user: User,
    pub csrf_token: String,
}

async fn lookup_session(state: &AppState, parts: &Parts) -> Option<SessionUser> {
    let token = cookie_value(parts, SESSION_COOKIE)?;
    if token.is_empty() {
        return None;
    }
    let hash = token_hash(&token);
    crate::db::sessions::lookup(&state.pool, hash).await.ok()?
}

/// Current user if a valid session cookie is present.
pub struct MaybeUser(pub Option<SessionUser>);

impl FromRequestParts<AppState> for MaybeUser {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(MaybeUser(lookup_session(state, parts).await))
    }
}

/// Requires a session; browsers get redirected to /login.
pub struct AuthUser(pub SessionUser);

pub enum AuthRejection {
    RedirectToLogin,
}

impl IntoResponse for AuthRejection {
    fn into_response(self) -> Response {
        Redirect::to("/login").into_response()
    }
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AuthRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        lookup_session(state, parts)
            .await
            .map(AuthUser)
            .ok_or(AuthRejection::RedirectToLogin)
    }
}

/// Requires an admin session.
pub struct AdminUser(pub SessionUser);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let session = lookup_session(state, parts)
            .await
            .ok_or_else(|| AuthRejection::RedirectToLogin.into_response())?;
        if !session.user.is_admin() {
            return Err(AppError::Forbidden.into_response());
        }
        Ok(AdminUser(session))
    }
}

/// CSRF guard for mutating cookie-authenticated routes. Checks the
/// `X-CSRF-Token` header (set globally on the body by htmx); plain HTML forms
/// instead submit a `csrf_token` field validated by `require_csrf_field`.
pub struct CsrfGuard(pub SessionUser);

impl FromRequestParts<AppState> for CsrfGuard {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let session = lookup_session(state, parts)
            .await
            .ok_or_else(|| AuthRejection::RedirectToLogin.into_response())?;
        let header = parts
            .headers
            .get("x-csrf-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !constant_time_eq(header, &session.csrf_token) {
            return Err(AppError::Forbidden.into_response());
        }
        Ok(CsrfGuard(session))
    }
}

/// For plain (non-htmx) form posts carrying `csrf_token` as a field.
pub fn require_csrf_field(session: &SessionUser, submitted: &str) -> Result<(), AppError> {
    if constant_time_eq(submitted, &session.csrf_token) {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// API auth: `Authorization: Bearer ksn_…` only. Cookies are deliberately
/// ignored, which is what lets `/api/v1/*` skip CSRF checks.
pub struct ApiUser(pub User);

impl FromRequestParts<AppState> for ApiUser {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let unauthorized = || {
            (
                axum::http::StatusCode::UNAUTHORIZED,
                axum::Json(serde_json::json!({"error": "missing or invalid API token"})),
            )
                .into_response()
        };
        let token = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|t| t.starts_with(API_TOKEN_PREFIX))
            .ok_or_else(unauthorized)?;
        let user = crate::db::tokens::lookup_user(&state.pool, token_hash(token))
            .await
            .map_err(|e| e.into_response())?
            .ok_or_else(unauthorized)?;
        Ok(ApiUser(user))
    }
}
