use askama::Template;
use axum::Form;
use axum::extract::State;
use axum::http::HeaderValue;
use axum::http::header::SET_COOKIE;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::auth::{self, MaybeUser};
use crate::db;
use crate::error::{AppError, render};
use crate::models::User;
use crate::state::AppState;

#[derive(Template)]
#[template(path = "login.html")]
struct LoginPage {
    error: Option<String>,
    registration_open: bool,
}

#[derive(Template)]
#[template(path = "register.html")]
struct RegisterPage {
    error: Option<String>,
    first_user: bool,
}

pub async fn login_page(
    State(state): State<AppState>,
    MaybeUser(user): MaybeUser,
) -> Result<Response, AppError> {
    if user.is_some() {
        return Ok(Redirect::to("/").into_response());
    }
    if db::users::count(&state.pool).await? == 0 {
        return Ok(Redirect::to("/register").into_response());
    }
    let registration_open = db::settings::registration_open(&state.pool).await?;
    render(&LoginPage {
        error: None,
        registration_open,
    })
}

#[derive(Deserialize)]
pub struct LoginForm {
    username: String,
    password: String,
}

pub async fn login_submit(
    State(state): State<AppState>,
    Form(form): Form<LoginForm>,
) -> Result<Response, AppError> {
    let username = form.username.trim().to_string();
    let verified = match db::users::get_for_login(&state.pool, username).await? {
        Some((user, hash)) => {
            let password = form.password;
            spawn_verify(password, hash).await?.then_some(user)
        }
        None => None,
    };
    match verified {
        Some(user) => start_session(&state, &user).await,
        None => {
            let registration_open = db::settings::registration_open(&state.pool).await?;
            render(&LoginPage {
                error: Some("Invalid username or password".into()),
                registration_open,
            })
        }
    }
}

pub async fn register_page(
    State(state): State<AppState>,
    MaybeUser(user): MaybeUser,
) -> Result<Response, AppError> {
    if user.is_some() {
        return Ok(Redirect::to("/").into_response());
    }
    if !db::settings::registration_open(&state.pool).await? {
        return Err(AppError::Forbidden);
    }
    let first_user = db::users::count(&state.pool).await? == 0;
    render(&RegisterPage {
        error: None,
        first_user,
    })
}

#[derive(Deserialize)]
pub struct RegisterForm {
    username: String,
    password: String,
    password_confirm: String,
}

fn valid_username(name: &str) -> bool {
    (3..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
}

pub async fn register_submit(
    State(state): State<AppState>,
    Form(form): Form<RegisterForm>,
) -> Result<Response, AppError> {
    let username = form.username.trim().to_string();
    let first_user = db::users::count(&state.pool).await? == 0;
    let fail = |msg: &str, first_user: bool| {
        render(&RegisterPage {
            error: Some(msg.into()),
            first_user,
        })
    };
    if !valid_username(&username) {
        return fail(
            "Username must be 3–32 characters: letters, digits, _ . -",
            first_user,
        );
    }
    if form.password.len() < 8 {
        return fail("Password must be at least 8 characters", first_user);
    }
    if form.password != form.password_confirm {
        return fail("Passwords don't match", first_user);
    }

    let hash = spawn_hash(form.password).await?;
    match db::users::register(&state.pool, username, hash).await? {
        db::users::RegisterOutcome::Created(user) => start_session(&state, &user).await,
        db::users::RegisterOutcome::RegistrationClosed => Err(AppError::Forbidden),
        db::users::RegisterOutcome::UsernameTaken => {
            fail("That username is already taken", first_user)
        }
    }
}

#[derive(Deserialize)]
pub struct LogoutForm {
    csrf_token: String,
}

pub async fn logout(
    State(state): State<AppState>,
    auth_user: crate::auth::AuthUser,
    headers: axum::http::HeaderMap,
    Form(form): Form<LogoutForm>,
) -> Result<Response, AppError> {
    auth::require_csrf_field(&auth_user.0, &form.csrf_token)?;
    if let Some(token) = auth::cookie_from_headers(&headers, auth::SESSION_COOKIE) {
        db::sessions::delete_by_hash(&state.pool, auth::token_hash(&token)).await?;
    }
    let mut resp = Redirect::to("/login").into_response();
    resp.headers_mut().append(
        SET_COOKIE,
        HeaderValue::from_str(&auth::clear_session_cookie(state.config.secure_cookies))
            .map_err(|e| AppError::Internal(format!("cookie header: {e}")))?,
    );
    Ok(resp)
}

async fn start_session(state: &AppState, user: &User) -> Result<Response, AppError> {
    let token = auth::random_token();
    let csrf = auth::random_token();
    db::sessions::create(&state.pool, user.id, auth::token_hash(&token), csrf).await?;
    let cookie = auth::session_cookie(&token, state.config.secure_cookies, auth::SESSION_TTL_SECS);
    let mut resp = Redirect::to("/").into_response();
    resp.headers_mut().append(
        SET_COOKIE,
        HeaderValue::from_str(&cookie)
            .map_err(|e| AppError::Internal(format!("cookie header: {e}")))?,
    );
    Ok(resp)
}

async fn spawn_hash(password: String) -> Result<String, AppError> {
    tokio::task::spawn_blocking(move || auth::hash_password(&password))
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))?
}

async fn spawn_verify(password: String, hash: String) -> Result<bool, AppError> {
    tokio::task::spawn_blocking(move || auth::verify_password(&password, &hash))
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))
}
