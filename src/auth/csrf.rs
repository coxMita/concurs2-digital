//! Synchronizer-token CSRF protection.
//!
//! The token lives in the session. Unsafe requests must carry it either in the
//! `X-CSRF-Token` header (HTMX, set globally via `hx-headers`), in a `_csrf` form
//! field (url-encoded forms) or in a `_csrf` query parameter (multipart uploads).

use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::{Method, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use tower_sessions::Session;

use crate::{error::AppError, services::credentials::random_token};

const CSRF_KEY: &str = "csrf";
pub const FIELD: &str = "_csrf";
const HEADER: &str = "x-csrf-token";
const MAX_FORM_BYTES: usize = 2 * 1024 * 1024;

/// Returns the session's CSRF token, creating it if needed.
pub async fn token(session: &Session) -> Result<String, AppError> {
    if let Some(t) = session.get::<String>(CSRF_KEY).await? {
        return Ok(t);
    }
    let t = random_token(32);
    session.insert(CSRF_KEY, &t).await?;
    Ok(t)
}

pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn field_from_query(query: Option<&str>) -> Option<String> {
    let pairs: Vec<(String, String)> = serde_urlencoded::from_str(query?).ok()?;
    pairs.into_iter().find(|(k, _)| k == FIELD).map(|(_, v)| v)
}

/// Middleware validating the CSRF token on every state-changing request.
pub async fn verify(req: Request, next: Next) -> Response {
    if matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        return next.run(req).await;
    }
    let Some(session) = req.extensions().get::<Session>().cloned() else {
        return AppError::Csrf.into_response();
    };
    let expected = match session.get::<String>(CSRF_KEY).await {
        Ok(Some(t)) => t,
        _ => return AppError::Csrf.into_response(),
    };

    let header_token = req
        .headers()
        .get(HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    if let Some(t) = header_token {
        return if constant_time_eq(t.as_bytes(), expected.as_bytes()) {
            next.run(req).await
        } else {
            AppError::Csrf.into_response()
        };
    }

    if let Some(t) = field_from_query(req.uri().query()) {
        return if constant_time_eq(t.as_bytes(), expected.as_bytes()) {
            next.run(req).await
        } else {
            AppError::Csrf.into_response()
        };
    }

    let is_form = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/x-www-form-urlencoded"));
    if !is_form {
        return AppError::Csrf.into_response();
    }

    // Buffer the (small) form body, check the field, then hand the body back.
    let (parts, body) = req.into_parts();
    let Ok(bytes) = to_bytes(body, MAX_FORM_BYTES).await else {
        return AppError::BadRequest("Formular prea mare.".into()).into_response();
    };
    let ok = serde_urlencoded::from_bytes::<Vec<(String, String)>>(&bytes)
        .ok()
        .and_then(|pairs| pairs.into_iter().find(|(k, _)| k == FIELD))
        .is_some_and(|(_, v)| constant_time_eq(v.as_bytes(), expected.as_bytes()));
    if !ok {
        return AppError::Csrf.into_response();
    }
    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}
