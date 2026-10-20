use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub bind: String,
    pub cookie_secure: bool,
    pub trust_proxy: bool,
    pub admin_username: String,
    pub admin_password: Option<String>,
    pub session_days: i64,
}

fn flag(name: &str, default: bool) -> bool {
    env::var(name)
        .map(|v| {
            matches!(
                v.trim().to_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(default)
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "postgres://santa:santa@localhost:5432/santa".into()),
            bind: env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".into()),
            cookie_secure: flag("COOKIE_SECURE", false),
            trust_proxy: flag("TRUST_PROXY", false),
            admin_username: env::var("ADMIN_USERNAME").unwrap_or_else(|_| "admin".into()),
            admin_password: env::var("ADMIN_PASSWORD").ok().filter(|p| !p.is_empty()),
            session_days: env::var("SESSION_DAYS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
        }
    }
}
