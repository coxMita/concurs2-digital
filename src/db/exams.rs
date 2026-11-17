use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

#[derive(sqlx::Type, Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[sqlx(type_name = "exam_status", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum ExamStatus {
    Draft,
    Open,
    Closed,
}

impl ExamStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Draft => "Ciornă",
            Self::Open => "Deschis",
            Self::Closed => "Închis",
        }
    }
}

#[derive(sqlx::Type, Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[sqlx(type_name = "result_visibility", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ResultVisibility {
    None,
    Score,
    ScoreAndAnswers,
}

impl ResultVisibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Score => "score",
            Self::ScoreAndAnswers => "score_and_answers",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "Nimic",
            Self::Score => "Doar nota",
            Self::ScoreAndAnswers => "Nota + răspunsuri",
        }
    }
}

/// How multiple-answer questions are scored (see services::grading).
#[derive(sqlx::Type, Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[sqlx(type_name = "scoring_mode", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ScoringMode {
    AllOrNothing,
    Partial,
}

impl ScoringMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AllOrNothing => "all_or_nothing",
            Self::Partial => "partial",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::AllOrNothing => "Totul sau nimic",
            Self::Partial => "Punctaj parțial",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Exam {
    pub id: Uuid,
    pub title: String,
    pub description: String,
    pub duration_minutes: i32,
    pub status: ExamStatus,
    pub result_visibility: ResultVisibility,
    pub scoring: ScoringMode,
    pub created_at: DateTime<Utc>,
}

/// Exam row plus counters for the admin list.
pub struct ExamSummary {
    pub exam: Exam,
    pub questions: i64,
    pub students: i64,
    pub submitted: i64,
}

pub struct ExamInput<'a> {
    pub title: &'a str,
    pub description: &'a str,
    pub duration_minutes: i32,
    pub result_visibility: ResultVisibility,
    pub scoring: ScoringMode,
}

pub async fn get(db: &PgPool, id: Uuid) -> Result<Option<Exam>, sqlx::Error> {
    sqlx::query_as!(
        Exam,
        r#"SELECT id, title, description, duration_minutes,
                  status AS "status: ExamStatus",
                  result_visibility AS "result_visibility: ResultVisibility",
                  scoring AS "scoring: ScoringMode",
                  created_at
           FROM exams WHERE id = $1"#,
        id
    )
    .fetch_optional(db)
    .await
}

pub async fn list(db: &PgPool) -> Result<Vec<ExamSummary>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT e.id, e.title, e.description, e.duration_minutes,
                  e.status AS "status: ExamStatus",
                  e.result_visibility AS "result_visibility: ResultVisibility",
                  e.scoring AS "scoring: ScoringMode",
                  e.created_at,
                  (SELECT count(*) FROM questions q WHERE q.exam_id = e.id) AS "questions!",
                  (SELECT count(*) FROM students s WHERE s.exam_id = e.id) AS "students!",
                  (SELECT count(*) FROM attempts a WHERE a.exam_id = e.id
                     AND a.submitted_at IS NOT NULL) AS "submitted!"
           FROM exams e ORDER BY e.created_at DESC"#
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| ExamSummary {
            exam: Exam {
                id: r.id,
                title: r.title,
                description: r.description,
                duration_minutes: r.duration_minutes,
                status: r.status,
                result_visibility: r.result_visibility,
                scoring: r.scoring,
                created_at: r.created_at,
            },
            questions: r.questions,
            students: r.students,
            submitted: r.submitted,
        })
        .collect())
}

pub async fn create(db: &PgPool, input: &ExamInput<'_>) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar!(
        "INSERT INTO exams (title, description, duration_minutes, result_visibility, scoring)
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
        input.title,
        input.description,
        input.duration_minutes,
        input.result_visibility as ResultVisibility,
        input.scoring as ScoringMode,
    )
    .fetch_one(db)
    .await
}

pub async fn update(db: &PgPool, id: Uuid, input: &ExamInput<'_>) -> Result<bool, sqlx::Error> {
    let r = sqlx::query!(
        "UPDATE exams SET title = $2, description = $3, duration_minutes = $4,
                result_visibility = $5, scoring = $6, updated_at = now()
         WHERE id = $1",
        id,
        input.title,
        input.description,
        input.duration_minutes,
        input.result_visibility as ResultVisibility,
        input.scoring as ScoringMode,
    )
    .execute(db)
    .await?;
    Ok(r.rows_affected() == 1)
}

pub async fn set_status(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    status: ExamStatus,
) -> Result<bool, sqlx::Error> {
    let r = sqlx::query!(
        "UPDATE exams SET status = $2, updated_at = now() WHERE id = $1",
        id,
        status as ExamStatus
    )
    .execute(&mut **tx)
    .await?;
    Ok(r.rows_affected() == 1)
}

/// Deletes the exam; returns the image paths of its questions (to remove from disk).
pub async fn delete(db: &PgPool, id: Uuid) -> Result<Vec<String>, sqlx::Error> {
    let mut tx = db.begin().await?;
    let images = sqlx::query_scalar!(
        r#"SELECT image_path AS "p!" FROM questions WHERE exam_id = $1 AND image_path IS NOT NULL"#,
        id
    )
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query!("DELETE FROM exams WHERE id = $1", id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(images)
}

/// True if any student has started this exam (structure edits are then locked).
pub async fn has_attempts(db: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM attempts WHERE exam_id = $1) AS "e!""#,
        id
    )
    .fetch_one(db)
    .await
}

/// Copies an exam (as draft) with its questions and options. `copy_image` maps an
/// existing image path to a new one (files are copied so each exam owns its images).
pub async fn duplicate(
    db: &PgPool,
    id: Uuid,
    image_map: &[(String, String)],
) -> Result<Option<Uuid>, sqlx::Error> {
    let mut tx = db.begin().await?;
    let Some(new_id) = sqlx::query_scalar!(
        "INSERT INTO exams (title, description, duration_minutes, result_visibility, scoring)
         SELECT title || ' (copie)', description, duration_minutes, result_visibility, scoring
         FROM exams WHERE id = $1 RETURNING id",
        id
    )
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(None);
    };
    let (old_paths, new_paths): (Vec<String>, Vec<String>) = image_map.iter().cloned().unzip();
    // Map old question ids to new ones, then copy options.
    sqlx::query!(
        r#"WITH src AS (
               SELECT id, position, text, image_path, points, answer_type,
                      gen_random_uuid() AS new_id
               FROM questions WHERE exam_id = $1
           ), ins_q AS (
               INSERT INTO questions (id, exam_id, position, text, image_path, points, answer_type)
               SELECT s.new_id, $2, s.position, s.text,
                      (SELECT m.new FROM unnest($3::text[], $4::text[]) AS m(old, new)
                        WHERE m.old = s.image_path),
                      s.points, s.answer_type
               FROM src s
               RETURNING id
           )
           INSERT INTO options (question_id, position, text, is_correct)
           SELECT s.new_id, o.position, o.text, o.is_correct
           FROM src s JOIN options o ON o.question_id = s.id"#,
        id,
        new_id,
        &old_paths,
        &new_paths
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(new_id))
}

pub async fn image_paths(db: &PgPool, id: Uuid) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT image_path AS "p!" FROM questions WHERE exam_id = $1 AND image_path IS NOT NULL"#,
        id
    )
    .fetch_all(db)
    .await
}
