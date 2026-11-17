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
        attempts,
        exams::Exam,
        questions::AnswerType,
        results::{self, ExportData},
    },
    error::{AppError, AppResult},
    routes::admin::{exams::load, students::xlsx_response},
    services::{
        exam_cache::ExamContent,
        export::{QuestionInfo, ResultRow, labeled, results_xlsx},
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
    /// e.g. "3 (1 min 20 s)", empty if none.
    pub focus: String,
    /// Currently outside the page (in-progress attempts only), e.g. "2 min".
    pub away_now: String,
    pub copies: i32,
    pub screenshots: i32,
}

fn fmt_secs(secs: i64) -> String {
    let secs = secs.max(0);
    match (secs / 60, secs % 60) {
        (0, s) => format!("{s} s"),
        (m, 0) => format!("{m} min"),
        (m, s) => format!("{m} min {s} s"),
    }
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
                focus: match r.focus_losses.unwrap_or(0) {
                    0 => String::new(),
                    n => format!("{n} ({})", fmt_secs(r.focus_away_ms.unwrap_or(0) / 1000)),
                },
                away_now: match (r.away_since, r.submitted_at) {
                    (Some(t), None) => fmt_secs((now - t).num_seconds()),
                    _ => String::new(),
                },
                copies: r.copy_attempts.unwrap_or(0),
                screenshots: r.screenshot_attempts.unwrap_or(0),
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
            options: q
                .options
                .iter()
                .enumerate()
                .map(|(i, o)| labeled(i, &o.text, 200))
                .collect::<Vec<_>>()
                .join("\n"),
            correct: q
                .options
                .iter()
                .enumerate()
                .filter(|(_, o)| o.is_correct)
                .map(|(i, o)| labeled(i, &o.text, 200))
                .collect::<Vec<_>>()
                .join("; "),
        })
        .collect();
    let empty = Default::default();
    let rows = data
        .students
        .iter()
        .map(|s| {
            // Chosen options per question, labeled by canonical position.
            let answers: Vec<String> = match s.attempt_id {
                None => vec![String::new(); content.questions.len()],
                Some(aid) => {
                    let sels = data.selections.get(&aid).unwrap_or(&empty);
                    content
                        .questions
                        .iter()
                        .map(|q| {
                            let chosen = sels.get(&q.id);
                            q.options
                                .iter()
                                .enumerate()
                                .filter(|(_, o)| chosen.is_some_and(|c| c.contains(&o.id)))
                                .map(|(i, o)| labeled(i, &o.text, 40))
                                .collect::<Vec<_>>()
                                .join("; ")
                        })
                        .collect()
                }
            };
            let scores: Vec<Option<grading::QuestionScore>> = match s.attempt_id {
                None => vec![None; content.questions.len()],
                Some(aid) => {
                    let in_attempt = data.attempt_questions.get(&aid);
                    let sels = data.selections.get(&aid).unwrap_or(&empty);
                    content
                        .questions
                        .iter()
                        .map(|q| {
                            in_attempt.is_some_and(|set| set.contains(&q.id)).then(|| {
                                grading::question_score(q, sels.get(&q.id), content.exam.scoring)
                            })
                        })
                        .collect()
                }
            };
            let outcomes = scores.iter().map(|x| x.map(|x| x.outcome)).collect();
            let fractions = scores
                .iter()
                .map(|x| x.map_or(0.0, |x| x.fraction))
                .collect();
            let status = match (s.started_at, s.submitted_at) {
                (None, _) => "Neînceput",
                (Some(_), None) => "În desfășurare",
                (Some(_), Some(_)) => "Terminat",
            };
            let done = s.submitted_at.is_some();
            ResultRow {
                student_id: s.id,
                name: s.full_name.clone().unwrap_or_default(),
                username: s.username.clone(),
                percentage: if done { s.percentage } else { None },
                earned: if done { s.earned_points } else { None },
                max: if done { s.max_points } else { None },
                status,
                started_at: fmt_dt_opt(&s.started_at),
                submitted_at: fmt_dt_opt(&s.submitted_at),
                outcomes,
                fractions,
                answers,
                focus_losses: s.focus_losses,
                away_seconds: s.focus_away_ms.map(|ms| (ms + 500) / 1000),
                copy_attempts: s.copy_attempts,
                screenshot_attempts: s.screenshot_attempts,
            }
        })
        .collect();
    (questions, rows)
}

