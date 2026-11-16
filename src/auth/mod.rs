//! Authentication: session keys, extractors and CSRF.

pub mod csrf;
pub mod ratelimit;

use axum::{extract::FromRequestParts, http::request::Parts};
use tower_sessions::Session;
use uuid::Uuid;

use crate::{error::AppError, state::AppState, web::is_htmx};

pub const ADMIN_KEY: &str = "admin_id";

/// The logged-in administrator.
pub struct CurrentAdmin {
    pub id: Uuid,
    pub username: String,
    pub session: Session,
}

impl FromRequestParts<AppState> for CurrentAdmin {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        let hx = is_htmx(&parts.headers);
        let session = Session::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::AdminLogin { hx })?;
        let Some(id) = session.get::<Uuid>(ADMIN_KEY).await? else {
            return Err(AppError::AdminLogin { hx });
        };
        let username = sqlx::query_scalar!("SELECT username FROM admins WHERE id = $1", id)
            .fetch_optional(&state.db)
            .await?;
        match username {
            Some(username) => Ok(Self {
                id,
                username,
                session,
            }),
            None => {
                // Admin was deleted: drop the stale session.
                session.flush().await?;
                Err(AppError::AdminLogin { hx })
            }
        }
    }
}
