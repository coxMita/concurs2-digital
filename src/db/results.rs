//! Aggregated queries for the live dashboard and the results export.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

pub struct LiveRow {
    pub id: Uuid,
    pub full_name: Option<String>,
    pub username: String,
    pub started_at: Option<DateTime<Utc>>,
    pub deadline: Option<DateTime<Utc>>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub percentage: Option<f64>,
    pub login_count: Option<i32>,
    pub answered: i64,
}

/// One query for the whole class: students, attempts and answered counts.
pub async fn live(db: &PgPool, exam_id: Uuid) -> Result<Vec<LiveRow>, sqlx::Error> {
    sqlx::query_as!(
        LiveRow,
        r#"SELECT s.id, s.full_name, s.username,
                  a.started_at AS "started_at?", a.deadline AS "deadline?", a.submitted_at,
                  a.percentage, a.login_count AS "login_count?",
                  COALESCE(c.answered, 0) AS "answered!"
           FROM students s
           LEFT JOIN attempts a ON a.student_id = s.id
           LEFT JOIN (
               SELECT x.attempt_id, count(DISTINCT x.question_id) AS answered
               FROM answer_selections x
               JOIN attempts a2 ON a2.id = x.attempt_id
               WHERE a2.exam_id = $1
               GROUP BY x.attempt_id
           ) c ON c.attempt_id = a.id
           WHERE s.exam_id = $1
           ORDER BY COALESCE(s.full_name, s.username), s.username"#,
        exam_id
    )
    .fetch_all(db)
    .await
}

pub struct ExportStudent {
    pub full_name: Option<String>,
    pub username: String,
    pub attempt_id: Option<Uuid>,
    pub started_at: Option<DateTime<Utc>>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub earned_points: Option<i32>,
    pub max_points: Option<i32>,
    pub percentage: Option<f64>,
}

pub struct ExportData {
    pub students: Vec<ExportStudent>,
    /// attempt id -> question ids in the attempt
    pub attempt_questions: HashMap<Uuid, HashSet<Uuid>>,
    /// attempt id -> question id -> selected options
    pub selections: HashMap<Uuid, HashMap<Uuid, HashSet<Uuid>>>,
}

/// Everything needed for the results export: three queries regardless of class size.
pub async fn export_data(db: &PgPool, exam_id: Uuid) -> Result<ExportData, sqlx::Error> {
    let students = sqlx::query_as!(
        ExportStudent,
        r#"SELECT s.full_name, s.username, a.id AS "attempt_id?", a.started_at AS "started_at?",
                  a.submitted_at, a.earned_points, a.max_points AS "max_points?", a.percentage
           FROM students s LEFT JOIN attempts a ON a.student_id = s.id
           WHERE s.exam_id = $1
           ORDER BY COALESCE(s.full_name, s.username), s.username"#,
        exam_id
    )
    .fetch_all(db)
    .await?;
    let mut attempt_questions: HashMap<Uuid, HashSet<Uuid>> = HashMap::new();
    for r in sqlx::query!(
        "SELECT aq.attempt_id, aq.question_id FROM attempt_questions aq
         JOIN attempts a ON a.id = aq.attempt_id WHERE a.exam_id = $1",
        exam_id
    )
    .fetch_all(db)
    .await?
    {
        attempt_questions
            .entry(r.attempt_id)
            .or_default()
            .insert(r.question_id);
    }
    let mut selections: HashMap<Uuid, HashMap<Uuid, HashSet<Uuid>>> = HashMap::new();
    for r in sqlx::query!(
        "SELECT x.attempt_id, x.question_id, x.option_id FROM answer_selections x
         JOIN attempts a ON a.id = x.attempt_id WHERE a.exam_id = $1",
        exam_id
    )
    .fetch_all(db)
    .await?
    {
        selections
            .entry(r.attempt_id)
            .or_default()
            .entry(r.question_id)
            .or_default()
            .insert(r.option_id);
    }
    Ok(ExportData {
        students,
        attempt_questions,
        selections,
    })
}
