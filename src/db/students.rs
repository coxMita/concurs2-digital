use std::collections::HashSet;

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// Row of the admin students table.
pub struct StudentRow {
    pub id: Uuid,
    pub full_name: Option<String>,
    pub username: String,
    pub extra_minutes: i32,
    pub started_at: Option<DateTime<Utc>>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub percentage: Option<f64>,
}

impl StudentRow {
    pub fn status(&self) -> (&'static str, &'static str) {
        match (self.started_at, self.submitted_at) {
            (None, _) => ("draft", "Neînceput"),
            (Some(_), None) => ("progress", "În desfășurare"),
            (Some(_), Some(_)) => ("ok", "Terminat"),
        }
    }
}

pub async fn list(db: &PgPool, exam_id: Uuid) -> Result<Vec<StudentRow>, sqlx::Error> {
    sqlx::query_as!(
        StudentRow,
        r#"SELECT s.id, s.full_name, s.username, s.extra_minutes,
                  a.started_at AS "started_at?", a.submitted_at, a.percentage
           FROM students s LEFT JOIN attempts a ON a.student_id = s.id
           WHERE s.exam_id = $1
           ORDER BY s.created_at, s.username"#,
        exam_id
    )
    .fetch_all(db)
    .await
}

/// All usernames in use (students are unique across exams).
pub async fn all_usernames(db: &PgPool) -> Result<HashSet<String>, sqlx::Error> {
    Ok(sqlx::query_scalar!("SELECT username FROM students")
        .fetch_all(db)
        .await?
        .into_iter()
        .collect())
}

pub struct NewStudent {
    pub full_name: Option<String>,
    pub username: String,
    pub password_hash: String,
}

/// Bulk insert in one statement. Returns false if a username collided (retry).
pub async fn insert_many(
    db: &PgPool,
    exam_id: Uuid,
    rows: &[NewStudent],
) -> Result<bool, sqlx::Error> {
    let names: Vec<Option<String>> = rows.iter().map(|r| r.full_name.clone()).collect();
    let users: Vec<String> = rows.iter().map(|r| r.username.clone()).collect();
    let hashes: Vec<String> = rows.iter().map(|r| r.password_hash.clone()).collect();
    let r = sqlx::query!(
        "INSERT INTO students (exam_id, full_name, username, password_hash, created_at)
         SELECT $1, t.name, t.username, t.hash, now() + make_interval(secs => t.ord / 1000000.0)
         FROM unnest($2::text[], $3::text[], $4::text[]) WITH ORDINALITY AS t(name, username, hash, ord)
         ON CONFLICT (username) DO NOTHING",
        exam_id,
        &names as &[Option<String>],
        &users,
        &hashes
    )
    .execute(db)
    .await?;
    Ok(r.rows_affected() == rows.len() as u64)
}

pub async fn rename(
    db: &PgPool,
    exam_id: Uuid,
    id: Uuid,
    name: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let r = sqlx::query!(
        "UPDATE students SET full_name = $3 WHERE id = $1 AND exam_id = $2",
        id,
        exam_id,
        name
    )
    .execute(db)
    .await?;
    Ok(r.rows_affected() == 1)
}

/// Sets a new password and logs the student out everywhere. Returns the username.
pub async fn set_password(
    db: &PgPool,
    exam_id: Uuid,
    id: Uuid,
    hash: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar!(
        "UPDATE students SET password_hash = $3, active_session = NULL
         WHERE id = $1 AND exam_id = $2 RETURNING username",
        id,
        exam_id,
        hash
    )
    .fetch_optional(db)
    .await
}

/// Sets extra minutes and moves the deadline of an in-progress attempt accordingly.
pub async fn set_extra_minutes(
    db: &PgPool,
    exam_id: Uuid,
    id: Uuid,
    minutes: i32,
) -> Result<bool, sqlx::Error> {
    let mut tx = db.begin().await?;
    let r = sqlx::query!(
        "UPDATE students SET extra_minutes = $3 WHERE id = $1 AND exam_id = $2",
        id,
        exam_id,
        minutes
    )
    .execute(&mut *tx)
    .await?;
    if r.rows_affected() == 0 {
        return Ok(false);
    }
    sqlx::query!(
        "UPDATE attempts a
         SET deadline = a.started_at + make_interval(mins => e.duration_minutes + $2)
         FROM exams e
         WHERE a.student_id = $1 AND a.submitted_at IS NULL AND e.id = a.exam_id",
        id,
        minutes
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

pub async fn delete_many(db: &PgPool, exam_id: Uuid, ids: &[Uuid]) -> Result<u64, sqlx::Error> {
    let r = sqlx::query!(
        "DELETE FROM students WHERE exam_id = $1 AND id = ANY($2)",
        exam_id,
        ids
    )
    .execute(db)
    .await?;
    Ok(r.rows_affected())
}

/// Deletes the student's attempt (and its answers) so they can take the exam again.
pub async fn allow_retake(db: &PgPool, exam_id: Uuid, id: Uuid) -> Result<bool, sqlx::Error> {
    let mut tx = db.begin().await?;
    let r = sqlx::query!(
        "DELETE FROM attempts a USING students s
         WHERE a.student_id = s.id AND s.id = $1 AND s.exam_id = $2",
        id,
        exam_id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(r.rows_affected() == 1)
}
