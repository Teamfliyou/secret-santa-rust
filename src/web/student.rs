use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use serde::Deserialize;

use crate::auth::StudentSession;
use crate::error::AppError;
use crate::templates::{
    Ctx, DashboardTemplate, PrefForm, PreferencesTemplate, RecipientTemplate, RecipientView,
    SavedTemplate,
};
use crate::util::{clean, fr_date};
use crate::web::{load_settings, page, CsrfForm};
use crate::AppState;

async fn prefs_completed(state: &AppState, user_id: i64) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM preferences WHERE user_id = ? AND completed_at IS NOT NULL)",
    )
    .bind(user_id)
    .fetch_one(&state.db)
    .await?)
}

pub async fn dashboard(
    State(state): State<AppState>,
    StudentSession(s): StudentSession,
) -> Result<Response, AppError> {
    if !prefs_completed(&state, s.user.id).await? {
        return Ok(Redirect::to("/preferences").into_response());
    }
    let settings = load_settings(&state.db).await?;
    let revealed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM assignments WHERE giver_id = ? AND revealed_at IS NOT NULL)",
    )
    .bind(s.user.id)
    .fetch_one(&state.db)
    .await?;
    let tpl = DashboardTemplate {
        ctx: Ctx::session("Mon espace", &s),
        first_name: s.user.first_name.clone(),
        exchange_date: fr_date(&settings.exchange_date),
        reveal_open: settings.draw_completed_at.is_some() && settings.reveal_enabled,
        revealed,
        s: settings,
    };
    Ok(page(tpl)?.into_response())
}

async fn load_prefs(state: &AppState, user_id: i64) -> Result<Option<PrefForm>, AppError> {
    #[allow(clippy::type_complexity)]
    let row: Option<(String, String, String, String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT interests, hobbies, gift_types, favorite_snacks, favorite_colors, wishlist, dislikes,
                food_restrictions, message FROM preferences WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;
    Ok(row.map(|r| PrefForm {
        interests: r.0,
        hobbies: r.1,
        gift_types: r.2,
        favorite_snacks: r.3,
        favorite_colors: r.4,
        wishlist: r.5,
        dislikes: r.6,
        food_restrictions: r.7,
        message: r.8,
    }))
}

pub async fn preferences_page(
    State(state): State<AppState>,
    StudentSession(s): StudentSession,
) -> Result<Response, AppError> {
    let first_time = !prefs_completed(&state, s.user.id).await?;
    let v = load_prefs(&state, s.user.id).await?.unwrap_or_default();
    let ctx = Ctx::session("Mes préférences", &s);
    Ok(page(PreferencesTemplate { ctx, v, first_time })?.into_response())
}

#[derive(Deserialize)]
pub struct PrefsInput {
    pub csrf: String,
    #[serde(default)]
    pub interests: String,
    #[serde(default)]
    pub hobbies: String,
    #[serde(default)]
    pub gift_types: String,
    #[serde(default)]
    pub favorite_snacks: String,
    #[serde(default)]
    pub favorite_colors: String,
    #[serde(default)]
    pub wishlist: String,
    #[serde(default)]
    pub dislikes: String,
    #[serde(default)]
    pub food_restrictions: String,
    #[serde(default)]
    pub message: String,
}

pub async fn preferences_save(
    State(state): State<AppState>,
    StudentSession(s): StudentSession,
    Form(f): Form<PrefsInput>,
) -> Result<Response, AppError> {
    s.check_csrf(&f.csrf)?;
    let v = PrefForm {
        interests: clean(&f.interests),
        hobbies: clean(&f.hobbies),
        gift_types: clean(&f.gift_types),
        favorite_snacks: clean(&f.favorite_snacks),
        favorite_colors: clean(&f.favorite_colors),
        wishlist: clean(&f.wishlist),
        dislikes: clean(&f.dislikes),
        food_restrictions: clean(&f.food_restrictions),
        message: clean(&f.message),
    };

    let mut missing = Vec::new();
    for (label, val) in [
        ("tes centres d'intérêt", &v.interests),
        ("tes loisirs", &v.hobbies),
        ("les types de cadeaux que tu préfères", &v.gift_types),
        ("tes couleurs préférées", &v.favorite_colors),
    ] {
        if val.is_empty() {
            missing.push(label);
        }
    }
    let too_long = [
        &v.interests,
        &v.hobbies,
        &v.gift_types,
        &v.favorite_snacks,
        &v.favorite_colors,
        &v.wishlist,
        &v.dislikes,
        &v.food_restrictions,
    ]
    .iter()
    .any(|x| x.chars().count() > 600)
        || v.message.chars().count() > 1000;

    let error = if !missing.is_empty() {
        Some(format!("Il manque encore : {}.", missing.join(", ")))
    } else if too_long {
        Some(
            "Un des champs est trop long (600 caractères maximum, 1000 pour le message)."
                .to_string(),
        )
    } else {
        None
    };
    if let Some(e) = error {
        let first_time = !prefs_completed(&state, s.user.id).await?;
        let ctx = Ctx::session("Mes préférences", &s).error(e);
        let html = page(PreferencesTemplate { ctx, v, first_time })?;
        return Ok((axum::http::StatusCode::UNPROCESSABLE_ENTITY, html).into_response());
    }

    sqlx::query(
        "INSERT INTO preferences (user_id, interests, hobbies, gift_types, favorite_snacks, favorite_colors,
                                  wishlist, dislikes, food_restrictions, message, completed_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
         ON CONFLICT(user_id) DO UPDATE SET
            interests = excluded.interests, hobbies = excluded.hobbies, gift_types = excluded.gift_types,
            favorite_snacks = excluded.favorite_snacks, favorite_colors = excluded.favorite_colors,
            wishlist = excluded.wishlist, dislikes = excluded.dislikes,
            food_restrictions = excluded.food_restrictions, message = excluded.message,
            completed_at = COALESCE(preferences.completed_at, excluded.completed_at),
            updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
    )
    .bind(s.user.id)
    .bind(&v.interests)
    .bind(&v.hobbies)
    .bind(&v.gift_types)
    .bind(&v.favorite_snacks)
    .bind(&v.favorite_colors)
    .bind(&v.wishlist)
    .bind(&v.dislikes)
    .bind(&v.food_restrictions)
    .bind(&v.message)
    .execute(&state.db)
    .await?;
    Ok(Redirect::to("/preferences/saved").into_response())
}

pub async fn preferences_saved(
    State(state): State<AppState>,
    StudentSession(s): StudentSession,
) -> Result<Response, AppError> {
    if !prefs_completed(&state, s.user.id).await? {
        return Ok(Redirect::to("/preferences").into_response());
    }
    let ctx = Ctx::session("Préférences enregistrées", &s);
    Ok(page(SavedTemplate {
        ctx,
        first_name: s.user.first_name.clone(),
    })?
    .into_response())
}

/// Première révélation : enregistre `revealed_at` (une seule fois) puis redirige vers la fiche.
/// L'élève ne fournit aucun identifiant : le destinataire est toujours déduit de sa session.
pub async fn reveal(
    State(state): State<AppState>,
    StudentSession(s): StudentSession,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    s.check_csrf(&f.csrf)?;
    let settings = load_settings(&state.db).await?;
    if settings.draw_completed_at.is_none() || !settings.reveal_enabled {
        return Err(AppError::Forbidden);
    }
    let updated = sqlx::query(
        "UPDATE assignments SET revealed_at = COALESCE(revealed_at, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
         WHERE giver_id = ?",
    )
    .bind(s.user.id)
    .execute(&state.db)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Redirect::to("/recipient?anim=1").into_response())
}

