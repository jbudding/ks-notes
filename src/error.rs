use askama::Template;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

#[derive(Debug)]
pub enum AppError {
    NotFound,
    Unauthorized,
    Forbidden,
    BadRequest(String),
    Internal(String),
}

impl AppError {
    pub fn status(&self) -> StatusCode {
        match self {
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::Forbidden => StatusCode::FORBIDDEN,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn public_message(&self) -> String {
        match self {
            AppError::NotFound => "Not found".into(),
            AppError::Unauthorized => "Please sign in".into(),
            AppError::Forbidden => "You don't have access to this".into(),
            AppError::BadRequest(msg) => msg.clone(),
            AppError::Internal(_) => "Something went wrong on our side".into(),
        }
    }
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPage {
    status: u16,
    message: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        if let AppError::Internal(detail) = &self {
            tracing::error!(detail, "internal error");
        }
        let status = self.status();
        let page = ErrorPage {
            status: status.as_u16(),
            message: self.public_message(),
        };
        match page.render() {
            Ok(body) => (status, Html(body)).into_response(),
            Err(_) => (status, self.public_message()).into_response(),
        }
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        match e {
            rusqlite::Error::QueryReturnedNoRows => AppError::NotFound,
            other => AppError::Internal(format!("sqlite: {other}")),
        }
    }
}

impl From<askama::Error> for AppError {
    fn from(e: askama::Error) -> Self {
        AppError::Internal(format!("template: {e}"))
    }
}

/// Render an askama template into an HTML response, mapping failures to AppError.
pub fn render<T: Template>(t: &T) -> Result<Response, AppError> {
    Ok(Html(t.render()?).into_response())
}
