pub mod auth;
pub mod config;
pub mod draw;
pub mod error;
pub mod templates;
pub mod util;
pub mod web;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Request};
use axum::http::{header, HeaderValue};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;

use crate::auth::RateLimiter;
use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub cfg: Arc<Config>,
    pub limiter: Arc<RateLimiter>,
    /// Hash factice pour égaliser le temps de réponse quand l'identifiant n'existe pas.
    pub dummy_hash: Arc<String>,
}

pub async fn connect(path: &str) -> anyhow::Result<SqlitePool> {
    if let Some(parent) = Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(10));
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(opts)
        .await?;
    Ok(pool)
}

/// Ouvre la base, applique les migrations et crée / synchronise le compte administrateur.
pub async fn setup(cfg: Config) -> anyhow::Result<AppState> {
    let db = connect(&cfg.database_path).await?;
    sqlx::migrate!("./migrations").run(&db).await?;
    let dummy_hash = util::hash_password_async(util::random_token()).await;
    let state = AppState {
        db,
        cfg: Arc::new(cfg),
        limiter: Arc::new(RateLimiter::default()),
        dummy_hash: Arc::new(dummy_hash),
    };
    ensure_admin(&state).await?;
    Ok(state)
}

async fn ensure_admin(state: &AppState) -> anyhow::Result<()> {
    let username = state.cfg.admin_username.trim().to_string();
    let existing: Option<(i64, String)> =
        sqlx::query_as("SELECT id, password_hash FROM users WHERE role = 'admin' AND username = ?")
            .bind(&username)
            .fetch_optional(&state.db)
            .await?;
    match (existing, &state.cfg.admin_password) {
        (Some((id, hash)), Some(pw)) => {
            // ADMIN_PASSWORD fait foi : si elle change, on met à jour le hash.
            if !util::verify_password_async(hash, pw.clone()).await {
                let new_hash = util::hash_password_async(pw.clone()).await;
                sqlx::query("UPDATE users SET password_hash = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ','now') WHERE id = ?")
                    .bind(new_hash)
                    .bind(id)
                    .execute(&state.db)
                    .await?;
                sqlx::query("DELETE FROM sessions WHERE user_id = ?")
                    .bind(id)
                    .execute(&state.db)
                    .await?;
                tracing::info!("mot de passe administrateur mis à jour depuis ADMIN_PASSWORD");
            }
        }
        (Some(_), None) => {}
        (None, pw) => {
            let (password, generated) = match pw {
                Some(p) => (p.clone(), false),
                None => (util::gen_code(), true),
            };
            let hash = util::hash_password_async(password.clone()).await;
            sqlx::query("INSERT INTO users (username, first_name, last_name, password_hash, role) VALUES (?, 'Admin', 'Santa', ?, 'admin')")
                .bind(&username)
                .bind(hash)
                .execute(&state.db)
                .await?;
            if generated {
                tracing::warn!("=====================================================");
                tracing::warn!("Compte administrateur créé : identifiant « {username} », mot de passe « {password} »");
                tracing::warn!(
                    "Notez-le maintenant (il ne sera plus affiché) ou définissez ADMIN_PASSWORD."
                );
                tracing::warn!("=====================================================");
            } else {
                tracing::info!("compte administrateur « {username} » créé");
            }
        }
    }
    Ok(())
}

/// Supprime les sessions expirées.
pub async fn purge_expired_sessions(db: &SqlitePool) {
    if let Err(e) = sqlx::query(
        "DELETE FROM sessions WHERE expires_at <= strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
    )
    .execute(db)
    .await
    {
        tracing::warn!("nettoyage des sessions impossible : {e}");
    }
}

async fn security_headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self'; \
             form-action 'self'; frame-ancestors 'none'; base-uri 'self'; object-src 'none'",
        ),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    if !h.contains_key(header::CACHE_CONTROL) {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
}

pub fn router(state: AppState) -> Router {
    use web::{admin, public, student};
    Router::new()
        .route("/health", get(public::health))
        .route("/static/style.css", get(public::css))
        .route("/static/app.js", get(public::app_js))
        .route("/static/early.js", get(public::early_js))
        .route("/login", get(public::login_page).post(public::login_post))
        .route("/logout", post(public::logout))
        .route("/", get(student::dashboard))
        .route(
            "/preferences",
            get(student::preferences_page).post(student::preferences_save),
        )
        .route("/preferences/saved", get(student::preferences_saved))
        .route("/reveal", post(student::reveal))
        .route("/recipient", get(student::recipient))
        .route("/admin", get(admin::dashboard))
        .route("/admin/login", get(admin::login_redirect))
        .route(
            "/set-password",
            get(public::set_password_page).post(public::set_password_save),
        )
        .route(
            "/admin/students",
            get(admin::students).post(admin::student_add),
        )
        .route("/admin/students/import", post(admin::students_import))
        .route(
            "/admin/students/:id/edit",
            get(admin::student_edit_page).post(admin::student_edit),
        )
        .route("/admin/students/:id/reset", post(admin::student_reset))
        .route("/admin/students/:id/delete", post(admin::student_delete))
        .route(
            "/admin/settings",
            get(admin::settings_page).post(admin::settings_save),
        )
        .route("/admin/draw", get(admin::draw_page).post(admin::draw_run))
        .route("/admin/draw/reveal", post(admin::reveal_toggle))
        .route(
            "/admin/assignments",
            get(admin::assignments_page).post(admin::assignments_show),
        )
        .route("/admin/audit", get(admin::audit_log_page))
        .fallback(public::not_found)
        .layer(DefaultBodyLimit::max(512 * 1024))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}
