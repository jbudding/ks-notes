use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::auth::{self, AdminUser, AuthUser, SessionUser};
use crate::db;
use crate::error::{AppError, render};
use crate::models::TagCount;
use crate::state::AppState;
use crate::views::format_times;

struct TokenRow {
    id: i64,
    name: String,
    created_iso: String,
    created_display: String,
    last_used_display: Option<String>,
}

#[derive(Template)]
#[template(path = "settings.html")]
struct SettingsPage {
    username: String,
    is_admin: bool,
    csrf_token: String,
    nav_active: &'static str,
    counts: crate::models::NoteCounts,
    tags: Vec<TagCount>,
    tag_filter: Option<String>,
    tokens: Vec<TokenRow>,
    message: Option<String>,
    error: Option<String>,
    new_token: Option<String>,
}

async fn settings_page_data(
    state: &AppState,
    session: &SessionUser,
    message: Option<String>,
    error: Option<String>,
    new_token: Option<String>,
) -> Result<SettingsPage, AppError> {
    let tokens = db::tokens::list(&state.pool, session.user.id)
        .await?
        .into_iter()
        .map(|t| {
            let (created_iso, created_display) = format_times(t.created_at);
            TokenRow {
                id: t.id,
                name: t.name,
                created_iso,
                created_display,
                last_used_display: t.last_used_at.map(|ts| format_times(ts).1),
            }
        })
        .collect();
    Ok(SettingsPage {
        username: session.user.username.clone(),
        is_admin: session.user.is_admin(),
        csrf_token: session.csrf_token.clone(),
        nav_active: "settings",
        counts: db::memos::note_counts(&state.pool, session.user.id).await?,
        tags: db::memos::tag_counts(&state.pool, session.user.id).await?,
        tag_filter: None,
        tokens,
        message,
        error,
        new_token,
    })
}

pub async fn settings(
    State(state): State<AppState>,
    AuthUser(session): AuthUser,
) -> Result<Response, AppError> {
    render(&settings_page_data(&state, &session, None, None, None).await?)
}

#[derive(Deserialize)]
pub struct PasswordForm {
    csrf_token: String,
    current_password: String,
    new_password: String,
    new_password_confirm: String,
}

pub async fn change_password(
    State(state): State<AppState>,
    AuthUser(session): AuthUser,
    headers: axum::http::HeaderMap,
    Form(form): Form<PasswordForm>,
) -> Result<Response, AppError> {
    auth::require_csrf_field(&session, &form.csrf_token)?;

    let fail = |state: &AppState, session: &SessionUser, msg: &str| {
        let msg = msg.to_string();
        let state = state.clone();
        let session = session.clone();
        async move {
            render(&settings_page_data(&state, &session, None, Some(msg), None).await?)
        }
    };

    if form.new_password.len() < 8 {
        return fail(&state, &session, "New password must be at least 8 characters").await;
    }
    if form.new_password != form.new_password_confirm {
        return fail(&state, &session, "New passwords don't match").await;
    }
    let current_hash = db::users::get_password_hash(&state.pool, session.user.id).await?;
    let current = form.current_password;
    let ok = tokio::task::spawn_blocking(move || auth::verify_password(&current, &current_hash))
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))?;
    if !ok {
        return fail(&state, &session, "Current password is incorrect").await;
    }
    let new_password = form.new_password;
    let new_hash = tokio::task::spawn_blocking(move || auth::hash_password(&new_password))
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))??;
    db::users::update_password(&state.pool, session.user.id, new_hash).await?;

    // Sign out every other session, keeping this one.
    if let Some(token) = auth::cookie_from_headers(&headers, auth::SESSION_COOKIE) {
        db::sessions::delete_all_for_user_except(
            &state.pool,
            session.user.id,
            auth::token_hash(&token),
        )
        .await?;
    }
    render(
        &settings_page_data(
            &state,
            &session,
            Some("Password updated. Other sessions have been signed out.".into()),
            None,
            None,
        )
        .await?,
    )
}

#[derive(Deserialize)]
pub struct TokenForm {
    csrf_token: String,
    name: String,
}

pub async fn create_token(
    State(state): State<AppState>,
    AuthUser(session): AuthUser,
    Form(form): Form<TokenForm>,
) -> Result<Response, AppError> {
    auth::require_csrf_field(&session, &form.csrf_token)?;
    let name = form.name.trim().chars().take(64).collect::<String>();
    if name.is_empty() {
        return Err(AppError::BadRequest("Token name is required".into()));
    }
    let token = auth::new_api_token();
    db::tokens::create(&state.pool, session.user.id, name, auth::token_hash(&token)).await?;
    render(&settings_page_data(&state, &session, None, None, Some(token)).await?)
}

#[derive(Deserialize)]
pub struct CsrfOnlyForm {
    csrf_token: String,
}

pub async fn revoke_token(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    AuthUser(session): AuthUser,
    Form(form): Form<CsrfOnlyForm>,
) -> Result<Response, AppError> {
    auth::require_csrf_field(&session, &form.csrf_token)?;
    db::tokens::revoke(&state.pool, session.user.id, id).await?;
    Ok(Redirect::to("/settings").into_response())
}

// ---------- Admin ----------

struct UserRow {
    username: String,
    role: &'static str,
    created_iso: String,
    created_display: String,
}

#[derive(Template)]
#[template(path = "admin.html")]
struct AdminPage {
    username: String,
    is_admin: bool,
    csrf_token: String,
    nav_active: &'static str,
    counts: crate::models::NoteCounts,
    tags: Vec<TagCount>,
    tag_filter: Option<String>,
    allow_registration: bool,
    users: Vec<UserRow>,
}

pub async fn admin(
    State(state): State<AppState>,
    AdminUser(session): AdminUser,
) -> Result<Response, AppError> {
    let users = db::users::list(&state.pool)
        .await?
        .into_iter()
        .map(|(u, created_at)| {
            let (created_iso, created_display) = format_times(created_at);
            UserRow {
                username: u.username,
                role: u.role.as_str(),
                created_iso,
                created_display,
            }
        })
        .collect();
    // registration_open() treats an empty instance as open; the admin page
    // shows the actual stored setting instead.
    let allow_registration = crate::db::run(&state.pool, |conn| {
        db::settings::allow_registration_sync(conn).map_err(AppError::from)
    })
    .await?;
    render(&AdminPage {
        username: session.user.username.clone(),
        is_admin: true,
        csrf_token: session.csrf_token.clone(),
        nav_active: "admin",
        counts: db::memos::note_counts(&state.pool, session.user.id).await?,
        tags: db::memos::tag_counts(&state.pool, session.user.id).await?,
        tag_filter: None,
        allow_registration,
        users,
    })
}

#[derive(Deserialize)]
pub struct RegistrationForm {
    csrf_token: String,
    enabled: String,
}

pub async fn set_registration(
    State(state): State<AppState>,
    AdminUser(session): AdminUser,
    Form(form): Form<RegistrationForm>,
) -> Result<Response, AppError> {
    auth::require_csrf_field(&session, &form.csrf_token)?;
    let value = if form.enabled == "true" { "true" } else { "false" };
    db::settings::set(&state.pool, db::settings::ALLOW_REGISTRATION, value.into()).await?;
    Ok(Redirect::to("/admin").into_response())
}
