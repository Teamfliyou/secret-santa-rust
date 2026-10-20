use askama::Template;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;

use crate::auth::{
    create_session, destroy_session, ensure_pre_csrf, load_session, removal_cookie, session_cookie,
    ClientIp, LOGIN_MAX_PER_IP, LOGIN_MAX_PER_USER, LOGIN_WINDOW, PRE_CSRF_COOKIE, SESSION_COOKIE,
};
use crate::error::AppError;
use crate::templates::{Ctx, LoginTemplate, SetPasswordTemplate};
use crate::util::{ct_eq, hash_password_async, verify_password_async};
use crate::web::page;
use crate::web::CsrfForm;
use crate::AppState;

pub async fn health(State(state): State<AppState>) -> (StatusCode, &'static str) {
    match sqlx::query_scalar::<_, i64>("SELECT 1::BIGINT")
        .fetch_one(&state.db)
        .await
    {
        Ok(_) => (StatusCode::OK, "ok"),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "db unavailable"),
    }
}

fn asset(content_type: &'static str, body: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        body,
    )
        .into_response()
}

pub async fn css() -> Response {
    asset(
        "text/css; charset=utf-8",
        include_str!("../../static/style.css"),
    )
}

pub async fn app_js() -> Response {
    asset(
        "text/javascript; charset=utf-8",
        include_str!("../../static/app.js"),
    )
}

pub async fn early_js() -> Response {
    asset(
        "text/javascript; charset=utf-8",
        include_str!("../../static/early.js"),
    )
}

pub async fn not_found() -> AppError {
    AppError::NotFound
}

#[derive(Deserialize)]
pub struct LoginForm {
    pub username: String,
    pub password: String,
    pub csrf: String,
}

