use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::async_trait;
use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::header::HeaderMap;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use sqlx::SqlitePool;

use crate::error::AppError;
use crate::util::{ct_eq, random_token, sha256_hex};
use crate::AppState;

pub const SESSION_COOKIE: &str = "sid";
pub const PRE_CSRF_COOKIE: &str = "csrf_pre";

#[derive(Clone, Debug)]
pub struct CurrentUser {
    pub id: i64,
    pub username: String,
    pub first_name: String,
    pub last_name: String,
    pub role: String,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub user: CurrentUser,
    pub needs_password: bool,
    pub csrf: String,
}

impl Session {
    pub fn is_admin(&self) -> bool {
        self.user.role == "admin"
    }

    /// Vérifie le jeton CSRF d'un formulaire POST.
    pub fn check_csrf(&self, given: &str) -> Result<(), AppError> {
        if ct_eq(&self.csrf, given) {
            Ok(())
        } else {
            Err(AppError::Forbidden)
        }
    }
}

fn base_cookie(name: &'static str, value: String, secure: bool) -> Cookie<'static> {
    Cookie::build((name, value))
        .path("/")
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .build()
}

pub fn session_cookie(token: String, state: &AppState) -> Cookie<'static> {
    let mut c = base_cookie(SESSION_COOKIE, token, state.cfg.cookie_secure);
    c.set_max_age(time::Duration::days(state.cfg.session_days));
    c
}

pub fn removal_cookie(name: &'static str, state: &AppState) -> Cookie<'static> {
    let mut c = base_cookie(name, String::new(), state.cfg.cookie_secure);
    c.make_removal();
    c
}

/// Jeton CSRF « avant connexion » (double-submit cookie) pour les formulaires de login.
pub fn ensure_pre_csrf(jar: CookieJar, state: &AppState) -> (CookieJar, String) {
    if let Some(c) = jar.get(PRE_CSRF_COOKIE) {
        if c.value().len() == 64 {
            let v = c.value().to_string();
            return (jar, v);
        }
    }
    let token = random_token();
    let jar = jar.add(base_cookie(
        PRE_CSRF_COOKIE,
        token.clone(),
        state.cfg.cookie_secure,
    ));
    (jar, token)
}

pub async fn create_session(state: &AppState, user_id: i64) -> Result<String, AppError> {
    let token = random_token();
    let csrf = random_token();
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, csrf_token, expires_at)
         VALUES (?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?))",
    )
    .bind(sha256_hex(&token))
    .bind(user_id)
    .bind(csrf)
    .bind(format!("+{} days", state.cfg.session_days))
    .execute(&state.db)
    .await?;
    Ok(token)
}

pub async fn destroy_session(db: &SqlitePool, token: &str) -> Result<(), AppError> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
        .bind(sha256_hex(token))
        .execute(db)
        .await?;
    Ok(())
}

pub async fn load_session(headers: &HeaderMap, db: &SqlitePool) -> Option<Session> {
    let jar = CookieJar::from_headers(headers);
    let token = jar.get(SESSION_COOKIE)?.value().to_string();
    if token.len() != 64 {
        return None;
    }
    #[allow(clippy::type_complexity)]
    let row: Option<(i64, String, String, String, String, String, bool)> = sqlx::query_as(
        "SELECT u.id, u.username, u.first_name, u.last_name, u.role, s.csrf_token, (u.password_hash = '')
         FROM sessions s JOIN users u ON u.id = s.user_id
         WHERE s.token_hash = ? AND s.expires_at > strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
    )
    .bind(sha256_hex(&token))
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    row.map(
        |(id, username, first_name, last_name, role, csrf, unset)| Session {
            user: CurrentUser {
                id,
                username,
                first_name,
                last_name,
                role: role.clone(),
            },
            needs_password: unset && role == "student",
            csrf,
        },
    )
}

/// Session d'un élève. Redirige vers /login sinon.
pub struct StudentSession(pub Session);

#[async_trait]
impl FromRequestParts<AppState> for StudentSession {
    type Rejection = Response;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        match load_session(&parts.headers, &state.db).await {
            Some(s) if s.needs_password => Err(Redirect::to("/set-password").into_response()),
            Some(s) if !s.is_admin() => Ok(StudentSession(s)),
            Some(_) => Err(Redirect::to("/admin").into_response()),
            None => Err(Redirect::to("/login").into_response()),
        }
    }
}

/// Session d'administrateur. Redirige vers /admin/login sinon, 403 pour un élève.
pub struct AdminSession(pub Session);

#[async_trait]
impl FromRequestParts<AppState> for AdminSession {
    type Rejection = Response;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        match load_session(&parts.headers, &state.db).await {
            Some(s) if s.is_admin() => Ok(AdminSession(s)),
            Some(_) => Err(AppError::Forbidden.into_response()),
            None => Err(Redirect::to("/login").into_response()),
        }
    }
}

pub struct ClientIp(pub String);

#[async_trait]
impl FromRequestParts<AppState> for ClientIp {
    type Rejection = Infallible;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Infallible> {
        let mut ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip().to_string())
            .unwrap_or_else(|| "inconnu".into());
        if state.cfg.trust_proxy {
            if let Some(fwd) = parts
                .headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
            {
                if let Some(last) = fwd.rsplit(',').next() {
                    let last = last.trim();
                    if !last.is_empty() {
                        ip = last.to_string();
                    }
                }
            }
        }
        Ok(ClientIp(ip))
    }
}

/// Limiteur de tentatives en mémoire (fenêtre glissante).
#[derive(Default)]
pub struct RateLimiter {
    hits: Mutex<HashMap<String, Vec<Instant>>>,
}

impl RateLimiter {
    pub fn is_blocked(&self, key: &str, max: usize, window: Duration) -> bool {
        let mut map = self.hits.lock().unwrap();
        let now = Instant::now();
        match map.get_mut(key) {
            Some(v) => {
                v.retain(|t| now.duration_since(*t) < window);
                v.len() >= max
            }
            None => false,
        }
    }

    pub fn record(&self, key: &str) {
        let mut map = self.hits.lock().unwrap();
        map.entry(key.to_string()).or_default().push(Instant::now());
        if map.len() > 10_000 {
            let now = Instant::now();
            map.retain(|_, v| {
                v.last()
                    .is_some_and(|t| now.duration_since(*t) < Duration::from_secs(3600))
            });
        }
    }

    pub fn clear(&self, key: &str) {
        self.hits.lock().unwrap().remove(key);
    }
}

pub const LOGIN_MAX_PER_USER: usize = 5;
pub const LOGIN_MAX_PER_IP: usize = 30;
pub const LOGIN_WINDOW: Duration = Duration::from_secs(15 * 60);
