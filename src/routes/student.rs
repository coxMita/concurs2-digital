//! Student area: login, name prompt, waiting room, exam, autosave, submit, results.
//!
//! The browser never receives `is_correct` (or anything derived from it) before the
//! attempt is submitted; afterwards only if the exam shows answers.

use std::collections::{HashMap, HashSet};

use askama::Template;
use axum::{
    Form,
    extract::{Query, State},
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use chrono::Utc;
use serde::Deserialize;
use tower_sessions::Session;
use uuid::Uuid;

use crate::{
    auth::{CurrentStudent, STUDENT_KEY, STUDENT_SESSION_KEY},
    db::{
        attempts::{self, AttemptState},
        exams::{ExamStatus, ResultVisibility},
        questions::AnswerType,
    },
    error::{AppError, AppResult},
    routes::auth::{GENERIC_LOGIN_ERROR, LoginForm, RATE_LIMIT_ERROR},
    services::{
        credentials::normalize_username,
        exam_cache::ExamContent,
        finalizer,
        grading::{self, Outcome},
        shuffle::shuffle_attempt,
    },
    state::AppState,
    web::{Ctx, render},
};

// ---- Login -------------------------------------------------------------------

#[derive(Template)]
#[template(path = "student/login.html")]
struct LoginPage {
    ctx: Ctx,
    error: Option<&'static str>,
    username: String,
}

pub async fn login_page(session: Session) -> AppResult<Response> {
    if session.get::<Uuid>(STUDENT_KEY).await?.is_some() {
        return Ok(Redirect::to("/test").into_response());
    }
    let ctx = Ctx::build(&session, "student", None, "").await?;
    Ok(render(&LoginPage {
        ctx,
        error: None,
        username: String::new(),
    })?
    .into_response())
}

pub async fn login(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<LoginForm>,
) -> AppResult<Response> {
    let username = normalize_username(&form.username);
    let key = format!("student:{username}");
    let error = if state.login_limiter.is_blocked(&key) {
        RATE_LIMIT_ERROR
    } else {
        let creds = attempts::login_credentials(&state.db, &username).await?;
        let (id, hash) = match creds {
            Some((id, h)) => (Some(id), Some(h)),
            None => (None, None),
        };
        let ok = state.hasher.verify(form.password, hash).await?;
        if let (true, Some(id)) = (ok, id) {
            state.login_limiter.clear(&key);
            let sess = Uuid::new_v4();
            attempts::record_login(&state.db, id, sess).await?;
            session.cycle_id().await?;
            session.insert(STUDENT_KEY, id).await?;
            session.insert(STUDENT_SESSION_KEY, sess).await?;
            return Ok(Redirect::to("/test").into_response());
        }
        state.login_limiter.record_failure(&key);
        GENERIC_LOGIN_ERROR
    };
    let ctx = Ctx::build(&session, "student", None, "").await?;
    Ok((
        StatusCode::UNAUTHORIZED,
        render(&LoginPage {
            ctx,
            error: Some(error),
            username,
        })?,
    )
        .into_response())
}

// ---- Name prompt -------------------------------------------------------------

#[derive(Template)]
#[template(path = "student/name.html")]
struct NamePage {
    ctx: Ctx,
    error: Option<&'static str>,
}

pub async fn name_page(student: CurrentStudent) -> AppResult<Response> {
    if student.state.full_name.is_some() {
        return Ok(Redirect::to("/test").into_response());
    }
    Ok(render(&NamePage {
        ctx: student.ctx().await?,
        error: None,
    })?
    .into_response())
}

#[derive(Deserialize)]
pub struct NameForm {
    full_name: String,
}

pub async fn name_submit(
    State(state): State<AppState>,
    student: CurrentStudent,
    Form(form): Form<NameForm>,
) -> AppResult<Response> {
    let name = form
        .full_name
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if student.state.full_name.is_none() {
        if name.chars().count() < 3 || name.chars().count() > 120 || !name.contains(' ') {
            return Ok(render(&NamePage {
                ctx: student.ctx().await?,
                error: Some("Scrie numele complet: prenumele și numele de familie."),
            })?
            .into_response());
        }
        attempts::set_name(&state.db, student.state.id, &name).await?;
    }
    Ok(Redirect::to("/test").into_response())
}

// ---- Waiting room ------------------------------------------------------------

#[derive(Template)]
#[template(path = "student/waiting.html")]
struct WaitingPage {
    ctx: Ctx,
    title: String,
    description: String,
}

pub async fn waiting(
    State(state): State<AppState>,
    student: CurrentStudent,
) -> AppResult<Response> {
    if let Some(to) = next_step(&state, &student).await?
        && to != "/asteptare"
    {
        return Ok(Redirect::to(to).into_response());
    }
    let content = state.cache.get(&state.db, student.state.exam_id).await?;
    Ok(render(&WaitingPage {
        ctx: student.ctx().await?,
        title: content.exam.title.clone(),
        description: content.exam.description.clone(),
    })?
    .into_response())
}

/// Polled by HTMX every few seconds; redirects as soon as the exam opens.
pub async fn waiting_status(
    State(state): State<AppState>,
    student: CurrentStudent,
) -> AppResult<Response> {
    match next_step(&state, &student).await? {
        Some(to) if to != "/asteptare" => {
            let mut res = StatusCode::NO_CONTENT.into_response();
            res.headers_mut()
                .insert("HX-Redirect", HeaderValue::from_static(to));
            Ok(res)
        }
        _ => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

/// Where the student should be, if not on the exam page itself.
async fn next_step(state: &AppState, student: &CurrentStudent) -> AppResult<Option<&'static str>> {
    let s = &student.state;
    if s.full_name.is_none() {
        return Ok(Some("/nume"));
    }
    match &s.attempt {
        Some(a) if !a.in_progress() => Ok(Some("/rezultat")),
        Some(_) => Ok(Some("/test")),
        None => {
            let content = state.cache.get(&state.db, s.exam_id).await?;
            if content.exam.status == ExamStatus::Open && !content.questions.is_empty() {
                Ok(Some("/test"))
            } else {
                Ok(Some("/asteptare"))
            }
        }
    }
}

// ---- Exam --------------------------------------------------------------------

pub struct OptView {
    pub id: Uuid,
    pub text: String,
    pub selected: bool,
}

pub struct Pill {
    pub n: usize,
    pub answered: bool,
    pub current: bool,
}

#[derive(Template)]
#[template(path = "student/exam.html")]
struct ExamPage {
    ctx: Ctx,
    watermark: String,
    title: String,
    description: String,
    deadline: String,
    now: String,
    remaining: String,
    warning: bool,
    number: usize,
    total: usize,
    answered: usize,
    question_id: Uuid,
    text: String,
    image: Option<String>,
    points: i32,
    multiple: bool,
    options: Vec<OptView>,
    pills: Vec<Pill>,
    prev: Option<usize>,
    next: Option<usize>,
}

#[derive(Deserialize)]
pub struct QParam {
    q: Option<usize>,
}

fn fmt_remaining(secs: i64) -> String {
    let secs = secs.max(0);
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Ensures an in-progress attempt exists (creating it if the exam is open).
/// Returns Err(redirect) when the student belongs elsewhere.
async fn ensure_attempt(
    state: &AppState,
    student: &CurrentStudent,
    content: &ExamContent,
) -> AppResult<Result<AttemptState, &'static str>> {
    let s = &student.state;
    if s.full_name.is_none() {
        return Ok(Err("/nume"));
    }
    match &s.attempt {
        Some(a) if !a.in_progress() => Ok(Err("/rezultat")),
        Some(a) if a.expired(Utc::now()) => {
            finalizer::finalize_attempt(state, a.id).await?;
            Ok(Err("/rezultat"))
        }
        Some(a) => {
            // Back on the exam after leaving it (e.g. the tab was closed while away).
            if a.away_since.is_some() {
                attempts::record_event(&state.db, a.id, attempts::IntegrityEvent::Return).await?;
            }
            Ok(Ok(a.clone()))
        }
        None => {
            if content.exam.status != ExamStatus::Open || content.questions.is_empty() {
                return Ok(Err("/asteptare"));
            }
            let order = shuffle_attempt(&content.questions);
            let max: i32 = content.questions.iter().map(|q| q.points).sum();
            attempts::create(&state.db, s.id, s.exam_id, max, &order).await?;
            let fresh = attempts::student_state(&state.db, s.id)
                .await?
                .and_then(|st| st.attempt)
                .ok_or_else(|| AppError::Internal("attempt not created".into()))?;
            tracing::debug!(student = %s.username, "attempt started");
            Ok(Ok(fresh))
        }
    }
}

pub async fn exam(
    State(state): State<AppState>,
    student: CurrentStudent,
    Query(qp): Query<QParam>,
) -> AppResult<Response> {
    let content = state.cache.get(&state.db, student.state.exam_id).await?;
    let attempt = match ensure_attempt(&state, &student, &content).await? {
        Ok(a) => a,
        Err(to) => return Ok(Redirect::to(to).into_response()),
    };
    let order = attempts::order(&state.db, attempt.id).await?;
    let sels = attempts::selections(&state.db, attempt.id).await?;
    let order: Vec<_> = order
        .into_iter()
        .filter(|(qid, _)| content.question(*qid).is_some())
        .collect();
    let total = order.len();
    if total == 0 {
        return Err(AppError::NotFound);
    }
    let number = qp.q.unwrap_or(1).clamp(1, total);
    let (qid, opt_order) = &order[number - 1];
    let q = content.question(*qid).ok_or(AppError::NotFound)?;
    let selected = sels.get(qid);
    let options = opt_order
        .iter()
        .filter_map(|oid| q.options.iter().find(|o| o.id == *oid))
        .map(|o| OptView {
            id: o.id,
            text: o.text.clone(),
            selected: selected.is_some_and(|s| s.contains(&o.id)),
        })
        .collect();
    let answered_of = |id: &Uuid| sels.get(id).is_some_and(|s| !s.is_empty());
    let pills = order
        .iter()
        .enumerate()
        .map(|(i, (id, _))| Pill {
            n: i + 1,
            answered: answered_of(id),
            current: i + 1 == number,
        })
        .collect();
    let now = Utc::now();
    let secs = (attempt.deadline - now).num_seconds();
    Ok(render(&ExamPage {
        watermark: watermark(&student),
        ctx: student.ctx().await?,
        title: content.exam.title.clone(),
        description: content.exam.description.clone(),
        deadline: attempt.deadline.to_rfc3339(),
        now: now.to_rfc3339(),
        remaining: fmt_remaining(secs),
        warning: secs <= 300,
        number,
        total,
        answered: order.iter().filter(|(id, _)| answered_of(id)).count(),
        question_id: q.id,
        text: q.text.clone(),
        image: q.image_path.clone(),
        points: q.points,
        multiple: q.answer_type == AnswerType::Multiple,
        options,
        pills,
        prev: (number > 1).then(|| number - 1),
        next: (number < total).then(|| number + 1),
    })?
    .into_response())
}

/// Text of the faint watermark on exam pages, so screenshots or photos show whose
/// screen they came from.
fn watermark(student: &CurrentStudent) -> String {
    match &student.state.full_name {
        Some(n) => format!("{n} · {}", student.state.username),
        None => student.state.username.clone(),
    }
}

fn hx_redirect(to: &'static str) -> Response {
    let mut res = StatusCode::OK.into_response();
    res.headers_mut()
        .insert("HX-Redirect", HeaderValue::from_static(to));
    res
}

/// Autosave: replaces the selection of one question. Validates that the question
/// belongs to the student's exam/attempt, the options to the question, and that a
/// single-answer question receives at most one option. Time is enforced in SQL.
pub async fn save_answer(
    State(state): State<AppState>,
    student: CurrentStudent,
    Form(fields): Form<Vec<(String, String)>>,
) -> AppResult<Response> {
    let Some(attempt) = student.state.attempt.as_ref() else {
        return Ok(hx_redirect("/test"));
    };
    if !attempt.in_progress() {
        return Ok(hx_redirect("/rezultat"));
    }
    if attempt.expired(Utc::now()) {
        finalizer::finalize_attempt(&state, attempt.id).await?;
        return Ok(hx_redirect("/rezultat"));
    }
    let bad = || AppError::BadRequest("Răspuns invalid.".into());
    let mut qid = None;
    let mut opts: Vec<Uuid> = Vec::new();
    for (k, v) in &fields {
        match k.as_str() {
            "q" => qid = Some(Uuid::parse_str(v).map_err(|_| bad())?),
            "opt" => opts.push(Uuid::parse_str(v).map_err(|_| bad())?),
            _ => {}
        }
    }
    let qid = qid.ok_or_else(bad)?;
    let content = state.cache.get(&state.db, student.state.exam_id).await?;
    let q = content.question(qid).ok_or_else(bad)?;
    let unique: HashSet<Uuid> = opts.iter().copied().collect();
    if unique.len() != opts.len() || !unique.iter().all(|o| q.options.iter().any(|x| x.id == *o)) {
        return Err(bad());
    }
    if q.answer_type == AnswerType::Single && opts.len() > 1 {
        return Err(AppError::BadRequest(
            "Această întrebare acceptă un singur răspuns.".into(),
        ));
    }
    if attempts::save_answer(&state.db, attempt.id, qid, &opts).await? {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    // Not saved: either time ran out in the meantime or the question is not part of
    // this attempt.
    if Utc::now() >= attempt.deadline {
        finalizer::finalize_attempt(&state, attempt.id).await?;
        return Ok(hx_redirect("/rezultat"));
    }
    Err(AppError::Forbidden)
}

/// Integrity events from the exam page (leaving the page, blocked copy, PrintScreen).
/// Always answers 204: the page must not behave differently either way.
pub async fn integrity_event(
    State(state): State<AppState>,
    student: CurrentStudent,
    Form(fields): Form<Vec<(String, String)>>,
) -> AppResult<StatusCode> {
    let event = fields
        .iter()
        .find(|(k, _)| k == "kind")
        .and_then(|(_, v)| attempts::IntegrityEvent::parse(v));
    if let (Some(event), Some(a)) = (event, student.state.attempt.as_ref())
        && a.in_progress()
    {
        attempts::record_event(&state.db, a.id, event).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Template)]
#[template(path = "student/confirm.html")]
struct ConfirmPage {
    ctx: Ctx,
    watermark: String,
    title: String,
    deadline: String,
    now: String,
    remaining: String,
    warning: bool,
    total: usize,
    unanswered: Vec<usize>,
}

pub async fn confirm(
    State(state): State<AppState>,
    student: CurrentStudent,
) -> AppResult<Response> {
    let content = state.cache.get(&state.db, student.state.exam_id).await?;
    let attempt = match ensure_attempt(&state, &student, &content).await? {
        Ok(a) => a,
        Err(to) => return Ok(Redirect::to(to).into_response()),
    };
    let order = attempts::order(&state.db, attempt.id).await?;
    let sels = attempts::selections(&state.db, attempt.id).await?;
    let unanswered = order
        .iter()
        .enumerate()
        .filter(|(_, (id, _))| sels.get(id).is_none_or(|s| s.is_empty()))
        .map(|(i, _)| i + 1)
        .collect();
    let now = Utc::now();
    let secs = (attempt.deadline - now).num_seconds();
    Ok(render(&ConfirmPage {
        watermark: watermark(&student),
        ctx: student.ctx().await?,
        title: content.exam.title.clone(),
        deadline: attempt.deadline.to_rfc3339(),
        now: now.to_rfc3339(),
        remaining: fmt_remaining(secs),
        warning: secs <= 300,
        total: order.len(),
        unanswered,
    })?
    .into_response())
}

pub async fn submit(State(state): State<AppState>, student: CurrentStudent) -> AppResult<Redirect> {
    if let Some(a) = &student.state.attempt
        && a.in_progress()
    {
        finalizer::finalize_attempt(&state, a.id).await?;
        tracing::debug!(student = %student.state.username, "attempt submitted");
    }
    Ok(Redirect::to("/rezultat"))
}

// ---- Results -----------------------------------------------------------------

pub struct ReviewOpt {
    pub text: String,
    pub class: &'static str,
    pub tag: &'static str,
    pub selected: bool,
}

pub struct ReviewQ {
    pub n: usize,
    pub text: String,
    pub image: Option<String>,
    pub points: i32,
    pub outcome: String,
    pub outcome_class: &'static str,
    pub multiple: bool,
    pub options: Vec<ReviewOpt>,
}

#[derive(Template)]
#[template(path = "student/result.html")]
struct ResultPage {
    ctx: Ctx,
    title: String,
    visibility: &'static str,
    percentage: String,
    earned: String,
    max: i32,
    /// SVG stroke-dasharray for the score ring (no inline styles: CSP).
    ring: String,
    review: Vec<ReviewQ>,
}

pub async fn result(State(state): State<AppState>, student: CurrentStudent) -> AppResult<Response> {
    let mut attempt = match &student.state.attempt {
        None => return Ok(Redirect::to("/test").into_response()),
        Some(a) => a.clone(),
    };
    if attempt.in_progress() {
        if !attempt.expired(Utc::now()) {
            return Ok(Redirect::to("/test").into_response());
        }
        finalizer::finalize_attempt(&state, attempt.id).await?;
        attempt = attempts::student_state(&state.db, student.state.id)
            .await?
            .and_then(|s| s.attempt)
            .ok_or(AppError::NotFound)?;
    }
    let content = state.cache.get(&state.db, student.state.exam_id).await?;
    let visibility = content.exam.result_visibility;
    let pct = attempt.percentage.unwrap_or(0.0);
    const CIRC: f64 = 2.0 * std::f64::consts::PI * 70.0;
    let filled = CIRC * pct.clamp(0.0, 100.0) / 100.0;

    let review = if visibility == ResultVisibility::ScoreAndAnswers {
        let order = attempts::order(&state.db, attempt.id).await?;
        let sels = attempts::selections(&state.db, attempt.id).await?;
        build_review(&content, &order, &sels)
    } else {
        vec![]
    };
    Ok(render(&ResultPage {
        ctx: student.ctx().await?,
        title: content.exam.title.clone(),
        visibility: visibility.as_str(),
        percentage: format!("{pct:.2}").replace('.', ","),
        earned: grading::fmt_points(attempt.earned_points.unwrap_or(0.0)),
        max: attempt.max_points,
        ring: format!("{filled:.2} {CIRC:.2}"),
        review,
    })?
    .into_response())
}

fn build_review(
    content: &ExamContent,
    order: &[(Uuid, Vec<Uuid>)],
    sels: &HashMap<Uuid, HashSet<Uuid>>,
) -> Vec<ReviewQ> {
    order
        .iter()
        .filter_map(|(qid, opt_order)| content.question(*qid).map(|q| (q, opt_order)))
        .enumerate()
        .map(|(i, (q, opt_order))| {
            let selected = sels.get(&q.id);
            let score = grading::question_score(q, selected, content.exam.scoring);
            let options = opt_order
                .iter()
                .filter_map(|oid| q.options.iter().find(|o| o.id == *oid))
                .map(|o| {
                    let sel = selected.is_some_and(|s| s.contains(&o.id));
                    let (class, tag) = match (sel, o.is_correct) {
                        (true, true) => ("option--correct", "Răspunsul tău · corect"),
                        (true, false) => ("option--wrong", "Răspunsul tău · greșit"),
                        (false, true) => ("option--correct", "Răspuns corect"),
                        (false, false) => ("", ""),
                    };
                    ReviewOpt {
                        text: o.text.clone(),
                        class,
                        tag,
                        selected: sel,
                    }
                })
                .collect();
            let (label, class) = match score.outcome {
                Outcome::Correct => ("Corect".to_string(), "ok"),
                Outcome::Partial => (
                    format!(
                        "Parțial: {} din {} p",
                        grading::fmt_points(f64::from(q.points) * score.fraction),
                        q.points
                    ),
                    "warn",
                ),
                Outcome::Wrong => ("Greșit".to_string(), "danger"),
                Outcome::Unanswered => ("Fără răspuns".to_string(), "draft"),
            };
            ReviewQ {
                n: i + 1,
                text: q.text.clone(),
                image: q.image_path.clone(),
                points: q.points,
                outcome: label,
                outcome_class: class,
                multiple: q.answer_type == AnswerType::Multiple,
                options,
            }
        })
        .collect()
}
