use axum::Form;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::auth::{self, AuthUser};
use crate::db;
use crate::error::AppError;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct CreateForm {
    csrf_token: String,
    name: String,
}

pub async fn create(
    State(state): State<AppState>,
    AuthUser(session): AuthUser,
    Form(form): Form<CreateForm>,
) -> Result<Response, AppError> {
    auth::require_csrf_field(&session, &form.csrf_token)?;
    let id = db::sections::create(&state.pool, session.user.id, form.name).await?;
    Ok(Redirect::to(&format!("/s/{id}")).into_response())
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf_token: String,
}

pub async fn delete(
    State(state): State<AppState>,
    AuthUser(session): AuthUser,
    Path(id): Path<i64>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    auth::require_csrf_field(&session, &form.csrf_token)?;
    db::sections::delete(&state.pool, session.user.id, id).await?;
    // Its notes fall back to Home; go there.
    Ok(Redirect::to("/").into_response())
}