pub struct OptionStat {
    pub label: String,
    pub correct: bool,
    pub count: usize,
    pub pct: f64,
}

pub struct QuestionStat {
    pub n: usize,
    pub text: String,
    pub multiple: bool,
    pub correct: usize,
    pub partial: usize,
    pub wrong: usize,
    pub unanswered: usize,
    /// Share of answers that were fully correct.
    pub pct: f64,
    /// Students who had this question (denominator of the option percentages).
    pub answered_total: usize,
    pub options: Vec<OptionStat>,
}

/// One student in the results table.
pub struct StudentResult {
    pub id: Uuid,
    pub name: String,
    pub username: String,
    pub status: &'static str,
    pub status_class: &'static str,
    /// Sort keys (numbers; -1 when not available).
    pub pct_sort: f64,
    pub secs_sort: i64,
    pub pct: String,
    pub grade: String,
    pub points: String,
    pub duration: String,
    pub flags: String,
}

pub struct Bucket {
    pub label: String,
    pub count: usize,
}

pub struct Summary {
    pub submitted: usize,
    pub total: usize,
    pub average: String,
    pub average_grade: String,
    pub median: String,
    pub min: String,
    pub max: String,
}

#[derive(Template)]
#[template(path = "admin/results.html")]
struct ResultsPage {
    ctx: Ctx,
    exam: Exam,
    tab: &'static str,
    summary: Summary,
    buckets: Vec<Bucket>,
    bucket_max: usize,
    students: Vec<StudentResult>,
    stats: Vec<QuestionStat>,
}

fn pct_str(p: f64) -> String {
    format!("{p:.2}%").replace('.', ",")
}

fn grade_str(p: f64) -> String {
    format!("{:.2}", grading::grade_1_10(p)).replace('.', ",")
}

fn duration_str(secs: i64) -> String {
    let secs = secs.max(0);
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h} h {m:02} min")
    } else {
        format!("{m} min {s:02} s")
    }
}

