use std::collections::{HashMap, HashSet};

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt;

use secret_santa::config::Config;
use secret_santa::draw::{perform_draw, DrawError};
use secret_santa::util::{hash_password, random_token};
use secret_santa::{router, setup, AppState};

const PW: &str = "abcd-efgh";
const ADMIN_PW: &str = "admin-pass-123";

// ------------------------------------------------------------------ outils

struct Resp {
    status: StatusCode,
    location: Option<String>,
    body: String,
    headers: axum::http::HeaderMap,
}

struct Client {
    app: Router,
    jar: HashMap<String, String>,
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

impl Client {
    fn new(app: Router) -> Self {
        Self {
            app,
            jar: HashMap::new(),
        }
    }

    async fn send(&mut self, mut req: Request<Body>) -> Resp {
        if !self.jar.is_empty() {
            let c = self
                .jar
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; ");
            req.headers_mut().insert(header::COOKIE, c.parse().unwrap());
        }
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        for sc in headers.get_all(header::SET_COOKIE) {
            let s = sc.to_str().unwrap();
            let (pair, _) = s.split_once(';').unwrap_or((s, ""));
            let (k, v) = pair.split_once('=').unwrap();
            if v.is_empty() {
                self.jar.remove(k);
            } else {
                self.jar.insert(k.to_string(), v.to_string());
            }
        }
        let location = headers
            .get(header::LOCATION)
            .map(|v| v.to_str().unwrap().to_string());
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        Resp {
            status,
            location,
            body: String::from_utf8_lossy(&bytes).into_owned(),
            headers,
        }
    }

    async fn get(&mut self, path: &str) -> Resp {
        self.send(Request::get(path).body(Body::empty()).unwrap())
            .await
    }

    async fn post(&mut self, path: &str, form: &[(&str, &str)]) -> Resp {
        let body = form
            .iter()
            .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
            .collect::<Vec<_>>()
            .join("&");
        self.send(
            Request::post(path)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
    }
}

fn csrf(body: &str) -> String {
    let key = "name=\"csrf\" value=\"";
    let i = body.find(key).expect("jeton csrf absent") + key.len();
    body[i..i + 64].to_string()
}

/// Chaque test travaille dans sa propre base PostgreSQL, créée à la volée.
/// Serveur de test : variable `TEST_DATABASE_URL` (utilisateur autorisé à créer des bases).
async fn test_app() -> (Router, AppState) {
    let base = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/postgres".into());
    let (prefix, _) = base.rsplit_once('/').expect("TEST_DATABASE_URL invalide");
    let name = format!("santa_test_{}", &random_token()[..16]);
    let admin_pool = PgPool::connect(&base)
        .await
        .expect("serveur PostgreSQL de test");
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&admin_pool)
        .await
        .unwrap();
    admin_pool.close().await;
    let url = format!("{prefix}/{name}");
    let cfg = Config {
        database_url: url,
        bind: String::new(),
        cookie_secure: false,
        trust_proxy: false,
        admin_username: "admin".into(),
        admin_password: Some(ADMIN_PW.into()),
        session_days: 30,
    };
    let state = setup(cfg).await.unwrap();
    (router(state.clone()), state)
}

/// Crée `n` élèves (mot de passe `PW`), avec ou sans questionnaire. Retourne (id, identifiant).
async fn add_students(state: &AppState, n: usize, with_prefs: bool) -> Vec<(i64, String)> {
    let hash = hash_password(PW);
    let mut out = Vec::new();
    for i in 1..=n {
        let username = format!("eleve{i}");
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, first_name, last_name, password_hash, role) VALUES ($1, $2, $3, $4, 'student') RETURNING id",
        )
        .bind(&username)
        .bind(format!("Prenom{i}"))
        .bind(format!("Nom{i}"))
        .bind(&hash)
        .fetch_one(&state.db)
        .await
        .unwrap();
        if with_prefs {
            set_prefs(state, id).await;
        }
        out.push((id, username));
    }
    out
}

