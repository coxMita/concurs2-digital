//! Central application error, mapped to HTTP responses with styled Romanian error pages.

use askama::Template;
use axum::{
    http::{HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
};

use crate::web::Ctx;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("not found")]
    NotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("invalid CSRF token")]
    Csrf,
    #[error("too many requests")]
    TooManyRequests,
    /// Admin must log in; `hx` is true for HTMX requests (redirect via HX-Redirect).
    #[error("admin login required")]
    AdminLogin { hx: bool },
    /// Student must log in (or their session was replaced by a newer login).
    #[error("student login required")]
    StudentLogin { hx: bool },
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("session error: {0}")]
    Session(#[from] tower_sessions::session::Error),
    #[error("template error: {0}")]
    Template(#[from] askama::Error),
    #[error("internal error: {0}")]
    Internal(String),
}

pub type AppResult<T> = Result<T, AppError>;

#[derive(Template)]
#[template(path = "errors/error.html")]
struct ErrorPage {
    ctx: Ctx,
    code: u16,
    title: &'static str,
    message: String,
}

fn hx_redirect(to: &'static str) -> Response {
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut()
        .insert("HX-Redirect", HeaderValue::from_static(to));
    res
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, title, message) = match &self {
            AppError::AdminLogin { hx: true } => return hx_redirect("/admin/login"),
            AppError::AdminLogin { hx: false } => {
                return Redirect::to("/admin/login").into_response();
            }
            AppError::StudentLogin { hx: true } => return hx_redirect("/"),
            AppError::StudentLogin { hx: false } => return Redirect::to("/").into_response(),
            AppError::NotFound => (
                StatusCode::NOT_FOUND,
                "Pagina nu a fost găsită",
                "Adresa accesată nu există sau nu mai este disponibilă.".to_string(),
            ),
            AppError::Forbidden => (
                StatusCode::FORBIDDEN,
                "Acces interzis",
                "Nu aveți permisiunea de a accesa această pagină.".to_string(),
            ),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, "Cerere invalidă", msg.clone()),
            AppError::Csrf => (
                StatusCode::FORBIDDEN,
                "Sesiune expirată",
                "Formularul a expirat. Reîncărcați pagina și încercați din nou.".to_string(),
            ),
            AppError::TooManyRequests => (
                StatusCode::TOO_MANY_REQUESTS,
                "Prea multe încercări",
                "Așteptați un minut și încercați din nou.".to_string(),
            ),
            AppError::Db(_)
            | AppError::Session(_)
            | AppError::Template(_)
            | AppError::Internal(_) => {
                tracing::error!(error = %self, "request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "A apărut o eroare",
                    "Ceva nu a funcționat. Încercați din nou peste câteva momente.".to_string(),
                )
            }
        };
        error_page(status, title, message)
    }
}

pub fn error_page(status: StatusCode, title: &'static str, message: String) -> Response {
    let page = ErrorPage {
        ctx: Ctx::anonymous(),
        code: status.as_u16(),
        title,
        message,
    };
    match page.render() {
        Ok(html) => {
            let mut res = (status, Html(html)).into_response();
            res.headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            res
        }
        Err(_) => (status, title).into_response(),
    }
}
