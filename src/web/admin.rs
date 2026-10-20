use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use serde::Deserialize;
use sqlx::SqliteConnection;

use crate::auth::{AdminSession, LOGIN_MAX_PER_USER, LOGIN_WINDOW};
use crate::draw::{perform_draw, DrawError};
use crate::error::AppError;
use crate::templates::*;
use crate::util::{clean, fr_datetime, slugify, verify_password_async};
use crate::web::{audit, load_settings, page, CsrfForm};
use crate::AppState;

#[derive(Deserialize)]
pub struct FlashQuery {
    pub ok: Option<String>,
}

fn flash_text(code: Option<&str>) -> Option<String> {
    let t = match code? {
        "added" => "Élève ajouté.",
        "updated" => "Modifications enregistrées.",
        "deleted" => "Élève supprimé.",
        "settings" => "Paramètres enregistrés.",
        "draw" => {
            "Le tirage au sort a été effectué et enregistré. Les attributions restent secrètes."
        }
        "reveal_on" => "Les élèves peuvent maintenant découvrir leur personne.",
        "reveal_off" => "Les révélations sont suspendues. Les attributions n'ont pas changé.",
        _ => return None,
    };
    Some(t.to_string())
}

// ---------------------------------------------------------------- connexion

/// Une seule page de connexion pour tout le monde.
pub async fn login_redirect() -> Redirect {
    Redirect::to("/login")
}

// ---------------------------------------------------------------- tableau de bord

pub async fn dashboard(
    State(state): State<AppState>,
    AdminSession(s): AdminSession,
) -> Result<Response, AppError> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role = 'student'")
        .fetch_one(&state.db)
        .await?;
    let done: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM users u JOIN preferences p ON p.user_id = u.id
         WHERE u.role = 'student' AND p.completed_at IS NOT NULL",
    )
    .fetch_one(&state.db)
    .await?;
    let revealed_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM assignments WHERE revealed_at IS NOT NULL")
            .fetch_one(&state.db)
            .await?;
    let settings = load_settings(&state.db).await?;
    let pct = if total > 0 { done * 100 / total } else { 0 };
    let tpl = AdminDashboardTemplate {
        ctx: Ctx::session("Tableau de bord", &s),
        total,
        done,
        pending: total - done,
        pct,
        draw_done: settings.draw_completed_at.is_some(),
        reveal_enabled: settings.reveal_enabled,
        revealed_count,
    };
    Ok(page(tpl)?.into_response())
}

// ---------------------------------------------------------------- élèves

async fn student_rows(state: &AppState) -> Result<Vec<StudentRow>, AppError> {
    let rows: Vec<(i64, String, String, String, bool, bool)> = sqlx::query_as(
        "SELECT u.id, u.username, u.first_name, u.last_name,
                EXISTS(SELECT 1 FROM preferences p WHERE p.user_id = u.id AND p.completed_at IS NOT NULL),
                EXISTS(SELECT 1 FROM assignments a WHERE a.giver_id = u.id AND a.revealed_at IS NOT NULL)
         FROM users u WHERE u.role = 'student'
         ORDER BY lower(u.last_name), lower(u.first_name)",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, username, first_name, last_name, completed, revealed)| StudentRow {
                id,
                username,
                first_name,
                last_name,
                completed,
                revealed,
            },
        )
        .collect())
}

async fn render_students(
    state: &AppState,
    s: &crate::auth::Session,
    flash: Option<String>,
    error: Option<String>,
    status: StatusCode,
) -> Result<Response, AppError> {
    let settings = load_settings(&state.db).await?;
    let mut ctx = Ctx::session("Élèves", s).flash(flash);
    ctx.error = error;
    let tpl = AdminStudentsTemplate {
        ctx,
        students: student_rows(state).await?,
        draw_done: settings.draw_completed_at.is_some(),
    };
    Ok((status, page(tpl)?).into_response())
}

pub async fn students(
    State(state): State<AppState>,
    AdminSession(s): AdminSession,
    Query(q): Query<FlashQuery>,
) -> Result<Response, AppError> {
    render_students(
        &state,
        &s,
        flash_text(q.ok.as_deref()),
        None,
        StatusCode::OK,
    )
    .await
}