fn median(sorted: &[f64]) -> f64 {
    match sorted.len() {
        0 => 0.0,
        n if n % 2 == 1 => sorted[n / 2],
        n => (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0,
    }
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

    // Summary and distribution of finished attempts.
    let mut pcts: Vec<f64> = rows.iter().filter_map(|r| r.percentage).collect();
    pcts.sort_by(f64::total_cmp);
    let summary = if pcts.is_empty() {
        Summary {
            submitted: 0,
            total: rows.len(),
            average: "—".into(),
            average_grade: "—".into(),
            median: "—".into(),
            min: "—".into(),
            max: "—".into(),
        }
    } else {
        let avg = pcts.iter().sum::<f64>() / pcts.len() as f64;
        Summary {
            submitted: pcts.len(),
            total: rows.len(),
            average: pct_str(avg),
            average_grade: grade_str(avg),
            median: pct_str(median(&pcts)),
            min: pct_str(pcts[0]),
            max: pct_str(pcts[pcts.len() - 1]),
        }
    };
    let mut counts = [0usize; 10];
    for p in &pcts {
        // 100% goes into the last bucket.
        let i = ((p / 10.0).floor() as usize).min(9);
        counts[i] += 1;
    }
    let buckets: Vec<Bucket> = counts
        .iter()
        .enumerate()
        .map(|(i, c)| Bucket {
            label: if i == 9 {
                "90–100%".into()
            } else {
                format!("{}–{}%", i * 10, i * 10 + 10)
            },
            count: *c,
        })
        .collect();
    let bucket_max = counts.iter().copied().max().unwrap_or(0).max(1);

    // Students table.
    // Time worked by students who submitted.
    let worked: std::collections::HashMap<Uuid, i64> = data
        .students
        .iter()
        .filter_map(|s| match (s.started_at, s.submitted_at) {
            (Some(a), Some(b)) => Some((s.id, (b - a).num_seconds())),
            _ => None,
        })
        .collect();
    let students = rows
        .iter()
        .map(|r| {
            let (status_class, status) = match r.status {
                "Terminat" => ("ok", "Terminat"),
                "În desfășurare" => ("progress", "În desfășurare"),
                _ => ("draft", "Neînceput"),
            };
            let secs = worked.get(&r.student_id).copied();
            let mut flags = Vec::new();
            if let Some(n) = r.focus_losses.filter(|n| *n > 0) {
                flags.push(format!("{n} ieșiri"));
            }
            if let Some(n) = r.copy_attempts.filter(|n| *n > 0) {
                flags.push(format!("{n} copieri"));
            }
            if let Some(n) = r.screenshot_attempts.filter(|n| *n > 0) {
                flags.push(format!("{n} capturi"));
            }
            StudentResult {
                id: r.student_id,
                name: r.name.clone(),
                username: r.username.clone(),
                status,
                status_class,
                pct_sort: r.percentage.unwrap_or(-1.0),
                secs_sort: secs.unwrap_or(-1),
                pct: r.percentage.map(pct_str).unwrap_or_default(),
                grade: r.percentage.map(grade_str).unwrap_or_default(),
                points: match (r.earned, r.max) {
                    (Some(e), Some(m)) => format!("{} / {m}", grading::fmt_points(e)),
                    _ => String::new(),
                },
                duration: secs.map(duration_str).unwrap_or_default(),
                flags: flags.join(", "),
            }
        })
        .collect();

    // Per-question statistics with option choices (who had the question at all).
    let empty = Default::default();
    let stats = content
        .questions
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let (mut c, mut p, mut w, mut u) = (0, 0, 0, 0);
            for r in &rows {
                match r.outcomes[i] {
                    Some(Outcome::Correct) => c += 1,
                    Some(Outcome::Partial) => p += 1,
                    Some(Outcome::Wrong) => w += 1,
                    Some(Outcome::Unanswered) => u += 1,
                    None => {}
                }
            }
            let total = c + p + w + u;
            let mut option_counts = vec![0usize; q.options.len()];
            for s in &data.students {
                let Some(aid) = s.attempt_id else { continue };
                if !data
                    .attempt_questions
                    .get(&aid)
                    .is_some_and(|set| set.contains(&q.id))
                {
                    continue;
                }
                let sels = data.selections.get(&aid).unwrap_or(&empty);
                if let Some(chosen) = sels.get(&q.id) {
                    for (j, o) in q.options.iter().enumerate() {
                        if chosen.contains(&o.id) {
                            option_counts[j] += 1;
                        }
                    }
                }
            }
            let share = |n: usize| {
                if total == 0 {
                    0.0
                } else {
                    (n as f64 / total as f64 * 1000.0).round() / 10.0
                }
            };
            QuestionStat {
                n: i + 1,
                text: q.text.clone(),
                multiple: q.answer_type == AnswerType::Multiple,
                correct: c,
                partial: p,
                wrong: w,
                unanswered: u,
                pct: share(c),
                answered_total: total,
                options: q
                    .options
                    .iter()
                    .enumerate()
                    .map(|(j, o)| OptionStat {
                        label: labeled(j, &o.text, 120),
                        correct: o.is_correct,
                        count: option_counts[j],
                        pct: share(option_counts[j]),
                    })
                    .collect(),
            }
        })
        .collect();

    render(&ResultsPage {
        ctx: admin.ctx("teste").await?,
        exam,
        tab: "rezultate",
        summary,
        buckets,
        bucket_max,
        students,
        stats,
    })
}

// ---- One student's work ------------------------------------------------------------

pub struct WorkOption {
    pub label: String,
    pub selected: bool,
    pub correct: bool,
}

pub struct WorkQuestion {
    pub n: usize,
    pub text: String,
    pub image: Option<String>,
    pub points: i32,
    pub earned: String,
    pub outcome: &'static str,
    pub outcome_class: &'static str,
    pub multiple: bool,
    pub options: Vec<WorkOption>,
}