async fn set_prefs(state: &AppState, user_id: i64) {
    sqlx::query(
        "INSERT INTO preferences (user_id, interests, hobbies, gift_types, favorite_colors, completed_at)
         VALUES ($1, $2, 'hobby', 'type', 'bleu', iso_now())",
    )
    .bind(user_id)
    .bind(format!("interet-secret-{user_id}"))
    .execute(&state.db)
    .await
    .unwrap();
}

async fn login(app: &Router, username: &str, pw: &str, admin: bool) -> (Client, Resp) {
    let mut c = Client::new(app.clone());
    let _ = admin;
    let path = "/login";
    let page = c.get(path).await;
    let token = csrf(&page.body);
    let r = c
        .post(
            path,
            &[("csrf", &token), ("username", username), ("password", pw)],
        )
        .await;
    (c, r)
}

async fn student(app: &Router, username: &str) -> Client {
    let (c, r) = login(app, username, PW, false).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "connexion de {username}");
    c
}

async fn admin(app: &Router) -> Client {
    let (c, r) = login(app, "admin", ADMIN_PW, true).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.location.as_deref(), Some("/admin"));
    c
}

async fn admin_id(state: &AppState) -> i64 {
    sqlx::query_scalar("SELECT id FROM users WHERE role = 'admin'")
        .fetch_one(&state.db)
        .await
        .unwrap()
}

async fn assignments(state: &AppState) -> Vec<(i64, i64)> {
    sqlx::query_as("SELECT giver_id, receiver_id FROM assignments ORDER BY giver_id")
        .fetch_all(&state.db)
        .await
        .unwrap()
}

