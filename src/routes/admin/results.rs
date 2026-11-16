//! Live dashboard (HTMX polling) and results export.

use askama::Template;
use axum::{
    extract::{Path, State},
    response::{Html, Response},
};
use chrono::Utc;
use uuid::Uuid;

use crate::{
    auth::CurrentAdmin,
    db::{
        exams::Exam,
        questions::AnswerType,
        results::{self, ExportData},
    },
    error::{AppError, AppResult},
    routes::admin::{exams::load, students::xlsx_response},
    services::{
        exam_cache::ExamContent,
        export::{QuestionInfo, ResultRow, results_xlsx},
        grading::{self, Outcome},
    },
    state::AppState,
    web::{Ctx, fmt_dt_opt, render, slug},
};

// ---- Live dashboard ------------------------------------------------------------

pub struct LiveView {
    pub id: Uuid,
    pub name: String,
    pub username: String,
    pub status_class: &'static str,
    pub status: &'static str,
    pub answered: i64,
    pub time_left: String,
    pub score: String,
    pub relogins: i32,
}

pub struct Counters {
    pub total: usize,
    pub not_started: usize,
    pub in_progress: usize,
    pub finished: usize,
    pub average: String,
}

#[derive(Template)]
#[template(path = "admin/live.html")]
struct LivePage {
    ctx: Ctx,
    exam: Exam,
    tab: &'static str,
}

#[derive(Template)]
#[template(path = "admin/live_table.html")]
struct LiveTable {
    exam: Exam,
    questions: usize,
    counters: Counters,
    rows: Vec<LiveView>,
    updated: String,
}