async fn draw_done(state: &AppState) -> Result<bool, AppError> {
    Ok(load_settings(&state.db).await?.draw_completed_at.is_some())
}

/// Génère un identifiant libre à partir du prénom et du nom (`prenom.nom`, `prenom.nom2`, …).
async fn unique_username(
    conn: &mut SqliteConnection,
    first: &str,
    last: &str,
) -> Result<String, sqlx::Error> {
    let (f, l) = (slugify(first), slugify(last));
    let mut base = match (f.is_empty(), l.is_empty()) {
        (false, false) => format!("{f}.{l}"),
        (false, true) => f,
        (true, false) => l,
        (true, true) => "eleve".to_string(),
    };
    base.truncate(40);
    let mut candidate = base.clone();
    let mut n = 1;
    loop {
        let taken: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE username = ?)")
                .bind(&candidate)
                .fetch_one(&mut *conn)
                .await?;
        if !taken {
            return Ok(candidate);
        }
        n += 1;
        candidate = format!("{base}{n}");
    }
}

fn valid_name(n: &str) -> bool {
    let c = n.chars().count();
    (1..=60).contains(&c)
}

fn valid_username(u: &str) -> bool {
    (2..=40).contains(&u.len())
        && u.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

#[derive(Deserialize)]
pub struct AddStudentForm {
    pub csrf: String,
    pub first_name: String,
    pub last_name: String,
    #[serde(default)]
    pub username: String,
}

pub async fn student_add(
    State(state): State<AppState>,
    AdminSession(s): AdminSession,
    Form(f): Form<AddStudentForm>,
) -> Result<Response, AppError> {
    s.check_csrf(&f.csrf)?;
    let (first, last) = (clean(&f.first_name), clean(&f.last_name));
    let wanted = f.username.trim().to_lowercase();
    let err = |m: &str| render_students_err(&state, &s, m.to_string());
    if draw_done(&state).await? {
        return err("Le tirage est effectué : on ne peut plus ajouter d'élève.").await;
    }
    if !valid_name(&first) || !valid_name(&last) {
        return err("Le prénom et le nom sont obligatoires (60 caractères maximum).").await;
    }
    if !wanted.is_empty() && !valid_username(&wanted) {
        return err("L'identifiant doit faire 2 à 40 caractères : lettres minuscules, chiffres, « . », « _ » ou « - ».").await;
    }

    let mut tx = state.db.begin().await?;
    let username = if wanted.is_empty() {
        unique_username(&mut tx, &first, &last).await?
    } else {
        let taken: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE username = ?)")
                .bind(&wanted)
                .fetch_one(&mut *tx)
                .await?;
        if taken {
            drop(tx);
            return err("Cet identifiant est déjà utilisé.").await;
        }
        wanted
    };
    sqlx::query("INSERT INTO users (username, first_name, last_name, password_hash, role) VALUES (?, ?, ?, '', 'student')")
        .bind(&username)
        .bind(&first)
        .bind(&last)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    audit(&state.db, &s, "student_add", &username).await?;

    let tpl = CredentialsTemplate {
        ctx: Ctx::session("Identifiants", &s),
        heading: "Élève ajouté".into(),
        creds: vec![Cred {
            name: format!("{first} {last}"),
            username,
        }],
        problems: vec![],
    };
    Ok(page(tpl)?.into_response())
}

async fn render_students_err(
    state: &AppState,
    s: &crate::auth::Session,
    msg: String,
) -> Result<Response, AppError> {
    render_students(state, s, None, Some(msg), StatusCode::UNPROCESSABLE_ENTITY).await
}

#[derive(Deserialize)]
pub struct ImportForm {
    pub csrf: String,
    pub csv: String,
}

