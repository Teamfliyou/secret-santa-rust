use askama::Template;

use crate::auth::Session;

/// Contexte commun à toutes les pages (utilisé par `base.html`).
#[derive(Clone, Default)]
pub struct Ctx {
    pub title: String,
    pub user: Option<String>,
    pub is_admin: bool,
    pub csrf: String,
    pub flash: Option<String>,
    pub error: Option<String>,
}

impl Ctx {
    pub fn anon(title: &str) -> Self {
        Self {
            title: title.into(),
            ..Default::default()
        }
    }

    pub fn with_csrf(title: &str, csrf: &str) -> Self {
        Self {
            title: title.into(),
            csrf: csrf.into(),
            ..Default::default()
        }
    }

    pub fn session(title: &str, s: &Session) -> Self {
        Self {
            title: title.into(),
            user: Some(format!("{} {}", s.user.first_name, s.user.last_name)),
            is_admin: s.is_admin(),
            csrf: s.csrf.clone(),
            ..Default::default()
        }
    }

    pub fn flash(mut self, msg: Option<String>) -> Self {
        self.flash = msg;
        self
    }

    pub fn error(mut self, msg: impl Into<String>) -> Self {
        self.error = Some(msg.into());
        self
    }
}

#[derive(Clone, Default, sqlx::FromRow)]
pub struct Settings {
    pub class_name: String,
    pub event_name: String,
    pub budget: String,
    pub exchange_date: String,
    pub welcome_message: String,
    pub draw_completed_at: Option<String>,
    pub reveal_enabled: bool,
}

#[derive(Clone, Default)]
pub struct PrefForm {
    pub interests: String,
    pub hobbies: String,
    pub gift_types: String,
    pub favorite_snacks: String,
    pub favorite_colors: String,
    pub wishlist: String,
    pub dislikes: String,
    pub food_restrictions: String,
    pub message: String,
}

#[derive(Clone, Default, sqlx::FromRow)]
pub struct RecipientView {
    pub first_name: String,
    pub last_name: String,
    pub interests: String,
    pub hobbies: String,
    pub gift_types: String,
    pub favorite_snacks: String,
    pub favorite_colors: String,
    pub wishlist: String,
    pub dislikes: String,
    pub food_restrictions: String,
    pub message: String,
}

#[derive(Clone)]
pub struct StudentRow {
    pub id: i64,
    pub username: String,
    pub first_name: String,
    pub last_name: String,
    pub completed: bool,
    pub revealed: bool,
}

#[derive(Clone)]
pub struct Cred {
    pub name: String,
    pub username: String,
}

#[derive(Clone)]
pub struct AssignRow {
    pub giver: String,
    pub receiver: String,
    pub revealed: String,
}

#[derive(Clone)]
pub struct AuditRow {
    pub when: String,
    pub actor: String,
    pub action: String,
    pub details: String,
}

#[derive(Template)]
#[template(path = "error.html")]
pub struct ErrorTemplate {
    pub ctx: Ctx,
    pub code: u16,
    pub message: String,
}

#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginTemplate {
    pub ctx: Ctx,
    pub username: String,
}

#[derive(Template)]
#[template(path = "preferences.html")]
pub struct PreferencesTemplate {
    pub ctx: Ctx,
    pub v: PrefForm,
    pub first_time: bool,
}

#[derive(Template)]
#[template(path = "saved.html")]
pub struct SavedTemplate {
    pub ctx: Ctx,
    pub first_name: String,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct DashboardTemplate {
    pub ctx: Ctx,
    pub first_name: String,
    pub s: Settings,
    pub exchange_date: String,
    pub reveal_open: bool,
    pub revealed: bool,
}

#[derive(Template)]
#[template(path = "recipient.html")]
pub struct RecipientTemplate {
    pub ctx: Ctx,
    pub r: RecipientView,
    pub budget: String,
    pub anim: bool,
}

#[derive(Template)]
#[template(path = "admin_dashboard.html")]
pub struct AdminDashboardTemplate {
    pub ctx: Ctx,
    pub total: i64,
    pub done: i64,
    pub pending: i64,
    pub pct: i64,
    pub draw_done: bool,
    pub reveal_enabled: bool,
    pub revealed_count: i64,
}

#[derive(Template)]
#[template(path = "admin_students.html")]
pub struct AdminStudentsTemplate {
    pub ctx: Ctx,
    pub students: Vec<StudentRow>,
    pub draw_done: bool,
}

#[derive(Template)]
#[template(path = "admin_student_edit.html")]
pub struct AdminStudentEditTemplate {
    pub ctx: Ctx,
    pub s: StudentRow,
}

#[derive(Template)]
#[template(path = "admin_credentials.html")]
pub struct CredentialsTemplate {
    pub ctx: Ctx,
    pub heading: String,
    pub creds: Vec<Cred>,
    pub problems: Vec<String>,
}

#[derive(Template)]
#[template(path = "admin_settings.html")]
pub struct SettingsTemplate {
    pub ctx: Ctx,
    pub s: Settings,
}

#[derive(Template)]
#[template(path = "admin_draw.html")]
pub struct DrawTemplate {
    pub ctx: Ctx,
    pub total: i64,
    pub pending: Vec<String>,
    pub can_draw: bool,
    pub draw_done: bool,
    pub drawn_at: String,
    pub reveal_enabled: bool,
}

#[derive(Template)]
#[template(path = "admin_assignments.html")]
pub struct AssignmentsTemplate {
    pub ctx: Ctx,
    pub rows: Option<Vec<AssignRow>>,
}

#[derive(Template)]
#[template(path = "admin_audit.html")]
pub struct AuditTemplate {
    pub ctx: Ctx,
    pub rows: Vec<AuditRow>,
}

#[derive(Template)]
#[template(path = "set_password.html")]
pub struct SetPasswordTemplate {
    pub ctx: Ctx,
    pub first_name: String,
}
