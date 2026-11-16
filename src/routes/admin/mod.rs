//! Admin panel (`/admin`).

pub mod account;
pub mod exams;
pub mod questions;
pub mod results;
pub mod students;

use axum::{
    Router,
    response::Redirect,
    routing::{get, post},
};

use crate::{auth::CurrentAdmin, state::AppState};

pub async fn home(_admin: CurrentAdmin) -> Redirect {
    Redirect::to("/admin/teste")
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(home))
        .route("/cont", get(account::show))
        .route("/cont/parola", post(account::change_password))
        .route("/cont/admini", post(account::add_admin))
        .route("/cont/admini/{id}/sterge", post(account::delete_admin))
        .route("/teste", get(exams::list))
        .route("/teste/nou", get(exams::new_page).post(exams::create))
        .route("/teste/{id}", get(exams::edit_page).post(exams::update))
        .route("/teste/{id}/stare", post(exams::set_status))
        .route("/teste/{id}/duplica", post(exams::duplicate))
        .route("/teste/{id}/sterge", post(exams::delete))
        .route("/teste/{id}/recalculeaza", post(exams::regrade))
        .route("/teste/{id}/intrebari", get(questions::list))
        .route(
            "/teste/{id}/intrebari/noua",
            get(questions::new_page).post(questions::create),
        )
        .route("/teste/{id}/intrebari/ordine", post(questions::reorder))
        .route(
            "/teste/{id}/intrebari/{qid}",
            get(questions::edit_page).post(questions::update),
        )
        .route(
            "/teste/{id}/intrebari/{qid}/sterge",
            post(questions::delete),
        )
        .route(
            "/teste/{id}/intrebari/{qid}/duplica",
            post(questions::duplicate),
        )
        .route("/teste/{id}/intrebari/{qid}/sus", post(questions::move_up))
        .route(
            "/teste/{id}/intrebari/{qid}/jos",
            post(questions::move_down),
        )
        .route(
            "/teste/{id}/import",
            get(questions::import_page).post(questions::import_submit),
        )
        .route("/teste/{id}/export-intrebari", get(questions::export))
        .route("/teste/{id}/elevi", get(students::list))
        .route("/teste/{id}/elevi/genereaza", post(students::generate))
        .route(
            "/teste/{id}/elevi/credentiale.xlsx",
            post(students::credentials_download),
        )
        .route("/teste/{id}/elevi/sterge", post(students::delete_many))
        .route("/teste/{id}/elevi/{sid}/nume", post(students::rename))
        .route(
            "/teste/{id}/elevi/{sid}/parola",
            post(students::reset_password),
        )
        .route("/teste/{id}/elevi/{sid}/timp", post(students::extra_time))
        .route("/teste/{id}/elevi/{sid}/reluare", post(students::retake))
        .route("/teste/{id}/live", get(results::live))
        .route("/teste/{id}/live/tabel", get(results::live_table))
        .route("/teste/{id}/rezultate", get(results::results_page))
        .route(
            "/teste/{id}/rezultate.xlsx",
            get(results::results_xlsx_download),
        )
}