/// Lit `prénom,nom[,identifiant]` (séparateur `,` ou `;`, en-tête facultatif).
fn parse_csv(text: &str) -> (Vec<(String, String, String)>, Vec<String>) {
    let text = text.trim_start_matches('\u{feff}');
    let delim = if text.matches(';').count() > text.matches(',').count() {
        b';'
    } else {
        b','
    };
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delim)
        .trim(csv::Trim::All)
        .from_reader(text.as_bytes());
    let mut rows = Vec::new();
    let mut problems = Vec::new();
    for (i, rec) in rdr.records().enumerate() {
        let line = i + 1;
        let Ok(rec) = rec else {
            problems.push(format!("Ligne {line} : format illisible."));
            continue;
        };
        let cells: Vec<String> = rec.iter().map(clean).collect();
        if cells.iter().all(|c| c.is_empty()) {
            continue;
        }
        if i == 0 {
            let h = cells[0].to_lowercase();
            if matches!(
                h.as_str(),
                "prenom" | "prénom" | "first_name" | "firstname" | "first name"
            ) {
                continue;
            }
        }
        if cells.len() < 2 || !valid_name(&cells[0]) || !valid_name(&cells[1]) {
            problems.push(format!(
                "Ligne {line} : prénom et nom requis (60 caractères maximum)."
            ));
            continue;
        }
        let username = cells.get(2).map(|u| u.to_lowercase()).unwrap_or_default();
        if !username.is_empty() && !valid_username(&username) {
            problems.push(format!("Ligne {line} : identifiant invalide « {username} » (ignoré, un identifiant sera généré)."));
            rows.push((cells[0].clone(), cells[1].clone(), String::new()));
            continue;
        }
        rows.push((cells[0].clone(), cells[1].clone(), username));
    }
    (rows, problems)
}

pub async fn students_import(
    State(state): State<AppState>,
    AdminSession(s): AdminSession,
    Form(f): Form<ImportForm>,
) -> Result<Response, AppError> {
    s.check_csrf(&f.csrf)?;
    if draw_done(&state).await? {
        return render_students_err(
            &state,
            &s,
            "Le tirage est effectué : on ne peut plus importer d'élèves.".into(),
        )
        .await;
    }
    let (rows, mut problems) = parse_csv(&f.csv);
    if rows.is_empty() {
        return render_students_err(
            &state,
            &s,
            "Aucun élève valide trouvé dans le CSV (format : prénom,nom).".into(),
        )
        .await;
    }
    if rows.len() > 300 {
        return render_students_err(&state, &s, "Import limité à 300 élèves à la fois.".into())
            .await;
    }

    let mut creds = Vec::new();
    let mut tx = state.db.begin().await?;
    for (first, last, wanted) in &rows {
        let username = if wanted.is_empty() {
            unique_username(&mut tx, first, last).await?
        } else {
            let taken: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE username = ?)")
                    .bind(wanted)
                    .fetch_one(&mut *tx)
                    .await?;
            if taken {
                problems.push(format!(
                    "{first} {last} : identifiant « {wanted} » déjà pris, un autre a été généré."
                ));
                unique_username(&mut tx, first, last).await?
            } else {
                wanted.clone()
            }
        };
        sqlx::query("INSERT INTO users (username, first_name, last_name, password_hash, role) VALUES (?, ?, ?, '', 'student')")
            .bind(&username)
            .bind(first)
            .bind(last)
            .execute(&mut *tx)
            .await?;
        creds.push(Cred {
            name: format!("{first} {last}"),
            username,
        });
    }
    tx.commit().await?;
    audit(
        &state.db,
        &s,
        "student_import",
        &format!("{} élèves importés", creds.len()),
    )
    .await?;

    let tpl = CredentialsTemplate {
        ctx: Ctx::session("Identifiants", &s),
        heading: format!("{} élèves importés", creds.len()),
        creds,
        problems,
    };
    Ok(page(tpl)?.into_response())
}

async fn find_student(state: &AppState, id: i64) -> Result<StudentRow, AppError> {
    student_rows(state)
        .await?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or(AppError::NotFound)
}

pub async fn student_edit_page(
    State(state): State<AppState>,
    AdminSession(s): AdminSession,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let row = find_student(&state, id).await?;
    Ok(page(AdminStudentEditTemplate {
        ctx: Ctx::session("Modifier un élève", &s),
        s: row,
    })?
    .into_response())
}

