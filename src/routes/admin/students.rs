//! Student accounts per exam: generation, credentials export, management.

use askama::Template;
use axum::{
    Form,
    extract::{Path, State},
    http::header,
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use tokio::task::JoinSet;
use uuid::Uuid;

use crate::{
    auth::CurrentAdmin,
    db::{
        exams::Exam,
        students::{self, NewStudent, StudentRow},
    },
    error::{AppError, AppResult},
    routes::admin::exams::load,
    services::{
        credentials::{generate_password, plan_usernames},
        export::{Credential, credentials_xlsx},
    },
    state::AppState,
    web::{Ctx, render, slug},
};

const MAX_PER_BATCH: usize = 1000;

#[derive(Template)]
#[template(path = "admin/students.html")]
struct StudentsPage {
    ctx: Ctx,
    exam: Exam,
    tab: &'static str,
    students: Vec<StudentRow>,
}

impl StudentsPage {
    fn pct(&self, s: &StudentRow) -> String {
        s.percentage.map(|p| format!("{p:.2}%")).unwrap_or_default()
    }
}

pub async fn list(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Html<String>> {
    let exam = load(&state, id).await?;
    let students = students::list(&state.db, id).await?;
    render(&StudentsPage {
        ctx: admin.ctx("teste").await?,
        exam,
        tab: "elevi",
        students,
    })
}

#[derive(Template)]
#[template(path = "admin/credentials.html")]
struct CredentialsPage {
    ctx: Ctx,
    exam: Exam,
    tab: &'static str,
    creds: Vec<Credential>,
}

#[derive(Deserialize)]
pub struct GenerateForm {
    #[serde(default)]
    count: String,
    #[serde(default)]
    names: String,
}

pub async fn generate(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path(id): Path<Uuid>,
    Form(form): Form<GenerateForm>,
) -> AppResult<Response> {
    let exam = load(&state, id).await?;
    let back = format!("/admin/teste/{id}/elevi");
    let names: Vec<String> = form
        .names
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .map(|l| l.chars().take(120).collect())
        .collect();
    let count = if form.count.trim().is_empty() {
        0
    } else {
        match form.count.trim().parse::<usize>() {
            Ok(n) => n,
            Err(_) => {
                admin
                    .flash(
                        "error",
                        "Numărul de conturi trebuie să fie un număr întreg.",
                    )
                    .await?;
                return Ok(Redirect::to(&back).into_response());
            }
        }
    };
    let total = names.len() + count;
    if total == 0 || total > MAX_PER_BATCH {
        admin
            .flash(
                "error",
                format!("Introduceți un număr de conturi și/sau o listă de nume (maximum {MAX_PER_BATCH} odată)."),
            )
            .await?;
        return Ok(Redirect::to(&back).into_response());
    }

    // Retry in the unlikely case another admin generated the same usernames concurrently.
    for _ in 0..3 {
        let mut taken = students::all_usernames(&state.db).await?;
        let plan = plan_usernames(&names, count, &mut taken);
        let passwords: Vec<String> = plan.iter().map(|_| generate_password()).collect();

        // Hash in parallel; the hasher's semaphore bounds CPU use.
        let mut set = JoinSet::new();
        for (i, pw) in passwords.iter().enumerate() {
            let hasher = state.hasher.clone();
            let pw = pw.clone();
            set.spawn(async move { (i, hasher.hash(pw).await) });
        }
        let mut hashes = vec![String::new(); plan.len()];
        while let Some(res) = set.join_next().await {
            let (i, h) = res.map_err(|e| AppError::Internal(e.to_string()))?;
            hashes[i] = h?;
        }

        let rows: Vec<NewStudent> = plan
            .iter()
            .zip(hashes)
            .map(|((name, username), hash)| NewStudent {
                full_name: name.clone(),
                username: username.clone(),
                password_hash: hash,
            })
            .collect();
        if students::insert_many(&state.db, id, &rows).await? {
            let creds = plan
                .into_iter()
                .zip(passwords)
                .map(|((name, username), password)| Credential {
                    name: name.unwrap_or_default(),
                    username,
                    password,
                })
                .collect();
            tracing::info!(exam = %id, count = total, "generated student accounts");
            return Ok(render(&CredentialsPage {
                ctx: admin.ctx("teste").await?,
                exam,
                tab: "elevi",
                creds,
            })?
            .into_response());
        }
        // Collision: remove partial inserts of this batch and retry.
        let users: Vec<String> = rows.iter().map(|r| r.username.clone()).collect();
        sqlx::query!(
            "DELETE FROM students WHERE exam_id = $1 AND username = ANY($2)
             AND NOT EXISTS (SELECT 1 FROM attempts a WHERE a.student_id = students.id)
             AND created_at > now() - interval '1 minute'",
            id,
            &users
        )
        .execute(&state.db)
        .await?;
    }
    Err(AppError::Internal("could not allocate usernames".into()))
}

/// Excel with the credentials just shown. The plaintext passwords are posted back by
/// the admin's browser (they are never stored on the server).
pub async fn credentials_download(
    State(state): State<AppState>,
    _admin: CurrentAdmin,
    Path(id): Path<Uuid>,
    Form(fields): Form<Vec<(String, String)>>,
) -> AppResult<Response> {
    let exam = load(&state, id).await?;
    let mut rows: Vec<Credential> = Vec::new();
    for (k, v) in fields {
        match k.as_str() {
            "n" => rows.push(Credential {
                name: v,
                username: String::new(),
                password: String::new(),
            }),
            "u" => {
                if let Some(r) = rows.last_mut() {
                    r.username = v;
                }
            }
            "p" => {
                if let Some(r) = rows.last_mut() {
                    r.password = v;
                }
            }
            _ => {}
        }
    }
    if rows.is_empty() || rows.len() > MAX_PER_BATCH {
        return Err(AppError::BadRequest("Nicio parolă de exportat.".into()));
    }
    let bytes =
        credentials_xlsx(&exam.title, &rows).map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(xlsx_response(
        bytes,
        &format!("conturi-{}.xlsx", slug(&exam.title)),
    ))
}

pub fn xlsx_response(bytes: Vec<u8>, filename: &str) -> Response {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_string(),
            ),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        bytes,
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct NameForm {
    #[serde(default)]
    full_name: String,
}

pub async fn rename(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path((id, sid)): Path<(Uuid, Uuid)>,
    Form(form): Form<NameForm>,
) -> AppResult<Redirect> {
    let name = form
        .full_name
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let name: String = name.chars().take(120).collect();
    let name = (!name.is_empty()).then_some(name.as_str());
    if students::rename(&state.db, id, sid, name).await? {
        admin.flash("ok", "Numele a fost salvat.").await?;
    }
    Ok(Redirect::to(&format!("/admin/teste/{id}/elevi#s-{sid}")))
}

pub async fn reset_password(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path((id, sid)): Path<(Uuid, Uuid)>,
) -> AppResult<Redirect> {
    let pw = generate_password();
    let hash = state.hasher.hash(pw.clone()).await?;
    match students::set_password(&state.db, id, sid, &hash).await? {
        Some(username) => admin
            .flash(
                "ok",
                format!(
                    "Parola nouă pentru „{username}”: {pw} (notați-o acum: nu va mai fi afișată)."
                ),
            )
            .await?,
        None => return Err(AppError::NotFound),
    }
    Ok(Redirect::to(&format!("/admin/teste/{id}/elevi#s-{sid}")))
}

#[derive(Deserialize)]
pub struct ExtraForm {
    extra_minutes: String,
}

pub async fn extra_time(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path((id, sid)): Path<(Uuid, Uuid)>,
    Form(form): Form<ExtraForm>,
) -> AppResult<Redirect> {
    match form.extra_minutes.trim().parse::<i32>() {
        Ok(m) if (0..=600).contains(&m) => {
            if students::set_extra_minutes(&state.db, id, sid, m).await? {
                admin
                    .flash("ok", format!("Timp suplimentar: {m} minute."))
                    .await?;
            }
        }
        _ => {
            admin
                .flash(
                    "error",
                    "Timpul suplimentar trebuie să fie între 0 și 600 de minute.",
                )
                .await?
        }
    }
    Ok(Redirect::to(&format!("/admin/teste/{id}/elevi#s-{sid}")))
}

pub async fn retake(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path((id, sid)): Path<(Uuid, Uuid)>,
) -> AppResult<Redirect> {
    if students::allow_retake(&state.db, id, sid).await? {
        admin
            .flash(
                "ok",
                "Lucrarea a fost ștearsă. Elevul poate susține din nou testul.",
            )
            .await?;
    }
    Ok(Redirect::to(&format!("/admin/teste/{id}/elevi#s-{sid}")))
}

pub async fn delete_many(
    State(state): State<AppState>,
    admin: CurrentAdmin,
    Path(id): Path<Uuid>,
    Form(fields): Form<Vec<(String, String)>>,
) -> AppResult<Redirect> {
    let ids: Vec<Uuid> = fields
        .into_iter()
        .filter(|(k, _)| k == "ids")
        .filter_map(|(_, v)| Uuid::parse_str(&v).ok())
        .collect();
    if ids.is_empty() {
        admin.flash("error", "Nu ați selectat niciun elev.").await?;
    } else {
        let n = students::delete_many(&state.db, id, &ids).await?;
        admin
            .flash("ok", format!("Au fost șterse {n} conturi."))
            .await?;
    }
    Ok(Redirect::to(&format!("/admin/teste/{id}/elevi")))
}
