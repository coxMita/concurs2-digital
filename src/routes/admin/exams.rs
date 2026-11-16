//! Exam list and CRUD.

use askama::Template;
use axum::response::Html;

use crate::{
    auth::CurrentAdmin,
    error::AppResult,
    web::{Ctx, render},
};

#[derive(Template)]
#[template(path = "admin/exams.html")]
struct ExamsPage {
    ctx: Ctx,
}

pub async fn list(admin: CurrentAdmin) -> AppResult<Html<String>> {
    let ctx = Ctx::build(&admin.session, "admin", Some(admin.username), "teste").await?;
    render(&ExamsPage { ctx })
}