#[derive(Deserialize)]
pub struct EditStudentForm {
    pub csrf: String,
    pub first_name: String,
    pub last_name: String,
    pub username: String,
}

pub async fn student_edit(
    State(state): State<AppState>,
    AdminSession(s): AdminSession,
    Path(id): Path<i64>,
    Form(f): Form<EditStudentForm>,
) -> Result<Response, AppError> {
    s.check_csrf(&f.csrf)?;
    let current = find_student(&state, id).await?;
    let (first, last, username) = (
        clean(&f.first_name),
        clean(&f.last_name),
        f.username.trim().to_lowercase(),
    );
    let problem = if !valid_name(&first) || !valid_name(&last) {
        Some("Le prénom et le nom sont obligatoires (60 caractères maximum).")
    } else if !valid_username(&username) {
        Some(
            "Identifiant invalide : 2 à 40 caractères (minuscules, chiffres, « . », « _ », « - »).",
        )
    } else {
        None
    };
    let taken: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE username = ? AND id <> ?)")
            .bind(&username)
            .bind(id)
            .fetch_one(&state.db)
            .await?;
    let problem = problem.or(if taken {
        Some("Cet identifiant est déjà utilisé.")
    } else {
        None
    });
    if let Some(p) = problem {
        let ctx = Ctx::session("Modifier un élève", &s).error(p);
        let html = page(AdminStudentEditTemplate { ctx, s: current })?;
        return Ok((StatusCode::UNPROCESSABLE_ENTITY, html).into_response());
    }
    sqlx::query(
        "UPDATE users SET first_name = ?, last_name = ?, username = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ','now')
         WHERE id = ? AND role = 'student'",
    )
    .bind(&first)
    .bind(&last)
    .bind(&username)
    .bind(id)
    .execute(&state.db)
    .await?;
    audit(
        &state.db,
        &s,
        "student_edit",
        &format!("#{id} → {username}"),
    )
    .await?;
    Ok(Redirect::to("/admin/students?ok=updated").into_response())
}

pub async fn student_reset(
    State(state): State<AppState>,
    AdminSession(s): AdminSession,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    s.check_csrf(&f.csrf)?;
    let student = find_student(&state, id).await?;
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE users SET password_hash = '', updated_at = strftime('%Y-%m-%dT%H:%M:%SZ','now') WHERE id = ? AND role = 'student'")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE user_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    audit(&state.db, &s, "student_reset_password", &student.username).await?;
    let tpl = CredentialsTemplate {
        ctx: Ctx::session("Mot de passe réinitialisé", &s),
        heading: "Mot de passe réinitialisé".into(),
        creds: vec![Cred {
            name: format!("{} {}", student.first_name, student.last_name),
            username: student.username,
        }],
        problems: vec![],
    };
    Ok(page(tpl)?.into_response())
}

pub async fn student_delete(
    State(state): State<AppState>,
    AdminSession(s): AdminSession,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    s.check_csrf(&f.csrf)?;
    if draw_done(&state).await? {
        return render_students_err(
            &state,
            &s,
            "Le tirage est effectué : on ne peut plus supprimer d'élève.".into(),
        )
        .await;
    }
    let student = find_student(&state, id).await?;
    sqlx::query("DELETE FROM users WHERE id = ? AND role = 'student'")
        .bind(id)
        .execute(&state.db)
        .await?;
    audit(&state.db, &s, "student_delete", &student.username).await?;
    Ok(Redirect::to("/admin/students?ok=deleted").into_response())
}

// ---------------------------------------------------------------- paramètres

pub async fn settings_page(
    State(state): State<AppState>,
    AdminSession(s): AdminSession,
    Query(q): Query<FlashQuery>,
) -> Result<Response, AppError> {
    let ctx = Ctx::session("Paramètres", &s).flash(flash_text(q.ok.as_deref()));
    Ok(page(SettingsTemplate {
        ctx,
        s: load_settings(&state.db).await?,
    })?
    .into_response())
}

#[derive(Deserialize)]
pub struct SettingsForm {
    pub csrf: String,
    pub class_name: String,
    pub event_name: String,
    pub budget: String,
    #[serde(default)]
    pub exchange_date: String,
    #[serde(default)]
    pub welcome_message: String,
}