#[derive(Template)]
#[template(path = "admin/student_work.html")]
struct StudentWorkPage {
    ctx: Ctx,
    exam: Exam,
    tab: &'static str,
    name: String,
    username: String,
    status: &'static str,
    pct: String,
    grade: String,
    points: String,
    started: String,
    submitted: String,
    duration: String,
    focus: String,
    copies: i32,
    screenshots: i32,
    relogins: i32,
    questions: Vec<WorkQuestion>,
}

/// Everything one student selected, question by question (canonical order, options
/// lettered A, B, C... as in the Excel export).
pub async fn student_work(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path((id, sid)): Path<(Uuid, Uuid)>,
) -> AppResult<Html<String>> {
    let exam = load(&state, id).await?;
    let content = state.cache.get(&state.db, id).await?;
    let st = attempts::student_state(&state.db, sid)
        .await?
        .filter(|s| s.exam_id == id)
        .ok_or(AppError::NotFound)?;
    let extra = results::attempt_extras(&state.db, sid).await?;
    let (sels, in_attempt) = match &st.attempt {
        Some(a) => (
            attempts::selections(&state.db, a.id).await?,
            attempts::order(&state.db, a.id)
                .await?
                .into_iter()
                .map(|(q, _)| q)
                .collect::<std::collections::HashSet<_>>(),
        ),
        None => (Default::default(), Default::default()),
    };
    let questions = content
        .questions
        .iter()
        .enumerate()
        .filter(|(_, q)| st.attempt.is_none() || in_attempt.contains(&q.id))
        .map(|(i, q)| {
            let chosen = sels.get(&q.id);
            let score = grading::question_score(q, chosen, content.exam.scoring);
            let (outcome, outcome_class) = match (&st.attempt, score.outcome) {
                (None, _) => ("Neînceput", "draft"),
                (_, Outcome::Correct) => ("Corect", "ok"),
                (_, Outcome::Partial) => ("Parțial", "warn"),
                (_, Outcome::Wrong) => ("Greșit", "danger"),
                (_, Outcome::Unanswered) => ("Fără răspuns", "draft"),
            };
            WorkQuestion {
                n: i + 1,
                text: q.text.clone(),
                image: q.image_path.clone(),
                points: q.points,
                earned: grading::fmt_points(f64::from(q.points) * score.fraction),
                outcome,
                outcome_class,
                multiple: q.answer_type == AnswerType::Multiple,
                options: q
                    .options
                    .iter()
                    .enumerate()
                    .map(|(j, o)| WorkOption {
                        label: labeled(j, &o.text, 2000),
                        selected: chosen.is_some_and(|c| c.contains(&o.id)),
                        correct: o.is_correct,
                    })
                    .collect(),
            }
        })
        .collect();
    let a = st.attempt.as_ref();
    let pct = a.and_then(|a| a.submitted_at.and(a.percentage));
    let secs = a.and_then(|a| a.submitted_at.map(|s| (s - a.started_at).num_seconds()));
    render(&StudentWorkPage {
        ctx: admin.ctx("teste").await?,
        exam,
        tab: "rezultate",
        name: st.full_name.clone().unwrap_or_else(|| "(fără nume)".into()),
        username: st.username.clone(),
        status: match a {
            None => "Neînceput",
            Some(a) if a.in_progress() => "În desfășurare",
            Some(_) => "Terminat",
        },
        pct: pct.map(pct_str).unwrap_or_else(|| "—".into()),
        grade: pct.map(grade_str).unwrap_or_else(|| "—".into()),
        points: match a {
            Some(a) if a.submitted_at.is_some() => format!(
                "{} / {}",
                grading::fmt_points(a.earned_points.unwrap_or(0.0)),
                a.max_points
            ),
            _ => "—".into(),
        },
        started: fmt_dt_opt(&a.map(|a| a.started_at)),
        submitted: fmt_dt_opt(&a.and_then(|a| a.submitted_at)),
        duration: secs.map(duration_str).unwrap_or_default(),
        focus: match extra.focus_losses {
            0 => "—".into(),
            n => format!(
                "{n} ({} în total)",
                duration_str(extra.focus_away_ms / 1000)
            ),
        },
        copies: extra.copy_attempts,
        screenshots: extra.screenshot_attempts,
        relogins: (extra.login_count - 1).max(0),
        questions,
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
