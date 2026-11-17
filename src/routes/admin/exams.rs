//! Exam list and CRUD, status changes.

use askama::Template;
use axum::{
    Form,
    extract::{Path, State},
    response::{Html, Redirect},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    auth::CurrentAdmin,
    db::exams::{self, Exam, ExamInput, ExamStatus, ExamSummary, ResultVisibility, ScoringMode},
    error::{AppError, AppResult},
    services::{finalizer, uploads},
    state::AppState,
    web::{Ctx, render},
};

pub async fn load(state: &AppState, id: Uuid) -> AppResult<Exam> {
    exams::get(&state.db, id).await?.ok_or(AppError::NotFound)
}

#[derive(Template)]
#[template(path = "admin/exams.html")]
struct ExamsPage {
    ctx: Ctx,
    exams: Vec<ExamSummary>,
}

pub async fn list(State(state): State<AppState>, admin: CurrentAdmin) -> AppResult<Html<String>> {
    let ctx = admin.ctx("teste").await?;
    let exams = exams::list(&state.db).await?;
    render(&ExamsPage { ctx, exams })
}

#[derive(Template)]
#[template(path = "admin/exam_form.html")]
struct ExamFormPage {
    ctx: Ctx,
    /// None when creating.
    exam: Option<Exam>,
    tab: &'static str,
    title: String,
    description: String,
    duration: i32,
    visibility: &'static str,
    scoring: &'static str,
    error: Option<String>,
    locked: bool,
}

#[derive(Deserialize)]
pub struct ExamForm {
    title: String,
    #[serde(default)]
    description: String,
    duration_minutes: String,
    result_visibility: ResultVisibility,
    #[serde(default = "default_scoring")]
    scoring: ScoringMode,
}

fn default_scoring() -> ScoringMode {
    ScoringMode::Partial
}

impl ExamForm {
    fn validate(&self) -> Result<i32, String> {
        let title = self.title.trim();
        if title.is_empty() || title.chars().count() > 200 {
            return Err("Titlul este obligatoriu (maximum 200 de caractere).".into());
        }
        if self.description.len() > 20_000 {
            return Err("Descrierea este prea lungă.".into());
        }
        match self.duration_minutes.trim().parse::<i32>() {
            Ok(d) if (1..=1440).contains(&d) => Ok(d),
            _ => Err("Durata trebuie să fie între 1 și 1440 de minute.".into()),
        }
    }
}

pub async fn new_page(admin: CurrentAdmin) -> AppResult<Html<String>> {
    render(&ExamFormPage {
        ctx: admin.ctx("teste").await?,
        exam: None,
        tab: "setari",
        title: String::new(),
        description: String::new(),
        duration: 30,
        visibility: "score",
        scoring: ScoringMode::Partial.as_str(),
        error: None,
        locked: false,
    })
}

pub async fn create(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Form(form): Form<ExamForm>,
) -> AppResult<axum::response::Response> {
    use axum::response::IntoResponse;
    match form.validate() {
        Err(error) => Ok(render(&ExamFormPage {
            ctx: admin.ctx("teste").await?,
            exam: None,
            tab: "setari",
            title: form.title,
            description: form.description,
            duration: 30,
            visibility: form.result_visibility.as_str(),
            scoring: form.scoring.as_str(),
            error: Some(error),
            locked: false,
        })?
        .into_response()),
        Ok(duration) => {
            let id = exams::create(
                &state.db,
                &ExamInput {
                    title: form.title.trim(),
                    description: form.description.trim(),
                    duration_minutes: duration,
                    result_visibility: form.result_visibility,
                    scoring: form.scoring,
                },
            )
            .await?;
            admin
                .flash("ok", "Testul a fost creat. Adăugați întrebările.")
                .await?;
            Ok(Redirect::to(&format!("/admin/teste/{id}/intrebari")).into_response())
        }
    }
}