pub async fn settings_save(
    State(state): State<AppState>,
    AdminSession(s): AdminSession,
    Form(f): Form<SettingsForm>,
) -> Result<Response, AppError> {
    s.check_csrf(&f.csrf)?;
    let mut cur = load_settings(&state.db).await?;
    cur.class_name = clean(&f.class_name);
    cur.event_name = clean(&f.event_name);
    cur.budget = clean(&f.budget);
    cur.exchange_date = clean(&f.exchange_date);
    cur.welcome_message = clean(&f.welcome_message);
    let ok = (1..=80).contains(&cur.class_name.chars().count())
        && (1..=80).contains(&cur.event_name.chars().count())
        && cur.budget.chars().count() <= 40
        && cur.exchange_date.chars().count() <= 40
        && cur.welcome_message.chars().count() <= 300;
    if !ok {
        let ctx = Ctx::session("Paramètres", &s).error("Certains champs sont vides ou trop longs.");
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            page(SettingsTemplate { ctx, s: cur })?,
        )
            .into_response());
    }
    sqlx::query(
        "UPDATE event_settings SET class_name = ?, event_name = ?, budget = ?, exchange_date = ?, welcome_message = ? WHERE id = 1",
    )
    .bind(&cur.class_name)
    .bind(&cur.event_name)
    .bind(&cur.budget)
    .bind(&cur.exchange_date)
    .bind(&cur.welcome_message)
    .execute(&state.db)
    .await?;
    audit(&state.db, &s, "settings", "Paramètres modifiés").await?;
    Ok(Redirect::to("/admin/settings?ok=settings").into_response())
}

// ---------------------------------------------------------------- tirage

async fn render_draw(
    state: &AppState,
    s: &crate::auth::Session,
    flash: Option<String>,
    error: Option<String>,
    status: StatusCode,
) -> Result<Response, AppError> {
    let settings = load_settings(&state.db).await?;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role = 'student'")
        .fetch_one(&state.db)
        .await?;
    let pending: Vec<String> = sqlx::query_scalar(
        "SELECT u.first_name || ' ' || u.last_name FROM users u
         WHERE u.role = 'student' AND NOT EXISTS
               (SELECT 1 FROM preferences p WHERE p.user_id = u.id AND p.completed_at IS NOT NULL)
         ORDER BY lower(u.last_name), lower(u.first_name)",
    )
    .fetch_all(&state.db)
    .await?;
    let draw_done = settings.draw_completed_at.is_some();
    let mut ctx = Ctx::session("Tirage au sort", s).flash(flash);
    ctx.error = error;
    let tpl = DrawTemplate {
        ctx,
        total,
        can_draw: !draw_done && total >= 2 && pending.is_empty(),
        pending,
        draw_done,
        drawn_at: settings
            .draw_completed_at
            .as_deref()
            .map(fr_datetime)
            .unwrap_or_default(),
        reveal_enabled: settings.reveal_enabled,
    };
    Ok((status, page(tpl)?).into_response())
}

pub async fn draw_page(
    State(state): State<AppState>,
    AdminSession(s): AdminSession,
    Query(q): Query<FlashQuery>,
) -> Result<Response, AppError> {
    render_draw(
        &state,
        &s,
        flash_text(q.ok.as_deref()),
        None,
        StatusCode::OK,
    )
    .await
}

pub async fn draw_run(
    State(state): State<AppState>,
    AdminSession(s): AdminSession,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    s.check_csrf(&f.csrf)?;
    match perform_draw(&state.db, s.user.id, &s.user.username).await {
        Ok(_) => Ok(Redirect::to("/admin/draw?ok=draw").into_response()),
        Err(DrawError::Db(e)) => Err(AppError::Db(e)),
        Err(e) => render_draw(&state, &s, None, Some(e.message()), StatusCode::CONFLICT).await,
    }
}

#[derive(Deserialize)]
pub struct RevealForm {
    pub csrf: String,
    #[serde(default)]
    pub enabled: String,
}