pub async fn live(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Html<String>> {
    let exam = load(&state, id).await?;
    render(&LivePage {
        ctx: admin.ctx("teste").await?,
        exam,
        tab: "live",
    })
}

/// Polled every 5s: one aggregated query for the whole class.
pub async fn live_table(
    State(state): State<AppState>,
    _admin: CurrentAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Html<String>> {
    let content = state.cache.get(&state.db, id).await?;
    let rows = results::live(&state.db, id).await?;
    let now = Utc::now();
    let mut counters = Counters {
        total: rows.len(),
        not_started: 0,
        in_progress: 0,
        finished: 0,
        average: String::from("—"),
    };
    let mut sum = 0.0;
    let views = rows
        .into_iter()
        .map(|r| {
            let (status_class, status, time_left) = match (r.started_at, r.submitted_at, r.deadline)
            {
                (None, _, _) => {
                    counters.not_started += 1;
                    ("draft", "Neînceput", String::new())
                }
                (Some(_), Some(_), _) => {
                    counters.finished += 1;
                    sum += r.percentage.unwrap_or(0.0);
                    ("ok", "Terminat", String::new())
                }
                (Some(_), None, Some(d)) if d <= now => {
                    counters.in_progress += 1;
                    ("warn", "Timp expirat", "0:00".into())
                }
                (Some(_), None, d) => {
                    counters.in_progress += 1;
                    let secs = d.map_or(0, |d| (d - now).num_seconds()).max(0);
                    (
                        "progress",
                        "În desfășurare",
                        format!("{}:{:02}", secs / 60, secs % 60),
                    )
                }
            };
            LiveView {
                id: r.id,
                name: r.full_name.unwrap_or_default(),
                username: r.username,
                status_class,
                status,
                answered: r.answered,
                time_left,
                score: if r.submitted_at.is_some() {
                    format!("{:.2}%", r.percentage.unwrap_or(0.0))
                } else {
                    String::new()
                },
                relogins: r.login_count.unwrap_or(1) - 1,
            }
        })
        .collect();
    if counters.finished > 0 {
        counters.average = format!("{:.2}%", sum / counters.finished as f64);
    }
    render(&LiveTable {
        questions: content.questions.len(),
        exam: content.exam.clone(),
        counters,
        rows: views,
        updated: now
            .with_timezone(&chrono_tz::Europe::Bucharest)
            .format("%H:%M:%S")
            .to_string(),
    })
}

// ---- Results ---------------------------------------------------------------------

pub struct QuestionStat {
    pub n: usize,
    pub text: String,
    pub correct: usize,
    pub wrong: usize,
    pub unanswered: usize,
    pub pct: f64,
}

#[derive(Template)]
#[template(path = "admin/results.html")]
struct ResultsPage {
    ctx: Ctx,
    exam: Exam,
    tab: &'static str,
    submitted: usize,
    total: usize,
    average: String,
    stats: Vec<QuestionStat>,
}

/// Builds the rows (canonical question order) shared by the page and the Excel file.
pub fn build_rows(content: &ExamContent, data: &ExportData) -> (Vec<QuestionInfo>, Vec<ResultRow>) {
    let questions = content
        .questions
        .iter()
        .map(|q| QuestionInfo {
            text: q.text.clone(),
            answer_type: match q.answer_type {
                AnswerType::Single => "Un singur răspuns corect",
                AnswerType::Multiple => "Mai multe răspunsuri corecte",
            },
            points: q.points,
            correct: q
                .options
                .iter()
                .filter(|o| o.is_correct)
                .map(|o| o.text.as_str())
                .collect::<Vec<_>>()
                .join("; "),
        })
        .collect();
    let empty = Default::default();
    let rows = data
        .students
        .iter()
        .map(|s| {
            let outcomes = match s.attempt_id {
                None => vec![None; content.questions.len()],
                Some(aid) => {
                    let in_attempt = data.attempt_questions.get(&aid);
                    let sels = data.selections.get(&aid).unwrap_or(&empty);
                    content
                        .questions
                        .iter()
                        .map(|q| {
                            in_attempt
                                .is_some_and(|set| set.contains(&q.id))
                                .then(|| grading::question_outcome(q, sels.get(&q.id)))
                        })
                        .collect()
                }
            };
            let status = match (s.started_at, s.submitted_at) {
                (None, _) => "Neînceput",
                (Some(_), None) => "În desfășurare",
                (Some(_), Some(_)) => "Terminat",
            };
            let done = s.submitted_at.is_some();
            ResultRow {
                name: s.full_name.clone().unwrap_or_default(),
                username: s.username.clone(),
                percentage: if done { s.percentage } else { None },
                earned: if done { s.earned_points } else { None },
                max: if done { s.max_points } else { None },
                status,
                started_at: fmt_dt_opt(&s.started_at),
                submitted_at: fmt_dt_opt(&s.submitted_at),
                outcomes,
            }
        })
        .collect();
    (questions, rows)
}

pub async fn results_page(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Html<String>> {
    let exam = load(&state, id).await?;
    let content = state.cache.get(&state.db, id).await?;
    let data = results::export_data(&state.db, id).await?;
    let (_, rows) = build_rows(&content, &data);
    let finished: Vec<&ResultRow> = rows.iter().filter(|r| r.percentage.is_some()).collect();
    let average = if finished.is_empty() {
        "—".to_string()
    } else {
        let sum: f64 = finished.iter().filter_map(|r| r.percentage).sum();
        format!("{:.2}%", sum / finished.len() as f64)
    };
    let stats = content
        .questions
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let (mut c, mut w, mut u) = (0, 0, 0);
            for r in &rows {
                match r.outcomes[i] {
                    Some(Outcome::Correct) => c += 1,
                    Some(Outcome::Wrong) => w += 1,
                    Some(Outcome::Unanswered) => u += 1,
                    None => {}
                }
            }
            let total = c + w + u;
            QuestionStat {
                n: i + 1,
                text: q.text.clone(),
                correct: c,
                wrong: w,
                unanswered: u,
                pct: if total == 0 {
                    0.0
                } else {
                    (c as f64 / total as f64 * 1000.0).round() / 10.0
                },
            }
        })
        .collect();
    render(&ResultsPage {
        ctx: admin.ctx("teste").await?,
        exam,
        tab: "rezultate",
        submitted: finished.len(),
        total: rows.len(),
        average,
        stats,
    })
}

pub async fn results_xlsx_download(
    State(state): State<AppState>,
    _admin: CurrentAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Response> {
    let content = state.cache.get(&state.db, id).await?;
    let data = results::export_data(&state.db, id).await?;
    let (questions, rows) = build_rows(&content, &data);
    let bytes = tokio::task::spawn_blocking(move || results_xlsx(&questions, &rows))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(xlsx_response(
        bytes,
        &format!("rezultate-{}.xlsx", slug(&content.exam.title)),
    ))
}
