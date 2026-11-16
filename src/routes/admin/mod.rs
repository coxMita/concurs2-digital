//! Admin panel (`/admin`).

pub mod account;
pub mod exams;

use axum::response::Redirect;

use crate::auth::CurrentAdmin;

pub async fn home(_admin: CurrentAdmin) -> Redirect {
    Redirect::to("/admin/teste")
}
