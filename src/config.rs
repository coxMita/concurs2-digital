//! Configuration read from environment variables (with `.env` support for local dev).

use std::{env, net::SocketAddr, path::PathBuf, str::FromStr};

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: SocketAddr,
    /// Public domain, used only for log messages (e.g. the /setup URL).
    pub domain: String,
    pub db_max_connections: u32,
    pub cookie_secure: bool,
    pub setup_token: Option<String>,
    pub upload_dir: PathBuf,
    pub static_dir: PathBuf,
    pub max_upload_bytes: usize,
    pub argon2_memory_kib: u32,
    pub argon2_iterations: u32,
    /// Failed login attempts allowed per username per minute.
    pub login_fail_per_user_per_min: u32,
    /// Login POSTs allowed per client IP per minute (abuse protection only).
    pub login_per_ip_per_min: u32,
    /// How often the background finalizer runs.
    pub finalizer_interval_secs: u64,
}

fn var<T: FromStr>(name: &str, default: T) -> T {
    match env::var(name) {
        Ok(v) if !v.trim().is_empty() => v.trim().parse().unwrap_or_else(|_| {
            tracing::warn!("invalid value for {name}, using default");
            default
        }),
        _ => default,
    }
}

impl Config {
    pub fn from_env() -> Self {
        let database_url = env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgres://concurs:concurs@localhost:5432/concurs".into());
        let setup_token = env::var("SETUP_TOKEN")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        Self {
            database_url,
            bind_addr: var("BIND_ADDR", SocketAddr::from(([0, 0, 0, 0], 8080))),
            domain: var("DOMAIN", "localhost".to_string()),
            db_max_connections: var("DB_MAX_CONNECTIONS", 25),
            cookie_secure: var("COOKIE_SECURE", true),
            setup_token,
            upload_dir: var("UPLOAD_DIR", PathBuf::from("./data/uploads")),
            static_dir: var("STATIC_DIR", PathBuf::from("./static")),
            max_upload_bytes: var("MAX_UPLOAD_MB", 3usize) * 1024 * 1024,
            argon2_memory_kib: var("ARGON2_MEMORY_KIB", 19_456),
            argon2_iterations: var("ARGON2_ITERATIONS", 2),
            login_fail_per_user_per_min: var("LOGIN_FAIL_PER_USER_PER_MIN", 10),
            login_per_ip_per_min: var("LOGIN_PER_IP_PER_MIN", 600),
            finalizer_interval_secs: var("FINALIZER_INTERVAL_SECS", 30),
        }
    }
}
