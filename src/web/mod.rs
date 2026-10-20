pub mod admin;
pub mod public;
pub mod student;

use askama::Template;
use axum::response::Html;
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::auth::Session;
use crate::error::AppError;
use crate::templates::Settings;

#[derive(Deserialize)]
pub struct CsrfForm {
    pub csrf: String,
}

pub fn page<T: Template>(t: T) -> Result<Html<String>, AppError> {
    Ok(Html(t.render()?))
}

pub async fn load_settings(db: &SqlitePool) -> Result<Settings, AppError> {
    Ok(sqlx::query_as::<_, Settings>(
        "SELECT class_name, event_name, budget, exchange_date, welcome_message, draw_completed_at, reveal_enabled
         FROM event_settings WHERE id = 1",
    )
    .fetch_one(db)
    .await?)
}

pub async fn audit(
    db: &SqlitePool,
    s: &Session,
    action: &str,
    details: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO audit_log (actor_id, actor_name, action, details) VALUES (?, ?, ?, ?)",
    )
    .bind(s.user.id)
    .bind(&s.user.username)
    .bind(action)
    .bind(details)
    .execute(db)
    .await?;
    Ok(())
}
