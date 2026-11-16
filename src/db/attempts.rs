//! Student-side queries: login, attempt creation, answers. Hot paths use a fixed,
//! small number of queries.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct AttemptState {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub deadline: DateTime<Utc>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub earned_points: Option<i32>,
    pub max_points: i32,
    pub percentage: Option<f64>,
}

impl AttemptState {
    pub fn in_progress(&self) -> bool {
        self.submitted_at.is_none()
    }
    pub fn expired(&self, now: DateTime<Utc>) -> bool {
        self.submitted_at.is_none() && now >= self.deadline
    }
}

/// The logged-in student's row joined with their attempt (one query).
pub struct StudentState {
    pub id: Uuid,
    pub exam_id: Uuid,
    pub full_name: Option<String>,
    pub username: String,
    pub active_session: Option<Uuid>,
    pub attempt: Option<AttemptState>,
}

pub async fn student_state(db: &PgPool, id: Uuid) -> Result<Option<StudentState>, sqlx::Error> {
    let r = sqlx::query!(
        r#"SELECT s.id, s.exam_id, s.full_name, s.username, s.active_session,
                  a.id AS "attempt_id?", a.started_at AS "started_at?", a.deadline AS "deadline?",
                  a.submitted_at, a.earned_points, a.max_points AS "max_points?", a.percentage
           FROM students s LEFT JOIN attempts a ON a.student_id = s.id
           WHERE s.id = $1"#,
        id
    )
    .fetch_optional(db)
    .await?;
    Ok(r.map(|r| StudentState {
        id: r.id,
        exam_id: r.exam_id,
        full_name: r.full_name,
        username: r.username,
        active_session: r.active_session,
        attempt: match (r.attempt_id, r.started_at, r.deadline) {
            (Some(id), Some(started_at), Some(deadline)) => Some(AttemptState {
                id,
                started_at,
                deadline,
                submitted_at: r.submitted_at,
                earned_points: r.earned_points,
                max_points: r.max_points.unwrap_or(0),
                percentage: r.percentage,
            }),
            _ => None,
        },
    }))
}

pub async fn login_credentials(
    db: &PgPool,
    username: &str,
) -> Result<Option<(Uuid, String)>, sqlx::Error> {
    Ok(sqlx::query!(
        "SELECT id, password_hash FROM students WHERE username = $1",
        username
    )
    .fetch_optional(db)
    .await?
    .map(|r| (r.id, r.password_hash)))
}

/// Records a successful login: new active session id (older sessions become invalid)
/// and, if an attempt is in progress, increments its login counter.
pub async fn record_login(db: &PgPool, id: Uuid, session: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "WITH s AS (UPDATE students SET active_session = $2 WHERE id = $1 RETURNING id)
         UPDATE attempts SET login_count = login_count + 1
         WHERE student_id = (SELECT id FROM s) AND submitted_at IS NULL",
        id,
        session
    )
    .execute(db)
    .await?;
    Ok(())
}

pub async fn set_name(db: &PgPool, id: Uuid, name: &str) -> Result<(), sqlx::Error> {
    sqlx::query!("UPDATE students SET full_name = $2 WHERE id = $1", id, name)
        .execute(db)
        .await?;
    Ok(())
}

/// Creates the attempt with its shuffled order in one transaction.
/// `order`: (question id, option order) in display order. Returns false if an attempt
/// already existed (concurrent request): the caller reloads.
pub async fn create(
    db: &PgPool,
    student_id: Uuid,
    exam_id: Uuid,
    max_points: i32,
    order: &[(Uuid, Vec<Uuid>)],
) -> Result<bool, sqlx::Error> {
    let mut tx = db.begin().await?;
    let id = sqlx::query_scalar!(
        "INSERT INTO attempts (student_id, exam_id, started_at, deadline, max_points)
         SELECT s.id, s.exam_id, now(),
                now() + make_interval(mins => e.duration_minutes + s.extra_minutes), $3
         FROM students s JOIN exams e ON e.id = s.exam_id
         WHERE s.id = $1 AND s.exam_id = $2
         ON CONFLICT (student_id) DO NOTHING
         RETURNING id",
        student_id,
        exam_id,
        max_points
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(id) = id else {
        return Ok(false);
    };
    let qids: Vec<Uuid> = order.iter().map(|o| o.0).collect();
    // uuid[][] cannot be unnested row-wise, so pass each option order as a text literal.
    let orders: Vec<String> = order
        .iter()
        .map(|o| {
            let inner: Vec<String> = o.1.iter().map(Uuid::to_string).collect();
            format!("{{{}}}", inner.join(","))
        })
        .collect();
    sqlx::query!(
        "INSERT INTO attempt_questions (attempt_id, question_id, display_position, option_order)
         SELECT $1, t.qid, t.ord::int, t.opts::uuid[]
         FROM unnest($2::uuid[], $3::text[]) WITH ORDINALITY AS t(qid, opts, ord)",
        id,
        &qids,
        &orders
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// Display order of the attempt: (question id, option order).
pub async fn order(db: &PgPool, attempt_id: Uuid) -> Result<Vec<(Uuid, Vec<Uuid>)>, sqlx::Error> {
    Ok(sqlx::query!(
        "SELECT question_id, option_order FROM attempt_questions
         WHERE attempt_id = $1 ORDER BY display_position",
        attempt_id
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .map(|r| (r.question_id, r.option_order))
    .collect())
}

/// Selected options per question.
pub async fn selections(
    db: &PgPool,
    attempt_id: Uuid,
) -> Result<HashMap<Uuid, HashSet<Uuid>>, sqlx::Error> {
    let mut out: HashMap<Uuid, HashSet<Uuid>> = HashMap::new();
    for r in sqlx::query!(
        "SELECT question_id, option_id FROM answer_selections WHERE attempt_id = $1",
        attempt_id
    )
    .fetch_all(db)
    .await?
    {
        out.entry(r.question_id).or_default().insert(r.option_id);
    }
    Ok(out)
}

/// Replaces the selection for one question in a single statement. The write only
/// happens if the attempt is still open (not submitted, before the deadline, row
/// share-locked against a concurrent finalization) and the question belongs to it.
/// Returns false if nothing was allowed.
pub async fn save_answer(
    db: &PgPool,
    attempt_id: Uuid,
    question_id: Uuid,
    options: &[Uuid],
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar!(
        r#"WITH ok AS (
               SELECT a.id FROM attempts a
               WHERE a.id = $1 AND a.submitted_at IS NULL AND a.deadline > now()
                 AND EXISTS (SELECT 1 FROM attempt_questions aq
                             WHERE aq.attempt_id = $1 AND aq.question_id = $2)
               FOR SHARE OF a
           ), del AS (
               DELETE FROM answer_selections
               WHERE attempt_id = $1 AND question_id = $2 AND option_id <> ALL($3)
                 AND EXISTS (SELECT 1 FROM ok)
           ), ins AS (
               INSERT INTO answer_selections (attempt_id, question_id, option_id)
               SELECT $1, $2, o FROM unnest($3::uuid[]) AS o
               WHERE EXISTS (SELECT 1 FROM ok)
               ON CONFLICT DO NOTHING
           )
           SELECT EXISTS (SELECT 1 FROM ok) AS "ok!""#,
        attempt_id,
        question_id,
        options
    )
    .fetch_one(db)
    .await
}
