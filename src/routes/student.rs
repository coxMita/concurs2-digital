//! Student area: login, exam, results.

use askama::Template;
use axum::response::Html;
use tower_sessions::Session;

use crate::{
    error::AppResult,
    web::{Ctx, render},
};

#[derive(Template)]
#[template(path = "student/login.html")]
struct LoginPage {
    ctx: Ctx,
    error: Option<&'static str>,
    username: String,
}

pub async fn login_page(session: Session) -> AppResult<Html<String>> {
    let ctx = Ctx::build(&session, "student", None, "").await?;
    render(&LoginPage {
        ctx,
        error: None,
        username: String::new(),
    })
}
