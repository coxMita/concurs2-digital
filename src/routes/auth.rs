//! Admin login/logout.

use askama::Template;
use axum::{
    Form,
    extract::State,
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use tower_sessions::Session;

use crate::{
    auth::ADMIN_KEY,
    db::admins,
    error::AppResult,
    services::credentials::normalize_username,
    state::AppState,
    web::{Ctx, render},
};

pub const GENERIC_LOGIN_ERROR: &str = "Utilizator sau parolă greșită.";
pub const RATE_LIMIT_ERROR: &str =
    "Prea multe încercări nereușite pentru acest utilizator. Așteptați un minut.";

#[derive(Template)]
#[template(path = "admin/login.html")]
struct AdminLoginPage {
    ctx: Ctx,
    error: Option<&'static str>,
    username: String,
}

#[derive(Deserialize)]
pub struct LoginForm {
    pub username: String,
    pub password: String,
}

pub async fn admin_login_page(
    State(state): State<AppState>,
    session: Session,
) -> AppResult<Response> {
    if session.get::<uuid::Uuid>(ADMIN_KEY).await?.is_some() {
        return Ok(Redirect::to("/admin").into_response());
    }
    if !admins::any_exists(&state.db).await? {
        return Ok(Redirect::to("/setup").into_response());
    }
    let ctx = Ctx::build(&session, "admin-login", None, "").await?;
    Ok(render(&AdminLoginPage {
        ctx,
        error: None,
        username: String::new(),
    })?
    .into_response())
}

pub async fn admin_login(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<LoginForm>,
) -> AppResult<Response> {
    let username = normalize_username(&form.username);
    let limiter_key = format!("admin:{username}");
    let error = if state.login_limiter.is_blocked(&limiter_key) {
        Some(RATE_LIMIT_ERROR)
    } else {
        let creds = admins::credentials(&state.db, &username).await?;
        let (id, hash) = match creds {
            Some((id, h)) => (Some(id), Some(h)),
            None => (None, None),
        };
        let ok = state.hasher.verify(form.password, hash).await?;
        if let (true, Some(id)) = (ok, id) {
            state.login_limiter.clear(&limiter_key);
            session.cycle_id().await?;
            session.insert(ADMIN_KEY, id).await?;
            tracing::info!(%username, "admin logged in");
            return Ok(Redirect::to("/admin").into_response());
        }
        state.login_limiter.record_failure(&limiter_key);
        Some(GENERIC_LOGIN_ERROR)
    };
    let ctx = Ctx::build(&session, "admin-login", None, "").await?;
    Ok(render(&AdminLoginPage {
        ctx,
        error,
        username,
    })?
    .into_response())
}

pub async fn logout(session: Session) -> AppResult<Redirect> {
    let was_admin = session.get::<uuid::Uuid>(ADMIN_KEY).await?.is_some();
    session.flush().await?;
    Ok(Redirect::to(if was_admin { "/admin/login" } else { "/" }))
}
