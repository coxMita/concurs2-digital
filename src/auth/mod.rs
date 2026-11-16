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

impl CurrentAdmin {
    /// Page context for admin pages.
    pub async fn ctx(&self, nav: &'static str) -> Result<crate::web::Ctx, AppError> {
        crate::web::Ctx::build(&self.session, "admin", Some(self.username.clone()), nav).await
    }

    pub async fn flash(&self, kind: &str, message: impl Into<String>) -> Result<(), AppError> {
        crate::web::set_flash(&self.session, kind, message).await
    }
}

pub const STUDENT_KEY: &str = "student_id";
pub const STUDENT_SESSION_KEY: &str = "student_sess";

/// The logged-in student, with their attempt. Rejects sessions replaced by a newer login.
pub struct CurrentStudent {
    pub state: crate::db::attempts::StudentState,
    pub session: Session,
}

impl FromRequestParts<AppState> for CurrentStudent {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        let hx = is_htmx(&parts.headers);
        let session = Session::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::StudentLogin { hx })?;
        let (Some(id), Some(sess)) = (
            session.get::<Uuid>(STUDENT_KEY).await?,
            session.get::<Uuid>(STUDENT_SESSION_KEY).await?,
        ) else {
            return Err(AppError::StudentLogin { hx });
        };
        let student = crate::db::attempts::student_state(&state.db, id).await?;
        match student {
            Some(s) if s.active_session == Some(sess) => Ok(Self { state: s, session }),
            Some(_) => {
                session.flush().await?;
                crate::web::set_flash(
                    &session,
                    "error",
                    "Ai fost deconectat deoarece contul tău a fost folosit pentru autentificare pe alt dispozitiv.",
                )
                .await?;
                Err(AppError::StudentLogin { hx })
            }
            None => {
                session.flush().await?;
                Err(AppError::StudentLogin { hx })
            }
        }
    }
}

impl CurrentStudent {
    pub async fn ctx(&self) -> Result<crate::web::Ctx, AppError> {
        let name = self
            .state
            .full_name
            .clone()
            .unwrap_or_else(|| self.state.username.clone());
        crate::web::Ctx::build(&self.session, "student", Some(name), "").await
    }
}