#[derive(Deserialize)]
pub struct RecipientQuery {
    pub anim: Option<String>,
}

pub async fn recipient(
    State(state): State<AppState>,
    StudentSession(s): StudentSession,
    Query(q): Query<RecipientQuery>,
) -> Result<Response, AppError> {
    let settings = load_settings(&state.db).await?;
    if settings.draw_completed_at.is_none() || !settings.reveal_enabled {
        return Ok(Redirect::to("/").into_response());
    }
    let view: Option<RecipientView> = sqlx::query_as(
        "SELECT u.first_name, u.last_name,
                COALESCE(p.interests, '') AS interests, COALESCE(p.hobbies, '') AS hobbies,
                COALESCE(p.gift_types, '') AS gift_types, COALESCE(p.favorite_snacks, '') AS favorite_snacks,
                COALESCE(p.favorite_colors, '') AS favorite_colors, COALESCE(p.wishlist, '') AS wishlist,
                COALESCE(p.dislikes, '') AS dislikes, COALESCE(p.food_restrictions, '') AS food_restrictions,
                COALESCE(p.message, '') AS message
         FROM assignments a
         JOIN users u ON u.id = a.receiver_id
         LEFT JOIN preferences p ON p.user_id = u.id
         WHERE a.giver_id = ? AND a.revealed_at IS NOT NULL",
    )
    .bind(s.user.id)
    .fetch_optional(&state.db)
    .await?;
    let Some(r) = view else {
        return Ok(Redirect::to("/").into_response());
    };
    let tpl = RecipientTemplate {
        ctx: Ctx::session("Ma personne", &s),
        r,
        budget: settings.budget,
        anim: q.anim.as_deref() == Some("1"),
    };
    let html: Html<String> = page(tpl)?;
    Ok(html.into_response())
}
