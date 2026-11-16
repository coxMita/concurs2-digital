//! Admin account settings: own password, other admin accounts.

use askama::Template;
use axum::{
    Form,
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    auth::CurrentAdmin,
    db::admins::{self, Admin},
    error::AppResult,
    routes::validate::{validate_admin_username, validate_new_password},
    services::credentials::normalize_username,
    state::AppState,
    web::{Ctx, fmt_dt, render, set_flash},
};

#[derive(Template)]
#[template(path = "admin/account.html")]
struct AccountPage {
    ctx: Ctx,
    me: Uuid,
    admins: Vec<Admin>,
}

impl AccountPage {
    fn created(&self, a: &Admin) -> String {
        fmt_dt(&a.created_at)
    }
}

pub async fn show(State(state): State<AppState>, admin: CurrentAdmin) -> AppResult<Response> {
    let ctx = Ctx::build(
        &admin.session,
        "admin",
        Some(admin.username.clone()),
        "cont",
    )
    .await?;
    let admins = admins::list(&state.db).await?;
    Ok(render(&AccountPage {
        ctx,
        me: admin.id,
        admins,
    })?
    .into_response())
}

#[derive(Deserialize)]
pub struct PasswordForm {
    current: String,
    password: String,
    password_confirm: String,
}

pub async fn change_password(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Form(form): Form<PasswordForm>,
) -> AppResult<Redirect> {
    let hash = admins::password_hash(&state.db, admin.id).await?;
    if !state.hasher.verify(form.current, hash).await? {
        set_flash(&admin.session, "error", "Parola actuală este greșită.").await?;
        return Ok(Redirect::to("/admin/cont"));
    }
    if let Err(e) = validate_new_password(&form.password, &form.password_confirm) {
        set_flash(&admin.session, "error", e).await?;
        return Ok(Redirect::to("/admin/cont"));
    }
    let hash = state.hasher.hash(form.password).await?;
    admins::set_password(&state.db, admin.id, &hash).await?;
    set_flash(&admin.session, "ok", "Parola a fost schimbată.").await?;
    Ok(Redirect::to("/admin/cont"))
}

#[derive(Deserialize)]
pub struct NewAdminForm {
    username: String,
    password: String,
    password_confirm: String,
}

pub async fn add_admin(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Form(form): Form<NewAdminForm>,
) -> AppResult<Redirect> {
    let username = normalize_username(&form.username);
    if let Err(e) = validate_admin_username(&username)
        .and_then(|()| validate_new_password(&form.password, &form.password_confirm))
    {
        set_flash(&admin.session, "error", e).await?;
        return Ok(Redirect::to("/admin/cont"));
    }
    let hash = state.hasher.hash(form.password).await?;
    match admins::create(&state.db, &username, &hash).await? {
        Some(_) => {
            set_flash(
                &admin.session,
                "ok",
                format!("Administratorul „{username}” a fost adăugat."),
            )
            .await?
        }
        None => {
            set_flash(
                &admin.session,
                "error",
                "Acest nume de utilizator există deja.",
            )
            .await?
        }
    }
    Ok(Redirect::to("/admin/cont"))
}

pub async fn delete_admin(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Redirect> {
    if id == admin.id {
        set_flash(
            &admin.session,
            "error",
            "Nu vă puteți șterge propriul cont.",
        )
        .await?;
    } else if admins::delete(&state.db, id).await? {
        set_flash(&admin.session, "ok", "Administratorul a fost șters.").await?;
    } else {
        set_flash(
            &admin.session,
            "error",
            "Administratorul nu a putut fi șters.",
        )
        .await?;
    }
    Ok(Redirect::to("/admin/cont"))
}