async fn set_reveal(app: &Router, enabled: bool) {
    let mut a = admin(app).await;
    let page = a.get("/admin/draw").await;
    let t = csrf(&page.body);
    let r = a
        .post(
            "/admin/draw/reveal",
            &[("csrf", &t), ("enabled", if enabled { "1" } else { "0" })],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
}

// ------------------------------------------------------------------ authentification

#[tokio::test]
async fn student_login_works_and_wrong_credentials_fail() {
    let (app, state) = test_app().await;
    add_students(&state, 2, true).await;

    let (_, bad) = login(&app, "eleve1", "mauvais-code", false).await;
    assert_eq!(bad.status, StatusCode::UNAUTHORIZED);
    assert!(bad.body.contains("Identifiant ou mot de passe incorrect"));

    let (_, unknown) = login(&app, "inconnu", PW, false).await;
    assert_eq!(unknown.status, StatusCode::UNAUTHORIZED);

    // connaître le nom d'un élève ne suffit pas, et le mot de passe d'un autre non plus
    let (_, other) = login(&app, "eleve1", PW.replace("abcd", "zzzz").as_str(), false).await;
    assert_eq!(other.status, StatusCode::UNAUTHORIZED);

    let (mut c, ok) = login(&app, "eleve1", PW, false).await;
    assert_eq!(ok.status, StatusCode::SEE_OTHER);
    assert_eq!(ok.location.as_deref(), Some("/"));
    let home = c.get("/").await;
    assert_eq!(home.status, StatusCode::OK);
    assert!(home.body.contains("Bonjour Prenom1"));

    // le cookie de session est HttpOnly + SameSite
    let (_, ok2) = login(&app, "eleve2", PW, false).await;
    let cookie = ok2
        .headers
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .find(|s| s.starts_with("sid="))
        .unwrap();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Lax"));
}

#[tokio::test]
async fn logout_destroys_session() {
    let (app, state) = test_app().await;
    add_students(&state, 2, true).await;
    let mut c = student(&app, "eleve1").await;
    let home = c.get("/").await;
    let t = csrf(&home.body);
    let r = c.post("/logout", &[("csrf", &t)]).await;
    assert_eq!(r.location.as_deref(), Some("/login"));
    let again = c.get("/").await;
    assert_eq!(again.location.as_deref(), Some("/login"));
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn login_is_rate_limited() {
    let (app, state) = test_app().await;
    add_students(&state, 2, true).await;
    for _ in 0..5 {
        let (_, r) = login(&app, "eleve1", "faux", false).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    }
    let (_, r) = login(&app, "eleve1", PW, false).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    // un autre compte n'est pas affecté
    let (_, r2) = login(&app, "eleve2", PW, false).await;
    assert_eq!(r2.status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn login_requires_valid_csrf() {
    let (app, state) = test_app().await;
    add_students(&state, 1, true).await;
    let mut c = Client::new(app.clone());
    c.get("/login").await;
    let r = c
        .post(
            "/login",
            &[("csrf", "faux"), ("username", "eleve1"), ("password", PW)],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

// ------------------------------------------------------------------ droits

#[tokio::test]
async fn admin_area_is_protected() {
    let (app, state) = test_app().await;
    add_students(&state, 2, true).await;

    let mut anon = Client::new(app.clone());
    for path in [
        "/admin",
        "/admin/students",
        "/admin/draw",
        "/admin/settings",
        "/admin/audit",
        "/admin/assignments",
    ] {
        let r = anon.get(path).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER, "{path}");
        assert_eq!(r.location.as_deref(), Some("/login"));
    }

    let mut s = student(&app, "eleve1").await;
    let home = s.get("/").await;
    let t = csrf(&home.body);
    for path in [
        "/admin",
        "/admin/students",
        "/admin/draw",
        "/admin/settings",
        "/admin/audit",
        "/admin/assignments",
    ] {
        assert_eq!(s.get(path).await.status, StatusCode::FORBIDDEN, "{path}");
    }
    for path in ["/admin/draw", "/admin/draw/reveal", "/admin/students"] {
        let r = s
            .post(
                path,
                &[
                    ("csrf", &t),
                    ("enabled", "1"),
                    ("first_name", "x"),
                    ("last_name", "y"),
                ],
            )
            .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{path}");
    }

    // une seule page de connexion : l'admin y est reconnu et envoyé vers /admin
    let mut a = admin(&app).await;
    assert_eq!(a.get("/admin").await.status, StatusCode::OK);
    // l'admin n'a pas d'espace élève
    assert_eq!(a.get("/").await.location.as_deref(), Some("/admin"));
}

#[tokio::test]
async fn forms_reject_bad_csrf() {
    let (app, state) = test_app().await;
    add_students(&state, 2, false).await;
    let mut s = student(&app, "eleve1").await;
    let r = s
        .post(
            "/preferences",
            &[
                ("csrf", "faux"),
                ("interests", "a"),
                ("hobbies", "b"),
                ("gift_types", "c"),
                ("favorite_colors", "d"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    let mut a = admin(&app).await;
    let r = a.post("/admin/draw", &[("csrf", "faux")]).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = a
        .post(
            "/admin/students",
            &[("csrf", "faux"), ("first_name", "A"), ("last_name", "B")],
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

// ------------------------------------------------------------------ préférences

#[tokio::test]
async fn preferences_are_validated_and_saved() {
    let (app, state) = test_app().await;
    let ids = add_students(&state, 2, false).await;
    let mut s = student(&app, "eleve1").await;

    // première connexion : redirection vers le questionnaire
    let home = s.get("/").await;
    assert_eq!(home.location.as_deref(), Some("/preferences"));
    let form = s.get("/preferences").await;
    assert!(form.body.contains("Parlons un peu de toi"));
    let t = csrf(&form.body);

    let bad = s
        .post("/preferences", &[("csrf", &t), ("interests", "foot")])
        .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(bad.body.contains("Il manque encore"));
    assert!(
        bad.body.contains("foot"),
        "les valeurs saisies sont conservées"
    );

    let ok = s
        .post(
            "/preferences",
            &[
                ("csrf", &t),
                ("interests", "foot, manga"),
                ("hobbies", "dessin"),
                ("gift_types", "livres"),
                ("favorite_colors", "vert"),
                ("favorite_snacks", "chocolat"),
                ("message", "Merci <b>!</b>"),
            ],
        )
        .await;
    assert_eq!(ok.status, StatusCode::SEE_OTHER);
    assert_eq!(ok.location.as_deref(), Some("/preferences/saved"));
    assert!(s
        .get("/preferences/saved")
        .await
        .body
        .contains("C'est enregistré"));

    let (interests, completed): (String, Option<String>) =
        sqlx::query_as("SELECT interests, completed_at FROM preferences WHERE user_id = $1")
            .bind(ids[0].0)
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert_eq!(interests, "foot, manga");
    assert!(completed.is_some());

    // modification possible, le HTML est échappé à l'affichage
    let edit = s.get("/preferences").await;
    assert!(
        edit.body.contains("Merci &#60;b&#62;!&#60;/b&#62;")
            || edit.body.contains("Merci &lt;b&gt;")
    );
    assert!(!edit.body.contains("Merci <b>"));
    assert_eq!(s.get("/").await.status, StatusCode::OK);
}

#[tokio::test]
async fn admin_sees_completion_but_not_preferences() {
    let (app, state) = test_app().await;
    let ids = add_students(&state, 4, false).await;
    set_prefs(&state, ids[0].0).await;
    set_prefs(&state, ids[1].0).await;
    set_prefs(&state, ids[2].0).await;
    let mut a = admin(&app).await;
    let dash = a.get("/admin").await;
    assert!(
        dash.body.contains("75 % des questionnaires remplis"),
        "{}",
        dash.body
    );
    let list = a.get("/admin/students").await;
    assert!(!list.body.contains("interet-secret"));
    assert!(list.body.contains("En attente"));
}

// ------------------------------------------------------------------ tirage

#[tokio::test]
async fn draw_is_blocked_while_a_questionnaire_is_missing() {
    let (app, state) = test_app().await;
    let ids = add_students(&state, 4, true).await;
    sqlx::query("UPDATE preferences SET completed_at = NULL WHERE user_id = $1")
        .bind(ids[3].0)
        .execute(&state.db)
        .await
        .unwrap();

    let err = perform_draw(&state.db, admin_id(&state).await, "admin")
        .await
        .unwrap_err();
    assert!(matches!(err, DrawError::Incomplete(1)));
    // rien n'a été enregistré, et le tirage reste possible plus tard
    assert!(assignments(&state).await.is_empty());
    let drawn: Option<String> =
        sqlx::query_scalar("SELECT draw_completed_at FROM event_settings WHERE id = 1")
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert!(drawn.is_none());

    // côté interface : bouton désactivé + liste des retardataires, et POST refusé
    let mut a = admin(&app).await;
    let page = a.get("/admin/draw").await;
    assert!(page.body.contains("En attente des questionnaires"));
    assert!(page.body.contains("Prenom4 Nom4"));
    assert!(!page.body.contains("data-open=\"draw-dialog\""));
    let t = csrf(&page.body);
    let r = a.post("/admin/draw", &[("csrf", &t)]).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(assignments(&state).await.is_empty());

    sqlx::query("UPDATE preferences SET completed_at = iso_now() WHERE user_id = $1")
        .bind(ids[3].0)
        .execute(&state.db)
        .await
        .unwrap();
    let page = a.get("/admin/draw").await;
    assert!(page.body.contains("data-open=\"draw-dialog\""));
    let t = csrf(&page.body);
    let r = a.post("/admin/draw", &[("csrf", &t)]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(assignments(&state).await.len(), 4);
}

#[tokio::test]
async fn draw_needs_two_participants() {
    let (_app, state) = test_app().await;
    add_students(&state, 1, true).await;
    let err = perform_draw(&state.db, admin_id(&state).await, "admin")
        .await
        .unwrap_err();
    assert!(matches!(err, DrawError::NotEnoughParticipants));
    assert!(assignments(&state).await.is_empty());
}

#[tokio::test]
async fn draw_is_valid_for_2_3_10_and_30_students() {
    for n in [2usize, 3, 10, 30] {
        let (_app, state) = test_app().await;
        let ids = add_students(&state, n, true).await;
        let count = perform_draw(&state.db, admin_id(&state).await, "admin")
            .await
            .unwrap();
        assert_eq!(count, n);

        let rows = assignments(&state).await;
        assert_eq!(rows.len(), n);
        assert!(
            rows.iter().all(|(g, r)| g != r),
            "auto-attribution avec {n} élèves"
        );
        let all: HashSet<i64> = ids.iter().map(|i| i.0).collect();
        assert_eq!(
            rows.iter().map(|r| r.0).collect::<HashSet<_>>(),
            all,
            "chaque élève donne une fois"
        );
        assert_eq!(
            rows.iter().map(|r| r.1).collect::<HashSet<_>>(),
            all,
            "chaque élève reçoit une fois"
        );

        // le tirage ne se refait jamais, et ne change pas
        let again = perform_draw(&state.db, admin_id(&state).await, "admin")
            .await
            .unwrap_err();
        assert!(matches!(again, DrawError::AlreadyDone));
        assert_eq!(assignments(&state).await, rows);
    }
}

#[tokio::test]
async fn two_concurrent_draws_create_exactly_one_draw() {
    let (_app, state) = test_app().await;
    add_students(&state, 12, true).await;
    let admin = admin_id(&state).await;
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let pool = state.db.clone();
        tasks.push(tokio::spawn(async move {
            perform_draw(&pool, admin, "admin").await
        }));
    }
    let mut ok = 0;
    for t in tasks {
        match t.await.unwrap() {
            Ok(_) => ok += 1,
            Err(e) => assert!(matches!(e, DrawError::AlreadyDone), "{e:?}"),
        }
    }
    assert_eq!(ok, 1);
    let rows = assignments(&state).await;
    assert_eq!(rows.len(), 12);
    assert!(rows.iter().all(|(g, r)| g != r));
    let audits: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE action = 'draw'")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(audits, 1);
}

#[tokio::test]
async fn database_rejects_invalid_assignments() {
    let (_app, state) = test_app().await;
    let ids = add_students(&state, 3, true).await;
    let (a, b, c) = (ids[0].0, ids[1].0, ids[2].0);

    // pas d'attribution avant le tirage
    let r = sqlx::query("INSERT INTO assignments (giver_id, receiver_id) VALUES ($1, $2)")
        .bind(a)
        .bind(b)
        .execute(&state.db)
        .await;
    assert!(r.is_err());
    // pas d'autorisation de révélation avant le tirage
    assert!(
        sqlx::query("UPDATE event_settings SET reveal_enabled = TRUE")
            .execute(&state.db)
            .await
            .is_err()
    );

    perform_draw(&state.db, admin_id(&state).await, "admin")
        .await
        .unwrap();
    // auto-attribution, doublons, modification et suppression : tout est refusé
    assert!(
        sqlx::query("INSERT INTO assignments (giver_id, receiver_id) VALUES ($1, $2)")
            .bind(a)
            .bind(a)
            .execute(&state.db)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("INSERT INTO assignments (giver_id, receiver_id) VALUES ($1, $2)")
            .bind(a)
            .bind(c)
            .execute(&state.db)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE assignments SET receiver_id = $1 WHERE giver_id = $2")
            .bind(c)
            .bind(a)
            .execute(&state.db)
            .await
            .is_err()
    );
    assert!(sqlx::query("DELETE FROM assignments")
        .execute(&state.db)
        .await
        .is_err());
    assert!(sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(a)
        .execute(&state.db)
        .await
        .is_err());
    assert_eq!(assignments(&state).await.len(), 3);
}

// ------------------------------------------------------------------ révélation

#[tokio::test]
async fn reveal_is_impossible_until_admin_allows_it() {
    let (app, state) = test_app().await;
    let ids = add_students(&state, 3, true).await;
    perform_draw(&state.db, admin_id(&state).await, "admin")
        .await
        .unwrap();

    let mut s = student(&app, "eleve1").await;
    let home = s.get("/").await;
    assert!(home
        .body
        .contains("Le tirage au sort n'est pas encore disponible"));
    assert!(!home.body.contains("action=\"/reveal\""));
    let t = csrf(&home.body);

    // même en forçant la requête
    let r = s.post("/reveal", &[("csrf", &t)]).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = s.get("/recipient").await;
    assert_eq!(r.location.as_deref(), Some("/"));
    let r = s.get("/recipient?anim=1&id=2&giver=2").await;
    assert_eq!(r.location.as_deref(), Some("/"));
    let revealed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM assignments WHERE revealed_at IS NOT NULL")
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert_eq!(revealed, 0);
    let _ = ids;
}

#[tokio::test]
async fn recipient_is_persistent_private_and_switchable() {
    let (app, state) = test_app().await;
    add_students(&state, 5, true).await;
    perform_draw(&state.db, admin_id(&state).await, "admin")
        .await
        .unwrap();
    set_reveal(&app, true).await;

    let rows = assignments(&state).await;
    let receiver_of: HashMap<i64, i64> = rows.iter().copied().collect();
    // élève n°1 (id 2 car l'admin a l'id 1)
    let (giver_id, giver_name) = (2i64, "eleve1");
    let receiver_id = receiver_of[&giver_id];
    let receiver_first: String = sqlx::query_scalar("SELECT first_name FROM users WHERE id = $1")
        .bind(receiver_id)
        .fetch_one(&state.db)
        .await
        .unwrap();

    let mut s = student(&app, giver_name).await;
    let home = s.get("/").await;
    assert!(home.body.contains("Ton Secret Santa t'attend"));
    let t = csrf(&home.body);

    // pas de fiche avant la première révélation
    assert_eq!(s.get("/recipient").await.location.as_deref(), Some("/"));

    let r = s.post("/reveal", &[("csrf", &t)]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.location.as_deref(), Some("/recipient?anim=1"));
    let card = s.get("/recipient?anim=1").await;
    assert_eq!(card.status, StatusCode::OK);
    assert!(card.body.contains(&format!("{receiver_first} Nom")));
    assert!(card.body.contains(&format!("interet-secret-{receiver_id}")));
    assert!(card.body.contains("Tu vas offrir un cadeau à"));
    let first_reveal: String =
        sqlx::query_scalar("SELECT revealed_at FROM assignments WHERE giver_id = $1")
            .bind(giver_id)
            .fetch_one(&state.db)
            .await
            .unwrap();

    // seul son propre destinataire est visible : aucune autre préférence n'apparaît
    for (other_giver, other_receiver) in &rows {
        if *other_receiver != receiver_id {
            assert!(
                !card
                    .body
                    .contains(&format!("interet-secret-{other_receiver}")),
                "fuite de données (giver {other_giver})"
            );
        }
    }

    // déconnexion, nouvelle connexion plus tard : même personne, même date de première ouverture
    let t2 = csrf(&s.get("/").await.body);
    s.post("/logout", &[("csrf", &t2)]).await;
    let mut again = student(&app, giver_name).await;
    let home = again.get("/").await;
    assert!(home.body.contains("Voir mon destinataire"));
    assert!(home.body.contains("Revoir l'animation"));
    let card2 = again.get("/recipient").await;
    assert!(card2.body.contains(&format!("{receiver_first} Nom")));
    // la révélation répétée ne change pas revealed_at
    let t3 = csrf(&home.body);
    again.post("/reveal", &[("csrf", &t3)]).await;
    let second: String =
        sqlx::query_scalar("SELECT revealed_at FROM assignments WHERE giver_id = $1")
            .bind(giver_id)
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert_eq!(first_reveal, second);

    // un autre élève qui n'a pas révélé ne voit rien
    let mut other = student(&app, "eleve2").await;
    assert_eq!(other.get("/recipient").await.location.as_deref(), Some("/"));

    // l'admin peut suspendre sans toucher aux attributions
    set_reveal(&app, false).await;
    assert_eq!(again.get("/recipient").await.location.as_deref(), Some("/"));
    let t4 = csrf(&again.get("/").await.body);
    assert_eq!(
        again.post("/reveal", &[("csrf", &t4)]).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(assignments(&state).await, rows);
    set_reveal(&app, true).await;
    assert!(again
        .get("/recipient")
        .await
        .body
        .contains(&format!("{receiver_first} Nom")));
}

#[tokio::test]
async fn admin_can_view_assignments_only_with_password_and_it_is_logged() {
    let (app, state) = test_app().await;
    add_students(&state, 3, true).await;
    perform_draw(&state.db, admin_id(&state).await, "admin")
        .await
        .unwrap();
    let mut a = admin(&app).await;
    let page = a.get("/admin/assignments").await;
    let t = csrf(&page.body);

    let denied = a
        .post("/admin/assignments", &[("csrf", &t), ("password", "faux")])
        .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    assert!(!denied.body.contains("Nom1"));

    let ok = a
        .post(
            "/admin/assignments",
            &[("csrf", &t), ("password", ADMIN_PW)],
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK);
    assert!(ok.body.contains("Prenom1 Nom1") && ok.body.contains("Offre à"));

    let log = a.get("/admin/audit").await;
    assert!(log.body.contains("view_assignments"));
    assert!(log.body.contains("view_assignments_denied"));
    assert!(log.body.contains("draw"));
}

// ------------------------------------------------------------------ gestion des élèves

#[tokio::test]
async fn admin_manages_students_end_to_end() {
    let (app, state) = test_app().await;
    let mut a = admin(&app).await;
    let page = a.get("/admin/students").await;
    let t = csrf(&page.body);

    // ajout : l'identifiant est généré, aucun mot de passe n'existe
    let added = a
        .post(
            "/admin/students",
            &[("csrf", &t), ("first_name", "Léa"), ("last_name", "Dubois")],
        )
        .await;
    assert_eq!(added.status, StatusCode::OK);
    assert!(added.body.contains("lea.dubois"));
    assert!(!added.body.contains("code-big"));
    let stored: String =
        sqlx::query_scalar("SELECT password_hash FROM users WHERE username = 'lea.dubois'")
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert!(stored.is_empty());

    // première connexion : identifiant seul, puis choix du mot de passe
    let (mut lea, r) = login(&app, "lea.dubois", "", false).await;
    assert_eq!(r.location.as_deref(), Some("/set-password"));
    assert_eq!(
        lea.get("/").await.location.as_deref(),
        Some("/set-password")
    );
    let form = lea.get("/set-password").await;
    let lt = csrf(&form.body);
    let short = lea
        .post(
            "/set-password",
            &[("csrf", &lt), ("password", "court"), ("confirm", "court")],
        )
        .await;
    assert_eq!(short.status, StatusCode::UNPROCESSABLE_ENTITY);
    let diff = lea
        .post(
            "/set-password",
            &[
                ("csrf", &lt),
                ("password", "mon-secret-1"),
                ("confirm", "autre-chose"),
            ],
        )
        .await;
    assert_eq!(diff.status, StatusCode::UNPROCESSABLE_ENTITY);
    let ok = lea
        .post(
            "/set-password",
            &[
                ("csrf", &lt),
                ("password", "mon-secret-1"),
                ("confirm", "mon-secret-1"),
            ],
        )
        .await;
    assert_eq!(ok.location.as_deref(), Some("/"));
    let stored: String =
        sqlx::query_scalar("SELECT password_hash FROM users WHERE username = 'lea.dubois'")
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert!(stored.starts_with("$argon2id$"), "Argon2id attendu");
    assert_eq!(
        lea.get("/set-password").await.location.as_deref(),
        Some("/")
    );

    // ensuite, l'identifiant seul ne suffit plus
    assert_eq!(
        login(&app, "lea.dubois", "", false).await.1.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        login(&app, "lea.dubois", "mauvais", false).await.1.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        login(&app, "lea.dubois", "mon-secret-1", false)
            .await
            .1
            .status,
        StatusCode::SEE_OTHER
    );

    // import CSV avec doublon de nom
    let csv = "prénom,nom\nEmma,Martin\nLéa,Dubois\n;\nLucas;\n";
    let imp = a
        .post("/admin/students/import", &[("csrf", &t), ("csv", csv)])
        .await;
    assert_eq!(imp.status, StatusCode::OK);
    assert!(imp.body.contains("emma.martin"));
    assert!(imp.body.contains("lea.dubois2"));
    assert!(
        imp.body.contains("Ligne"),
        "les lignes invalides sont signalées"
    );
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role = 'student'")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(n, 3);

    // réinitialisation : sessions coupées, l'élève rechoisit son mot de passe
    let id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'lea.dubois'")
        .fetch_one(&state.db)
        .await
        .unwrap();
    let (mut lea, _) = login(&app, "lea.dubois", "mon-secret-1", false).await;
    let reset = a
        .post(&format!("/admin/students/{id}/reset"), &[("csrf", &t)])
        .await;
    assert_eq!(reset.status, StatusCode::OK);
    assert_eq!(lea.get("/").await.location.as_deref(), Some("/login"));
    let (_, again) = login(&app, "lea.dubois", "", false).await;
    assert_eq!(again.location.as_deref(), Some("/set-password"));

    // modification
    let edit = a
        .post(
            &format!("/admin/students/{id}/edit"),
            &[
                ("csrf", &t),
                ("first_name", "Léa"),
                ("last_name", "Durand"),
                ("username", "lea.d"),
            ],
        )
        .await;
    assert_eq!(edit.status, StatusCode::SEE_OTHER);
    let clash = a
        .post(
            &format!("/admin/students/{id}/edit"),
            &[
                ("csrf", &t),
                ("first_name", "Léa"),
                ("last_name", "Durand"),
                ("username", "emma.martin"),
            ],
        )
        .await;
    assert_eq!(clash.status, StatusCode::UNPROCESSABLE_ENTITY);

    // suppression avant le tirage
    let del = a
        .post(&format!("/admin/students/{id}/delete"), &[("csrf", &t)])
        .await;
    assert_eq!(del.status, StatusCode::SEE_OTHER);
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role = 'student'")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(n, 2);

    // pas de suppression ni d'ajout après le tirage
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM users WHERE role = 'student'")
        .fetch_all(&state.db)
        .await
        .unwrap();
    for id in &ids {
        set_prefs(&state, *id).await;
    }
    perform_draw(&state.db, admin_id(&state).await, "admin")
        .await
        .unwrap();
    let del = a
        .post(
            &format!("/admin/students/{}/delete", ids[0]),
            &[("csrf", &t)],
        )
        .await;
    assert_eq!(del.status, StatusCode::UNPROCESSABLE_ENTITY);
    let add = a
        .post(
            "/admin/students",
            &[("csrf", &t), ("first_name", "Tard"), ("last_name", "Venu")],
        )
        .await;
    assert_eq!(add.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(assignments(&state).await.len(), 2);
}

#[tokio::test]
async fn settings_are_saved_and_shown_to_students() {
    let (app, state) = test_app().await;
    add_students(&state, 2, true).await;
    let mut a = admin(&app).await;
    let t = csrf(&a.get("/admin/settings").await.body);
    let r = a
        .post(
            "/admin/settings",
            &[
                ("csrf", &t),
                ("class_name", "3ème B"),
                ("event_name", "Noël 2026"),
                ("budget", "12 €"),
                ("exchange_date", "2026-12-18"),
                ("welcome_message", "Joyeuses fêtes !"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let mut s = student(&app, "eleve1").await;
    let home = s.get("/").await;
    assert!(
        home.body.contains("12 €")
            && home.body.contains("18/12/2026")
            && home.body.contains("Joyeuses fêtes")
    );
}

// ------------------------------------------------------------------ divers

#[tokio::test]
async fn health_and_security_headers() {
    let (app, _state) = test_app().await;
    let mut c = Client::new(app);
    let h = c.get("/health").await;
    assert_eq!(h.status, StatusCode::OK);
    let login = c.get("/login").await;
    assert!(login
        .body
        .contains("Connecte-toi pour préparer ton cadeau de Noël"));
    assert!(login.headers.contains_key(header::CONTENT_SECURITY_POLICY));
    assert_eq!(login.headers.get(header::X_FRAME_OPTIONS).unwrap(), "DENY");
    assert_eq!(c.get("/static/style.css").await.status, StatusCode::OK);
    assert_eq!(c.get("/nimporte-quoi").await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_logs_in_from_the_single_login_page() {
    let (app, _state) = test_app().await;
    let (_, r) = login(&app, "admin", ADMIN_PW, false).await;
    assert_eq!(r.location.as_deref(), Some("/admin"));
    let (_, bad) = login(&app, "admin", "", false).await;
    assert_eq!(
        bad.status,
        StatusCode::UNAUTHORIZED,
        "l'admin a toujours un mot de passe"
    );
    let mut c = Client::new(app.clone());
    assert_eq!(
        c.get("/admin/login").await.location.as_deref(),
        Some("/login")
    );
}