pub async fn edit_page(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Html<String>> {
    let exam = load(&state, id).await?;
    let locked = exams::has_attempts(&state.db, id).await?;
    render(&ExamFormPage {
        ctx: admin.ctx("teste").await?,
        title: exam.title.clone(),
        description: exam.description.clone(),
        duration: exam.duration_minutes,
        visibility: exam.result_visibility.as_str(),
        scoring: exam.scoring.as_str(),
        exam: Some(exam),
        tab: "setari",
        error: None,
        locked,
    })
}

pub async fn update(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path(id): Path<Uuid>,
    Form(form): Form<ExamForm>,
) -> AppResult<Redirect> {
    let back = format!("/admin/teste/{id}");
    match form.validate() {
        Err(e) => admin.flash("error", e).await?,
        Ok(duration) => {
            let before = load(&state, id).await?;
            let ok = exams::update(
                &state.db,
                id,
                &ExamInput {
                    title: form.title.trim(),
                    description: form.description.trim(),
                    duration_minutes: duration,
                    result_visibility: form.result_visibility,
                    scoring: form.scoring,
                },
            )
            .await?;
            if !ok {
                return Err(AppError::NotFound);
            }
            state.cache.invalidate(id).await;
            if before.scoring != form.scoring {
                // Submitted grades follow the new rule right away.
                let n = finalizer::regrade_exam(&state, id).await?;
                admin
                    .flash(
                        "ok",
                        format!("Setările au fost salvate. Notele a {n} lucrări trimise au fost recalculate după noua regulă de punctaj."),
                    )
                    .await?;
            } else {
                admin.flash("ok", "Setările au fost salvate.").await?;
            }
        }
    }
    Ok(Redirect::to(&back))
}

#[derive(Deserialize)]
pub struct StatusForm {
    status: ExamStatus,
}

pub async fn set_status(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path(id): Path<Uuid>,
    Form(form): Form<StatusForm>,
) -> AppResult<Redirect> {
    let content = state.cache.get(&state.db, id).await?;
    if form.status == ExamStatus::Open && content.questions.is_empty() {
        admin
            .flash(
                "error",
                "Adăugați cel puțin o întrebare înainte de a deschide testul.",
            )
            .await?;
        return Ok(Redirect::to(&format!("/admin/teste/{id}")));
    }
    let mut tx = state.db.begin().await?;
    if !exams::set_status(&mut tx, id, form.status).await? {
        return Err(AppError::NotFound);
    }
    tx.commit().await?;
    state.cache.invalidate(id).await;
    let msg = match form.status {
        ExamStatus::Open => "Testul este deschis. Elevii îl pot începe acum.".to_string(),
        ExamStatus::Closed => {
            let n = finalizer::finalize_exam(&state, id).await?;
            format!("Testul a fost închis. Încercări finalizate acum: {n}.")
        }
        ExamStatus::Draft => "Testul a fost trecut în ciornă.".to_string(),
    };
    admin.flash("ok", msg).await?;
    Ok(Redirect::to(&format!("/admin/teste/{id}")))
}

pub async fn duplicate(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Redirect> {
    let dir = &state.config.upload_dir;
    let mut map = Vec::new();
    for old in exams::image_paths(&state.db, id).await? {
        if let Some(new) = uploads::copy(dir, &old).await {
            map.push((old, new));
        }
    }
    let new_id = exams::duplicate(&state.db, id, &map)
        .await?
        .ok_or(AppError::NotFound)?;
    admin
        .flash("ok", "Testul a fost duplicat (fără elevi și rezultate).")
        .await?;
    Ok(Redirect::to(&format!("/admin/teste/{new_id}")))
}

pub async fn delete(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Redirect> {
    let images = exams::delete(&state.db, id).await?;
    for img in images {
        uploads::remove(&state.config.upload_dir, &img).await;
    }
    state.cache.invalidate(id).await;
    admin.flash("ok", "Testul a fost șters.").await?;
    Ok(Redirect::to("/admin/teste"))
}

pub async fn regrade(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Redirect> {
    state.cache.invalidate(id).await;
    let n = finalizer::regrade_exam(&state, id).await?;
    admin
        .flash(
            "ok",
            format!("Notele au fost recalculate pentru {n} lucrări."),
        )
        .await?;
    Ok(Redirect::to(&format!("/admin/teste/{id}")))
}