pub async fn login_response(
    state: &AppState,
    jar: CookieJar,
    username: &str,
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response, AppError> {
    let (jar, token) = ensure_pre_csrf(jar, state);
    let mut ctx = Ctx::with_csrf("Connexion", &token);
    ctx.error = error.map(String::from);
    let html = LoginTemplate {
        ctx,
        username: username.to_string(),
    }
    .render()?;
    Ok((status, jar, Html(html)).into_response())
}

pub async fn login_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Response, AppError> {
    if let Some(s) = load_session(&headers, &state.db).await {
        let dest = if s.is_admin() {
            "/admin"
        } else if s.needs_password {
            "/set-password"
        } else {
            "/"
        };
        return Ok(Redirect::to(dest).into_response());
    }
    login_response(&state, jar, "", None, StatusCode::OK).await
}

/// Connexion unique pour élèves et administrateur : le rôle du compte décide de la suite.
/// Un compte élève sans mot de passe (première connexion ou réinitialisation) se connecte
/// avec son seul identifiant et doit alors choisir son mot de passe.
pub async fn login_post(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    jar: CookieJar,
    Form(form): Form<LoginForm>,
) -> Result<Response, AppError> {
    let state = &state;
    let username = form.username.trim().to_string();
    let pre = jar
        .get(PRE_CSRF_COOKIE)
        .map(|c| c.value().to_string())
        .unwrap_or_default();
    if pre.is_empty() || !ct_eq(&pre, &form.csrf) {
        return login_response(
            state,
            jar,
            &username,
            Some("Ta page avait expiré, réessaie."),
            StatusCode::BAD_REQUEST,
        )
        .await;
    }

    let ukey = format!("login:{}", username.to_lowercase());
    let ikey = format!("ip:{ip}");
    if state
        .limiter
        .is_blocked(&ukey, LOGIN_MAX_PER_USER, LOGIN_WINDOW)
        || state
            .limiter
            .is_blocked(&ikey, LOGIN_MAX_PER_IP, LOGIN_WINDOW)
    {
        return login_response(
            state,
            jar,
            &username,
            Some("Trop de tentatives. Patiente quelques minutes avant de réessayer."),
            StatusCode::TOO_MANY_REQUESTS,
        )
        .await;
    }

    let row: Option<(i64, String, String)> = if username.len() <= 64 {
        sqlx::query_as(
            "SELECT id, password_hash, role FROM users WHERE lower(username) = lower($1)",
        )
        .bind(&username)
        .fetch_optional(&state.db)
        .await?
    } else {
        None
    };

    let mut user: Option<(i64, bool)> = None; // (id, est admin)
    match row {
        Some((id, hash, role)) if hash.is_empty() && role == "student" => {
            user = Some((id, false));
        }
        Some((id, hash, role)) => {
            if form.password.len() <= 256
                && verify_password_async(hash, form.password.clone()).await
            {
                user = Some((id, role == "admin"));
            }
        }
        None => {
            // même coût de calcul que pour un vrai compte
            verify_password_async((*state.dummy_hash).clone(), form.password.clone()).await;
        }
    }
    let Some((user_id, is_admin)) = user else {
        state.limiter.record(&ukey);
        state.limiter.record(&ikey);
        return login_response(
            state,
            jar,
            &username,
            Some("Identifiant ou mot de passe incorrect."),
            StatusCode::UNAUTHORIZED,
        )
        .await;
    };

    state.limiter.clear(&ukey);
    let first_time = !is_admin && form.password.is_empty() && {
        let unset: bool = sqlx::query_scalar("SELECT password_hash = '' FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_one(&state.db)
            .await?;
        unset
    };
    let token = create_session(state, user_id).await?;
    let jar = jar
        .add(session_cookie(token, state))
        .add(removal_cookie(PRE_CSRF_COOKIE, state));
    let dest = if is_admin {
        "/admin"
    } else if first_time {
        "/set-password"
    } else {
        "/"
    };
    Ok((jar, Redirect::to(dest)).into_response())
}

pub async fn set_password_page(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let Some(s) = load_session(&headers, &state.db).await else {
        return Ok(Redirect::to("/login").into_response());
    };
    if !s.needs_password {
        return Ok(Redirect::to("/").into_response());
    }
    let tpl = SetPasswordTemplate {
        ctx: Ctx::session("Choisis ton mot de passe", &s),
        first_name: s.user.first_name.clone(),
    };
    Ok(page(tpl)?.into_response())
}

#[derive(Deserialize)]
pub struct SetPasswordForm {
    pub csrf: String,
    pub password: String,
    pub confirm: String,
}

pub async fn set_password_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<SetPasswordForm>,
) -> Result<Response, AppError> {
    let Some(s) = load_session(&headers, &state.db).await else {
        return Ok(Redirect::to("/login").into_response());
    };
    s.check_csrf(&f.csrf)?;
    if !s.needs_password {
        return Err(AppError::Forbidden);
    }
    let len = f.password.chars().count();
    let problem = if len < 8 {
        Some("Ton mot de passe doit faire au moins 8 caractères.")
    } else if len > 128 {
        Some("Ton mot de passe est trop long (128 caractères maximum).")
    } else if f.password != f.confirm {
        Some("Les deux mots de passe ne sont pas identiques.")
    } else {
        None
    };
    if let Some(p) = problem {
        let tpl = SetPasswordTemplate {
            ctx: Ctx::session("Choisis ton mot de passe", &s).error(p),
            first_name: s.user.first_name.clone(),
        };
        return Ok((StatusCode::UNPROCESSABLE_ENTITY, page(tpl)?).into_response());
    }
    let hash = hash_password_async(f.password).await;
    sqlx::query(
        "UPDATE users SET password_hash = $1, updated_at = iso_now()
         WHERE id = $2 AND password_hash = '' AND role = 'student'",
    )
    .bind(hash)
    .bind(s.user.id)
    .execute(&state.db)
    .await?;
    Ok(Redirect::to("/").into_response())
}

pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    let Some(session) = load_session(&headers, &state.db).await else {
        return Ok(Redirect::to("/login").into_response());
    };
    session.check_csrf(&form.csrf)?;
    if let Some(c) = jar.get(SESSION_COOKIE) {
        destroy_session(&state.db, c.value()).await?;
    }
    Ok((
        jar.add(removal_cookie(SESSION_COOKIE, &state)),
        Redirect::to("/login"),
    )
        .into_response())
}
