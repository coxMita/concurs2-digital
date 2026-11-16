//! First-run setup: creates the first admin, protected by SETUP_TOKEN.
//! Permanently disabled (404) once any admin exists.

use askama::Template;
use axum::{
    Form,
    extract::State,
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use tower_sessions::Session;

use crate::{
    auth::{ADMIN_KEY, csrf::constant_time_eq},
    db::{admins, settings},
    error::{AppError, AppResult},
    routes::validate::{validate_admin_username, validate_new_password},
    services::credentials::{normalize_username, random_token},
    state::AppState,
    web::{Ctx, render},
};

pub const SETUP_TOKEN_KEY: &str = "setup_token";

#[derive(Template)]
#[template(path = "setup.html")]
struct SetupPage {
    ctx: Ctx,
    error: Option<String>,
    username: String,
}

/// Returns the effective setup token: SETUP_TOKEN from env, else a generated one stored in the DB.
pub async fn effective_token(state: &AppState) -> Result<String, sqlx::Error> {
    if let Some(t) = &state.config.setup_token {
        return Ok(t.clone());
    }
    let bytes =
        settings::get_or_create(&state.db, SETUP_TOKEN_KEY, || random_token(24).into_bytes())
            .await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Called at startup: when no admin exists, print the setup URL and token.
pub async fn announce_if_needed(state: &AppState) -> Result<(), sqlx::Error> {
    if admins::any_exists(&state.db).await? {
        return Ok(());
    }
    let token = effective_token(state).await?;
    let source = if state.config.setup_token.is_some() {
        "from SETUP_TOKEN in .env"
    } else {
        "generated"
    };
    let line = "=".repeat(64);
    tracing::warn!(
        "\n{line}\n  FIRST-RUN SETUP: no admin account exists yet.\n  Open:  https://{}/setup\n  Setup token ({source}):  {token}\n{line}",
        state.config.domain
    );
    Ok(())
}

pub async fn show(State(state): State<AppState>, session: Session) -> AppResult<Response> {
    if admins::any_exists(&state.db).await? {
        return Err(AppError::NotFound);
    }
    let ctx = Ctx::build(&session, "setup", None, "").await?;
    Ok(render(&SetupPage {
        ctx,
        error: None,
        username: String::new(),
    })?
    .into_response())
}

#[derive(Deserialize)]
pub struct SetupForm {
    token: String,
    username: String,
    password: String,
    password_confirm: String,
}

pub async fn submit(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<SetupForm>,
) -> AppResult<Response> {
    if admins::any_exists(&state.db).await? {
        return Err(AppError::NotFound);
    }
    let username = normalize_username(&form.username);
    let expected = effective_token(&state).await?;

    let error = if !constant_time_eq(form.token.trim().as_bytes(), expected.as_bytes()) {
        Some("Codul de configurare este greșit. Îl găsiți în jurnalul aplicației (docker compose logs app) sau în fișierul .env.".to_string())
    } else if let Err(e) = validate_admin_username(&username) {
        Some(e)
    } else {
        validate_new_password(&form.password, &form.password_confirm).err()
    };
    if let Some(error) = error {
        let ctx = Ctx::build(&session, "setup", None, "").await?;
        return Ok(render(&SetupPage {
            ctx,
            error: Some(error),
            username,
        })?
        .into_response());
    }

    let hash = state.hasher.hash(form.password).await?;
    let mut tx = state.db.begin().await?;
    let Some(id) = admins::create_first(&mut tx, &username, &hash).await? else {
        return Err(AppError::NotFound);
    };
    tx.commit().await?;
    settings::delete(&state.db, SETUP_TOKEN_KEY).await?;
    tracing::info!(%username, "first admin created via /setup");

    session.cycle_id().await?;
    session.insert(ADMIN_KEY, id).await?;
    crate::web::set_flash(
        &session,
        "ok",
        "Contul de administrator a fost creat. Bine ați venit!",
    )
    .await?;
    Ok(Redirect::to("/admin").into_response())
}
