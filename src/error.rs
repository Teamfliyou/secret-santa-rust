use askama::Template;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

use crate::templates::{Ctx, ErrorTemplate};

#[derive(Debug)]
pub enum AppError {
    Db(sqlx::Error),
    Forbidden,
    NotFound,
    BadRequest(String),
    Internal(String),
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        AppError::Db(e)
    }
}

impl From<askama::Error> for AppError {
    fn from(e: askama::Error) -> Self {
        AppError::Internal(e.to_string())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::Forbidden => (StatusCode::FORBIDDEN, "Accès refusé.".to_string()),
            AppError::NotFound => (StatusCode::NOT_FOUND, "Page introuvable.".to_string()),
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            AppError::Db(e) => {
                tracing::error!("erreur base de données : {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Une erreur est survenue. Réessaie dans un instant.".into(),
                )
            }
            AppError::Internal(e) => {
                tracing::error!("erreur interne : {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Une erreur est survenue. Réessaie dans un instant.".into(),
                )
            }
        };
        let tpl = ErrorTemplate {
            ctx: Ctx::anon("Oups"),
            code: status.as_u16(),
            message,
        };
        match tpl.render() {
            Ok(html) => (status, Html(html)).into_response(),
            Err(_) => status.into_response(),
        }
    }
}
