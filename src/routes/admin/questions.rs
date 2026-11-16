//! Questions editor: list, create/edit (multipart with optional image), delete,
//! duplicate, reorder, bulk import/export.

use std::collections::BTreeMap;

use askama::Template;
use axum::{
    Form,
    extract::{Multipart, Path, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    auth::CurrentAdmin,
    db::{
        exams::{self, Exam},
        questions::{self, AnswerType, OptionInput, Question, QuestionInput, UpdateOutcome},
    },
    error::{AppError, AppResult},
    routes::admin::exams::load,
    services::{import_format, uploads},
    state::AppState,
    web::{Ctx, render},
};

const LOCKED_MSG: &str = "Elevii au început deja testul: nu se mai pot adăuga sau șterge întrebări ori variante. Puteți corecta textul, punctajul și răspunsurile corecte, apoi folosiți „Recalculează notele”.";

#[derive(Template)]
#[template(path = "admin/questions.html")]
struct QuestionsPage {
    ctx: Ctx,
    exam: Exam,
    tab: &'static str,
    questions: Vec<Question>,
    locked: bool,
    locked_msg: &'static str,
    total_points: i32,
}

pub async fn list(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Html<String>> {
    let exam = load(&state, id).await?;
    let questions = questions::list_full(&state.db, id).await?;
    let locked = exams::has_attempts(&state.db, id).await?;
    render(&QuestionsPage {
        ctx: admin.ctx("teste").await?,
        total_points: questions.iter().map(|q| q.points).sum(),
        exam,
        tab: "intrebari",
        questions,
        locked,
        locked_msg: LOCKED_MSG,
    })
}

/// One editable option row in the form.
pub struct OptionView {
    pub id: String,
    pub text: String,
    pub correct: bool,
}

#[derive(Template)]
#[template(path = "admin/question_form.html")]
struct QuestionFormPage {
    ctx: Ctx,
    exam: Exam,
    tab: &'static str,
    /// None when creating.
    question_id: Option<Uuid>,
    number: i32,
    text: String,
    points: i32,
    answer_type: &'static str,
    image: Option<String>,
    options: Vec<OptionView>,
    error: Option<String>,
    locked: bool,
    locked_msg: &'static str,
    max_mb: usize,
}

fn default_options() -> Vec<OptionView> {
    (0..4)
        .map(|_| OptionView {
            id: String::new(),
            text: String::new(),
            correct: false,
        })
        .collect()
}

pub async fn new_page(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Response> {
    let exam = load(&state, id).await?;
    if exams::has_attempts(&state.db, id).await? {
        admin.flash("error", LOCKED_MSG).await?;
        return Ok(Redirect::to(&format!("/admin/teste/{id}/intrebari")).into_response());
    }
    Ok(render(&QuestionFormPage {
        ctx: admin.ctx("teste").await?,
        exam,
        tab: "intrebari",
        question_id: None,
        number: 0,
        text: String::new(),
        points: 1,
        answer_type: "single",
        image: None,
        options: default_options(),
        error: None,
        locked: false,
        locked_msg: LOCKED_MSG,
        max_mb: state.config.max_upload_bytes / (1024 * 1024),
    })?
    .into_response())
}

pub async fn edit_page(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path((id, qid)): Path<(Uuid, Uuid)>,
) -> AppResult<Html<String>> {
    let exam = load(&state, id).await?;
    let q = questions::get_full(&state.db, id, qid)
        .await?
        .ok_or(AppError::NotFound)?;
    let locked = exams::has_attempts(&state.db, id).await?;
    render(&QuestionFormPage {
        ctx: admin.ctx("teste").await?,
        exam,
        tab: "intrebari",
        question_id: Some(q.id),
        number: q.position,
        text: q.text,
        points: q.points,
        answer_type: q.answer_type.as_str(),
        image: q.image_path,
        options: q
            .options
            .into_iter()
            .map(|o| OptionView {
                id: o.id.to_string(),
                text: o.text,
                correct: o.is_correct,
            })
            .collect(),
        error: None,
        locked,
        locked_msg: LOCKED_MSG,
        max_mb: state.config.max_upload_bytes / (1024 * 1024),
    })
}

/// Parsed multipart question form.
struct QuestionSubmission {
    input: QuestionInput,
    raw_options: Vec<OptionView>,
    image: Option<Vec<u8>>,
    remove_image: bool,
}

#[derive(Default)]
struct RawOption {
    id: String,
    text: String,
    correct: bool,
}

async fn parse_question_form(mut mp: Multipart) -> AppResult<(QuestionSubmission, Option<String>)> {
    let bad = |e: axum::extract::multipart::MultipartError| {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            AppError::BadRequest("Fișierul încărcat este prea mare.".into())
        } else {
            AppError::BadRequest("Formular invalid.".into())
        }
    };
    let mut text = String::new();
    let mut points_raw = String::from("1");
    let mut answer_type = AnswerType::Single;
    let mut image = None;
    let mut remove_image = false;
    let mut opts: BTreeMap<u32, RawOption> = BTreeMap::new();

    while let Some(field) = mp.next_field().await.map_err(bad)? {
        let name = field.name().unwrap_or_default().to_string();
        if name == "image" {
            let bytes = field.bytes().await.map_err(bad)?;
            if !bytes.is_empty() {
                image = Some(bytes.to_vec());
            }
            continue;
        }
        let value = field.text().await.map_err(bad)?;
        match name.as_str() {
            "text" => text = value,
            "points" => points_raw = value,
            "answer_type" => {
                answer_type = if value == "multiple" {
                    AnswerType::Multiple
                } else {
                    AnswerType::Single
                }
            }
            "remove_image" => remove_image = value == "1",
            _ => {
                let Some((kind, idx)) = name.rsplit_once('_') else {
                    continue;
                };
                let Ok(idx) = idx.parse::<u32>() else {
                    continue;
                };
                if idx > 63 {
                    continue;
                }
                let o = opts.entry(idx).or_default();
                match kind {
                    "opt_id" => o.id = value,
                    "opt_text" => o.text = value,
                    "opt_correct" => o.correct = true,
                    _ => {}
                }
            }
        }
    }

    let raw_options: Vec<OptionView> = opts
        .into_values()
        .filter(|o| !o.text.trim().is_empty() || !o.id.is_empty())
        .map(|o| OptionView {
            id: o.id,
            text: o.text,
            correct: o.correct,
        })
        .collect();
    let mut error = None;
    let points = match points_raw.trim().parse::<i32>() {
        Ok(p) => p,
        Err(_) => {
            error = Some("Punctajul trebuie să fie un număr întreg.".to_string());
            1
        }
    };
    let input = QuestionInput {
        text,
        points,
        answer_type,
        options: raw_options
            .iter()
            .map(|o| OptionInput {
                id: Uuid::parse_str(&o.id).ok(),
                text: o.text.clone(),
                is_correct: o.correct,
            })
            .collect(),
    };
    if error.is_none() {
        error = input.validate().err();
    }
    Ok((
        QuestionSubmission {
            input,
            raw_options,
            image,
            remove_image,
        },
        error,
    ))
}

#[allow(clippy::too_many_arguments)]
async fn rerender(
    state: &AppState,
    admin: &CurrentAdmin,
    exam: Exam,
    question_id: Option<Uuid>,
    number: i32,
    sub: QuestionSubmission,
    image: Option<String>,
    error: String,
    locked: bool,
) -> AppResult<Response> {
    let mut options = sub.raw_options;
    if options.is_empty() {
        options = default_options();
    }
    Ok((
        StatusCode::UNPROCESSABLE_ENTITY,
        render(&QuestionFormPage {
            ctx: admin.ctx("teste").await?,
            exam,
            tab: "intrebari",
            question_id,
            number,
            text: sub.input.text,
            points: sub.input.points,
            answer_type: sub.input.answer_type.as_str(),
            image,
            options,
            error: Some(error),
            locked,
            locked_msg: LOCKED_MSG,
            max_mb: state.config.max_upload_bytes / (1024 * 1024),
        })?,
    )
        .into_response())
}

pub async fn create(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path(id): Path<Uuid>,
    mp: Multipart,
) -> AppResult<Response> {
    let exam = load(&state, id).await?;
    if exams::has_attempts(&state.db, id).await? {
        admin.flash("error", LOCKED_MSG).await?;
        return Ok(Redirect::to(&format!("/admin/teste/{id}/intrebari")).into_response());
    }
    let (mut sub, error) = parse_question_form(mp).await?;
    if let Some(e) = error {
        return rerender(&state, &admin, exam, None, 0, sub, None, e, false).await;
    }
    let image = match sub.image.take() {
        Some(bytes) => {
            match uploads::save(
                &state.config.upload_dir,
                &bytes,
                state.config.max_upload_bytes,
            )
            .await
            {
                Ok(name) => Some(name),
                Err(e) => {
                    return rerender(
                        &state,
                        &admin,
                        exam,
                        None,
                        0,
                        sub,
                        None,
                        e.to_string(),
                        false,
                    )
                    .await;
                }
            }
        }
        None => None,
    };
    for o in &mut sub.input.options {
        o.id = None;
    }
    questions::insert_many(&state.db, id, &[(sub.input, image)]).await?;
    state.cache.invalidate(id).await;
    admin.flash("ok", "Întrebarea a fost adăugată.").await?;
    Ok(Redirect::to(&format!("/admin/teste/{id}/intrebari")).into_response())
}

pub async fn update(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path((id, qid)): Path<(Uuid, Uuid)>,
    mp: Multipart,
) -> AppResult<Response> {
    let exam = load(&state, id).await?;
    let current = questions::get_full(&state.db, id, qid)
        .await?
        .ok_or(AppError::NotFound)?;
    let locked = exams::has_attempts(&state.db, id).await?;
    let (mut sub, error) = parse_question_form(mp).await?;
    let number = current.position;
    if let Some(e) = error {
        return rerender(
            &state,
            &admin,
            exam,
            Some(qid),
            number,
            sub,
            current.image_path,
            e,
            locked,
        )
        .await;
    }
    let new_image = match sub.image.take() {
        Some(bytes) => {
            match uploads::save(
                &state.config.upload_dir,
                &bytes,
                state.config.max_upload_bytes,
            )
            .await
            {
                Ok(name) => Some(Some(name)),
                Err(e) => {
                    return rerender(
                        &state,
                        &admin,
                        exam,
                        Some(qid),
                        number,
                        sub,
                        current.image_path,
                        e.to_string(),
                        locked,
                    )
                    .await;
                }
            }
        }
        None if sub.remove_image => Some(None),
        None => None,
    };
    let outcome =
        questions::update(&state.db, id, qid, &sub.input, new_image.clone(), locked).await?;
    match outcome {
        UpdateOutcome::NotFound => Err(AppError::NotFound),
        UpdateOutcome::Locked => {
            if let Some(Some(name)) = &new_image {
                uploads::remove(&state.config.upload_dir, name).await;
            }
            rerender(
                &state,
                &admin,
                exam,
                Some(qid),
                number,
                sub,
                current.image_path,
                LOCKED_MSG.into(),
                locked,
            )
            .await
        }
        UpdateOutcome::Updated => {
            if let (Some(_), Some(old)) = (&new_image, &current.image_path) {
                uploads::remove(&state.config.upload_dir, old).await;
            }
            state.cache.invalidate(id).await;
            let msg = if locked {
                "Întrebarea a fost salvată. Dacă ați schimbat răspunsurile corecte sau punctajul, folosiți „Recalculează notele” din Setări."
            } else {
                "Întrebarea a fost salvată."
            };
            admin.flash("ok", msg).await?;
            Ok(Redirect::to(&format!("/admin/teste/{id}/intrebari#q-{qid}")).into_response())
        }
    }
}

pub async fn delete(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path((id, qid)): Path<(Uuid, Uuid)>,
) -> AppResult<Redirect> {
    if exams::has_attempts(&state.db, id).await? {
        admin.flash("error", LOCKED_MSG).await?;
    } else if let Some(image) = questions::delete(&state.db, id, qid).await? {
        if let Some(img) = image {
            uploads::remove(&state.config.upload_dir, &img).await;
        }
        state.cache.invalidate(id).await;
        admin.flash("ok", "Întrebarea a fost ștearsă.").await?;
    }
    Ok(Redirect::to(&format!("/admin/teste/{id}/intrebari")))
}

pub async fn duplicate(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path((id, qid)): Path<(Uuid, Uuid)>,
) -> AppResult<Redirect> {
    if exams::has_attempts(&state.db, id).await? {
        admin.flash("error", LOCKED_MSG).await?;
        return Ok(Redirect::to(&format!("/admin/teste/{id}/intrebari")));
    }
    let q = questions::get_full(&state.db, id, qid)
        .await?
        .ok_or(AppError::NotFound)?;
    let image = match &q.image_path {
        Some(p) => uploads::copy(&state.config.upload_dir, p).await,
        None => None,
    };
    let new_id = questions::insert_after(&state.db, id, q.position, &q.to_input(), image).await?;
    state.cache.invalidate(id).await;
    admin.flash("ok", "Întrebarea a fost duplicată.").await?;
    Ok(Redirect::to(&format!(
        "/admin/teste/{id}/intrebari#q-{new_id}"
    )))
}

pub async fn move_up(
    State(state): State<AppState>,
    _admin: CurrentAdmin,
    Path((id, qid)): Path<(Uuid, Uuid)>,
) -> AppResult<Redirect> {
    questions::move_by(&state.db, id, qid, -1).await?;
    state.cache.invalidate(id).await;
    Ok(Redirect::to(&format!(
        "/admin/teste/{id}/intrebari#q-{qid}"
    )))
}

pub async fn move_down(
    State(state): State<AppState>,
    _admin: CurrentAdmin,
    Path((id, qid)): Path<(Uuid, Uuid)>,
) -> AppResult<Redirect> {
    questions::move_by(&state.db, id, qid, 1).await?;
    state.cache.invalidate(id).await;
    Ok(Redirect::to(&format!(
        "/admin/teste/{id}/intrebari#q-{qid}"
    )))
}

#[derive(Deserialize)]
pub struct ReorderForm {
    /// Comma-separated question ids in the new order.
    ids: String,
}

/// Drag-and-drop reorder (called with fetch from admin.js).
pub async fn reorder(
    State(state): State<AppState>,
    _admin: CurrentAdmin,
    Path(id): Path<Uuid>,
    Form(form): Form<ReorderForm>,
) -> AppResult<StatusCode> {
    let ids: Result<Vec<Uuid>, _> = form
        .ids
        .split(',')
        .filter(|s| !s.is_empty())
        .map(Uuid::parse_str)
        .collect();
    let ids = ids.map_err(|_| AppError::BadRequest("Ordine invalidă.".into()))?;
    if questions::reorder(&state.db, id, &ids).await? {
        state.cache.invalidate(id).await;
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::BadRequest(
            "Lista de întrebări s-a schimbat. Reîncărcați pagina.".into(),
        ))
    }
}

// ---- Bulk import / export ------------------------------------------------

pub struct PreviewOption {
    pub text: String,
    pub correct: bool,
}

pub struct PreviewQuestion {
    pub text: String,
    pub points: i32,
    pub type_label: &'static str,
    pub options: Vec<PreviewOption>,
}

#[derive(Template)]
#[template(path = "admin/import.html")]
struct ImportPage {
    ctx: Ctx,
    exam: Exam,
    tab: &'static str,
    text: String,
    example: &'static str,
    errors: Vec<import_format::ParseError>,
    preview: Vec<PreviewQuestion>,
    locked: bool,
    locked_msg: &'static str,
}

pub async fn import_page(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Html<String>> {
    let exam = load(&state, id).await?;
    let locked = exams::has_attempts(&state.db, id).await?;
    render(&ImportPage {
        ctx: admin.ctx("teste").await?,
        exam,
        tab: "intrebari",
        text: String::new(),
        example: import_format::EXAMPLE,
        errors: vec![],
        preview: vec![],
        locked,
        locked_msg: LOCKED_MSG,
    })
}

#[derive(Deserialize)]
pub struct ImportForm {
    text: String,
    /// "preview" or "save"
    action: String,
}

pub async fn import_submit(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path(id): Path<Uuid>,
    Form(form): Form<ImportForm>,
) -> AppResult<Response> {
    let exam = load(&state, id).await?;
    let locked = exams::has_attempts(&state.db, id).await?;
    let parsed = import_format::parse(&form.text);
    if let (Ok(qs), "save", false) = (&parsed, form.action.as_str(), locked) {
        let items: Vec<(QuestionInput, Option<String>)> =
            qs.iter().cloned().map(|q| (q, None)).collect();
        questions::insert_many(&state.db, id, &items).await?;
        state.cache.invalidate(id).await;
        admin
            .flash(
                "ok",
                format!("Au fost importate {} întrebări.", items.len()),
            )
            .await?;
        return Ok(Redirect::to(&format!("/admin/teste/{id}/intrebari")).into_response());
    }
    let (preview, errors) = match parsed {
        Ok(qs) => (
            qs.into_iter()
                .map(|q| PreviewQuestion {
                    text: q.text,
                    points: q.points,
                    type_label: q.answer_type.short_label(),
                    options: q
                        .options
                        .into_iter()
                        .map(|o| PreviewOption {
                            text: o.text,
                            correct: o.is_correct,
                        })
                        .collect(),
                })
                .collect(),
            vec![],
        ),
        Err(errors) => (vec![], errors),
    };
    Ok(render(&ImportPage {
        ctx: admin.ctx("teste").await?,
        exam,
        tab: "intrebari",
        text: form.text,
        example: import_format::EXAMPLE,
        errors,
        preview,
        locked,
        locked_msg: LOCKED_MSG,
    })?
    .into_response())
}

pub async fn export(
    State(state): State<AppState>,
    _admin: CurrentAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Response> {
    let exam = load(&state, id).await?;
    let qs = questions::list_full(&state.db, id).await?;
    let body = import_format::export(&qs);
    let filename = format!("intrebari-{}.txt", crate::web::slug(&exam.title));
    Ok((
        [
            (
                header::CONTENT_TYPE,
                "text/plain; charset=utf-8".to_string(),
            ),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        body,
    )
        .into_response())
}