pub async fn reveal_toggle(
    State(state): State<AppState>,
    AdminSession(s): AdminSession,
    Form(f): Form<RevealForm>,
) -> Result<Response, AppError> {
    s.check_csrf(&f.csrf)?;
    let enable = f.enabled == "1";
    let r = sqlx::query("UPDATE event_settings SET reveal_enabled = ? WHERE id = 1 AND draw_completed_at IS NOT NULL")
        .bind(enable)
        .execute(&state.db)
        .await?;
    if r.rows_affected() == 0 {
        return render_draw(
            &state,
            &s,
            None,
            Some("Le tirage n'a pas encore été effectué.".into()),
            StatusCode::CONFLICT,
        )
        .await;
    }
    audit(
        &state.db,
        &s,
        if enable { "reveal_on" } else { "reveal_off" },
        "",
    )
    .await?;
    Ok(Redirect::to(if enable {
        "/admin/draw?ok=reveal_on"
    } else {
        "/admin/draw?ok=reveal_off"
    })
    .into_response())
}

// ---------------------------------------------------------------- attributions (action protégée)

pub async fn assignments_page(
    State(_): State<AppState>,
    AdminSession(s): AdminSession,
) -> Result<Response, AppError> {
    Ok(page(AssignmentsTemplate {
        ctx: Ctx::session("Consulter les attributions", &s),
        rows: None,
    })?
    .into_response())
}

#[derive(Deserialize)]
pub struct ConfirmPasswordForm {
    pub csrf: String,
    pub password: String,
}

pub async fn assignments_show(
    State(state): State<AppState>,
    AdminSession(s): AdminSession,
    Form(f): Form<ConfirmPasswordForm>,
) -> Result<Response, AppError> {
    s.check_csrf(&f.csrf)?;
    let key = format!("assign:{}", s.user.id);
    let fail = |msg: &str| {
        let ctx = Ctx::session("Consulter les attributions", &s).error(msg);
        page(AssignmentsTemplate { ctx, rows: None })
            .map(|h| (StatusCode::FORBIDDEN, h).into_response())
    };
    if state
        .limiter
        .is_blocked(&key, LOGIN_MAX_PER_USER, LOGIN_WINDOW)
    {
        return fail("Trop de tentatives. Patiente quelques minutes.");
    }
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
        .bind(s.user.id)
        .fetch_one(&state.db)
        .await?;
    if !verify_password_async(hash, f.password).await {
        state.limiter.record(&key);
        audit(
            &state.db,
            &s,
            "view_assignments_denied",
            "mot de passe incorrect",
        )
        .await?;
        return fail("Mot de passe incorrect.");
    }
    state.limiter.clear(&key);
    audit(
        &state.db,
        &s,
        "view_assignments",
        "Consultation de toutes les attributions",
    )
    .await?;

    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT g.first_name || ' ' || g.last_name, r.first_name || ' ' || r.last_name, a.revealed_at
         FROM assignments a JOIN users g ON g.id = a.giver_id JOIN users r ON r.id = a.receiver_id
         ORDER BY lower(g.last_name), lower(g.first_name)",
    )
    .fetch_all(&state.db)
    .await?;
    let rows = rows
        .into_iter()
        .map(|(giver, receiver, rev)| AssignRow {
            giver,
            receiver,
            revealed: rev
                .as_deref()
                .map(fr_datetime)
                .unwrap_or_else(|| "—".into()),
        })
        .collect();
    Ok(page(AssignmentsTemplate {
        ctx: Ctx::session("Attributions", &s),
        rows: Some(rows),
    })?
    .into_response())
}

// ---------------------------------------------------------------- journal

pub async fn audit_log_page(
    State(state): State<AppState>,
    AdminSession(s): AdminSession,
) -> Result<Response, AppError> {
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT created_at, actor_name, action, details FROM audit_log ORDER BY id DESC LIMIT 200",
    )
    .fetch_all(&state.db)
    .await?;
    let rows = rows
        .into_iter()
        .map(|(when, actor, action, details)| AuditRow {
            when: fr_datetime(&when),
            actor,
            action,
            details,
        })
        .collect();
    Ok(page(AuditTemplate {
        ctx: Ctx::session("Journal", &s),
        rows,
    })?
    .into_response())
}
