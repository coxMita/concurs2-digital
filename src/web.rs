//! View helpers shared by all pages: page context, rendering, flash messages, formatting.

use std::sync::OnceLock;

use askama::Template;
use axum::response::Html;
use chrono::{DateTime, Utc};
use chrono_tz::Europe::Bucharest;
use serde::{Deserialize, Serialize};
use tower_sessions::Session;

use crate::{auth::csrf, error::AppResult};

static ASSET_VERSION: OnceLock<String> = OnceLock::new();

/// Set once at startup to a hash of the static files (cache busting).
pub fn set_asset_version(v: String) {
    let _ = ASSET_VERSION.set(v);
}

pub fn asset_version() -> &'static str {
    ASSET_VERSION.get().map(String::as_str).unwrap_or("dev")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Flash {
    /// "ok" or "error"
    pub kind: String,
    pub message: String,
}

const FLASH_KEY: &str = "flash";

pub async fn set_flash(session: &Session, kind: &str, message: impl Into<String>) -> AppResult<()> {
    session
        .insert(
            FLASH_KEY,
            Flash {
                kind: kind.into(),
                message: message.into(),
            },
        )
        .await?;
    Ok(())
}

/// Data every page template needs.
#[derive(Clone, Debug, Default)]
pub struct Ctx {
    pub csrf: String,
    pub v: &'static str,
    /// Display name of the logged-in user, if any.
    pub user: Option<String>,
    /// "admin" or "student" area; selects the header variant.
    pub area: &'static str,
    /// Active admin nav item.
    pub nav: &'static str,
    pub flash: Option<Flash>,
}

impl Ctx {
    pub fn anonymous() -> Self {
        Self {
            v: asset_version(),
            ..Default::default()
        }
    }

    pub async fn build(
        session: &Session,
        area: &'static str,
        user: Option<String>,
        nav: &'static str,
    ) -> AppResult<Self> {
        let csrf = csrf::token(session).await?;
        let flash = session.remove::<Flash>(FLASH_KEY).await?;
        Ok(Self {
            csrf,
            v: asset_version(),
            user,
            area,
            nav,
            flash,
        })
    }
}

pub fn render<T: Template>(t: &T) -> AppResult<Html<String>> {
    Ok(Html(t.render()?))
}

/// Formats a UTC timestamp in Europe/Bucharest, e.g. "17.11.2026 14:05".
pub fn fmt_dt(dt: &DateTime<Utc>) -> String {
    dt.with_timezone(&Bucharest)
        .format("%d.%m.%Y %H:%M")
        .to_string()
}

pub fn fmt_dt_opt(dt: &Option<DateTime<Utc>>) -> String {
    dt.as_ref().map(fmt_dt).unwrap_or_default()
}

/// Whether the request came from HTMX.
pub fn is_htmx(headers: &axum::http::HeaderMap) -> bool {
    headers.contains_key("hx-request")
}

/// Short hash of all files under `dir`, appended as `?v=` to asset URLs.
pub fn compute_asset_version(dir: &std::path::Path) -> String {
    use std::hash::{Hash, Hasher};
    fn walk(dir: &std::path::Path, h: &mut std::collections::hash_map::DefaultHasher) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(&p, h);
            } else if let Ok(bytes) = std::fs::read(&p) {
                p.to_string_lossy().hash(h);
                bytes.hash(h);
            }
        }
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    walk(dir, &mut h);
    format!("{:x}", h.finish() & 0xffff_ffff)
}

/// ASCII file-name-safe slug ("Test de română" -> "test-de-romana").
pub fn slug(s: &str) -> String {
    let base = crate::services::credentials::username_base_from_name(s).unwrap_or_default();
    let slug: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() { "test".into() } else { slug }
}
