use std::collections::HashMap;

use serde::Deserialize;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

#[derive(sqlx::Type, Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[sqlx(type_name = "answer_type", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum AnswerType {
    Single,
    Multiple,
}

impl AnswerType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Multiple => "multiple",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Single => "Un singur răspuns corect",
            Self::Multiple => "Mai multe răspunsuri corecte",
        }
    }
    pub fn short_label(self) -> &'static str {
        match self {
            Self::Single => "Un răspuns",
            Self::Multiple => "Mai multe răspunsuri",
        }
    }
}

#[derive(Clone, Debug)]
pub struct OptionRow {
    pub id: Uuid,
    pub text: String,
    pub is_correct: bool,
}

#[derive(Clone, Debug)]
pub struct Question {
    pub id: Uuid,
    pub exam_id: Uuid,
    pub position: i32,
    pub text: String,
    pub image_path: Option<String>,
    pub points: i32,
    pub answer_type: AnswerType,
    pub options: Vec<OptionRow>,
}

#[derive(Clone, Debug)]
pub struct OptionInput {
    pub id: Option<Uuid>,
    pub text: String,
    pub is_correct: bool,
}

#[derive(Clone, Debug)]
pub struct QuestionInput {
    pub text: String,
    pub points: i32,
    pub answer_type: AnswerType,
    pub options: Vec<OptionInput>,
}

impl QuestionInput {
    /// Validates the question; returns a Romanian error message.
    pub fn validate(&self) -> Result<(), String> {
        if self.text.trim().is_empty() {
            return Err("Textul întrebării este obligatoriu.".into());
        }
        if self.text.len() > 10_000 {
            return Err("Textul întrebării este prea lung.".into());
        }
        if !(0..=1000).contains(&self.points) {
            return Err("Punctajul trebuie să fie între 0 și 1000.".into());
        }
        if !(2..=8).contains(&self.options.len()) {
            return Err("O întrebare trebuie să aibă între 2 și 8 variante de răspuns.".into());
        }
        if self.options.iter().any(|o| o.text.trim().is_empty()) {
            return Err("Toate variantele de răspuns trebuie completate.".into());
        }
        if self.options.iter().any(|o| o.text.len() > 2_000) {
            return Err("O variantă de răspuns este prea lungă.".into());
        }
        let correct = self.options.iter().filter(|o| o.is_correct).count();
        match self.answer_type {
            AnswerType::Single if correct != 1 => Err(
                "O întrebare cu un singur răspuns corect trebuie să aibă exact o variantă corectă."
                    .into(),
            ),
            AnswerType::Multiple if correct == 0 => {
                Err("Marcați cel puțin o variantă corectă.".into())
            }
            _ => Ok(()),
        }
    }
}

/// All questions of an exam (canonical order) with their options: two queries.
pub async fn list_full(db: &PgPool, exam_id: Uuid) -> Result<Vec<Question>, sqlx::Error> {
    let qs = sqlx::query!(
        r#"SELECT id, exam_id, position, text, image_path, points,
                  answer_type AS "answer_type: AnswerType"
           FROM questions WHERE exam_id = $1 ORDER BY position, id"#,
        exam_id
    )
    .fetch_all(db)
    .await?;
    let opts = sqlx::query!(
        r#"SELECT o.id, o.question_id, o.text, o.is_correct
           FROM options o JOIN questions q ON q.id = o.question_id
           WHERE q.exam_id = $1 ORDER BY o.question_id, o.position, o.id"#,
        exam_id
    )
    .fetch_all(db)
    .await?;
    let mut by_q: HashMap<Uuid, Vec<OptionRow>> = HashMap::new();
    for o in opts {
        by_q.entry(o.question_id).or_default().push(OptionRow {
            id: o.id,
            text: o.text,
            is_correct: o.is_correct,
        });
    }
    Ok(qs
        .into_iter()
        .map(|q| Question {
            options: by_q.remove(&q.id).unwrap_or_default(),
            id: q.id,
            exam_id: q.exam_id,
            position: q.position,
            text: q.text,
            image_path: q.image_path,
            points: q.points,
            answer_type: q.answer_type,
        })
        .collect())
}

pub async fn get_full(
    db: &PgPool,
    exam_id: Uuid,
    id: Uuid,
) -> Result<Option<Question>, sqlx::Error> {
    let Some(q) = sqlx::query!(
        r#"SELECT id, exam_id, position, text, image_path, points,
                  answer_type AS "answer_type: AnswerType"
           FROM questions WHERE id = $1 AND exam_id = $2"#,
        id,
        exam_id
    )
    .fetch_optional(db)
    .await?
    else {
        return Ok(None);
    };
    let options = sqlx::query_as!(
        OptionRow,
        "SELECT id, text, is_correct FROM options WHERE question_id = $1 ORDER BY position, id",
        id
    )
    .fetch_all(db)
    .await?;
    Ok(Some(Question {
        id: q.id,
        exam_id: q.exam_id,
        position: q.position,
        text: q.text,
        image_path: q.image_path,
        points: q.points,
        answer_type: q.answer_type,
        options,
    }))
}

async fn insert_options(
    tx: &mut Transaction<'_, Postgres>,
    question_id: Uuid,
    options: &[OptionInput],
) -> Result<(), sqlx::Error> {
    let texts: Vec<String> = options.iter().map(|o| o.text.trim().to_string()).collect();
    let correct: Vec<bool> = options.iter().map(|o| o.is_correct).collect();
    sqlx::query!(
        "INSERT INTO options (question_id, position, text, is_correct)
         SELECT $1, t.ord::int, t.text, t.correct
         FROM unnest($2::text[], $3::bool[]) WITH ORDINALITY AS t(text, correct, ord)",
        question_id,
        &texts,
        &correct
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Inserts several questions at the end of the exam (used by create and import).
pub async fn insert_many(
    db: &PgPool,
    exam_id: Uuid,
    items: &[(QuestionInput, Option<String>)],
) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut tx = db.begin().await?;
    // Lock the exam row so concurrent inserts get distinct positions.
    sqlx::query!("SELECT id FROM exams WHERE id = $1 FOR UPDATE", exam_id)
        .fetch_one(&mut *tx)
        .await?;
    let mut next = sqlx::query_scalar!(
        r#"SELECT COALESCE(MAX(position), 0) AS "m!" FROM questions WHERE exam_id = $1"#,
        exam_id
    )
    .fetch_one(&mut *tx)
    .await?;
    let mut ids = Vec::with_capacity(items.len());
    for (q, image) in items {
        next += 1;
        let id = sqlx::query_scalar!(
            "INSERT INTO questions (exam_id, position, text, image_path, points, answer_type)
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
            exam_id,
            next,
            q.text.trim_end(),
            image.as_deref(),
            q.points,
            q.answer_type as AnswerType,
        )
        .fetch_one(&mut *tx)
        .await?;
        insert_options(&mut tx, id, &q.options).await?;
        ids.push(id);
    }
    touch_exam(&mut tx, exam_id).await?;
    tx.commit().await?;
    Ok(ids)
}

async fn touch_exam(tx: &mut Transaction<'_, Postgres>, exam_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!("UPDATE exams SET updated_at = now() WHERE id = $1", exam_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub enum UpdateOutcome {
    Updated,
    NotFound,
    /// Options were added/removed while attempts exist.
    Locked,
}

/// Updates text, points, type and options. `new_image`: None = keep,
/// Some(None) = remove, Some(Some(path)) = replace.
pub async fn update(
    db: &PgPool,
    exam_id: Uuid,
    id: Uuid,
    input: &QuestionInput,
    new_image: Option<Option<String>>,
    locked: bool,
) -> Result<UpdateOutcome, sqlx::Error> {
    let mut tx = db.begin().await?;
    let existing: Vec<Uuid> = sqlx::query_scalar!(
        "SELECT o.id FROM options o JOIN questions q ON q.id = o.question_id
         WHERE o.question_id = $1 AND q.exam_id = $2 FOR UPDATE OF o",
        id,
        exam_id
    )
    .fetch_all(&mut *tx)
    .await?;
    let r = match &new_image {
        None => {
            sqlx::query!(
                "UPDATE questions SET text = $3, points = $4, answer_type = $5
                 WHERE id = $1 AND exam_id = $2",
                id,
                exam_id,
                input.text.trim_end(),
                input.points,
                input.answer_type as AnswerType,
            )
            .execute(&mut *tx)
            .await?
        }
        Some(img) => {
            sqlx::query!(
                "UPDATE questions SET text = $3, points = $4, answer_type = $5, image_path = $6
                 WHERE id = $1 AND exam_id = $2",
                id,
                exam_id,
                input.text.trim_end(),
                input.points,
                input.answer_type as AnswerType,
                img.as_deref(),
            )
            .execute(&mut *tx)
            .await?
        }
    };
    if r.rows_affected() == 0 {
        return Ok(UpdateOutcome::NotFound);
    }
    let kept: Vec<Uuid> = input
        .options
        .iter()
        .filter_map(|o| o.id)
        .filter(|id| existing.contains(id))
        .collect();
    let structure_changed =
        kept.len() != existing.len() || input.options.iter().any(|o| o.id.is_none());
    if locked && structure_changed {
        return Ok(UpdateOutcome::Locked);
    }
    sqlx::query!(
        "DELETE FROM options WHERE question_id = $1 AND id <> ALL($2)",
        id,
        &kept
    )
    .execute(&mut *tx)
    .await?;
    for (pos, o) in input.options.iter().enumerate() {
        let pos = i32::try_from(pos + 1).unwrap_or(i32::MAX);
        match o.id.filter(|oid| existing.contains(oid)) {
            Some(oid) => {
                sqlx::query!(
                    "UPDATE options SET position = $2, text = $3, is_correct = $4 WHERE id = $1",
                    oid,
                    pos,
                    o.text.trim(),
                    o.is_correct
                )
                .execute(&mut *tx)
                .await?;
            }
            None => {
                sqlx::query!(
                    "INSERT INTO options (question_id, position, text, is_correct)
                     VALUES ($1, $2, $3, $4)",
                    id,
                    pos,
                    o.text.trim(),
                    o.is_correct
                )
                .execute(&mut *tx)
                .await?;
            }
        }
    }
    touch_exam(&mut tx, exam_id).await?;
    tx.commit().await?;
    Ok(UpdateOutcome::Updated)
}

/// Deletes a question and closes the gap in positions. Returns its image path.
pub async fn delete(
    db: &PgPool,
    exam_id: Uuid,
    id: Uuid,
) -> Result<Option<Option<String>>, sqlx::Error> {
    let mut tx = db.begin().await?;
    let deleted = sqlx::query!(
        "DELETE FROM questions WHERE id = $1 AND exam_id = $2 RETURNING position, image_path",
        id,
        exam_id
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = deleted else {
        return Ok(None);
    };
    sqlx::query!(
        "UPDATE questions SET position = position - 1 WHERE exam_id = $1 AND position > $2",
        exam_id,
        row.position
    )
    .execute(&mut *tx)
    .await?;
    touch_exam(&mut tx, exam_id).await?;
    tx.commit().await?;
    Ok(Some(row.image_path))
}

/// Applies a new canonical order. `ids` must be exactly the exam's question ids.
pub async fn reorder(db: &PgPool, exam_id: Uuid, ids: &[Uuid]) -> Result<bool, sqlx::Error> {
    let mut tx = db.begin().await?;
    let mut current: Vec<Uuid> = sqlx::query_scalar!(
        "SELECT id FROM questions WHERE exam_id = $1 FOR UPDATE",
        exam_id
    )
    .fetch_all(&mut *tx)
    .await?;
    let mut wanted = ids.to_vec();
    current.sort();
    wanted.sort();
    wanted.dedup();
    if current != wanted || wanted.len() != ids.len() {
        return Ok(false);
    }
    sqlx::query!(
        "UPDATE questions q SET position = t.ord::int
         FROM unnest($2::uuid[]) WITH ORDINALITY AS t(id, ord)
         WHERE q.id = t.id AND q.exam_id = $1",
        exam_id,
        ids
    )
    .execute(&mut *tx)
    .await?;
    touch_exam(&mut tx, exam_id).await?;
    tx.commit().await?;
    Ok(true)
}

/// Moves a question one step up (-1) or down (+1).
pub async fn move_by(db: &PgPool, exam_id: Uuid, id: Uuid, delta: i32) -> Result<(), sqlx::Error> {
    let mut ids: Vec<Uuid> = sqlx::query_scalar!(
        "SELECT id FROM questions WHERE exam_id = $1 ORDER BY position, id",
        exam_id
    )
    .fetch_all(db)
    .await?;
    if let Some(i) = ids.iter().position(|q| *q == id) {
        let j = i.checked_add_signed(delta as isize);
        if let Some(j) = j.filter(|j| *j < ids.len()) {
            ids.swap(i, j);
            reorder(db, exam_id, &ids).await?;
        }
    }
    Ok(())
}

pub async fn set_image(
    db: &PgPool,
    exam_id: Uuid,
    id: Uuid,
    path: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE questions SET image_path = $3 WHERE id = $1 AND exam_id = $2",
        id,
        exam_id,
        path
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Inserts a copy of a question right after position `after` (shifting later ones).
pub async fn insert_after(
    db: &PgPool,
    exam_id: Uuid,
    after: i32,
    q: &QuestionInput,
    image: Option<String>,
) -> Result<Uuid, sqlx::Error> {
    let mut tx = db.begin().await?;
    sqlx::query!("SELECT id FROM exams WHERE id = $1 FOR UPDATE", exam_id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query!(
        "UPDATE questions SET position = position + 1 WHERE exam_id = $1 AND position > $2",
        exam_id,
        after
    )
    .execute(&mut *tx)
    .await?;
    let id = sqlx::query_scalar!(
        "INSERT INTO questions (exam_id, position, text, image_path, points, answer_type)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
        exam_id,
        after + 1,
        q.text.trim_end(),
        image.as_deref(),
        q.points,
        q.answer_type as AnswerType,
    )
    .fetch_one(&mut *tx)
    .await?;
    insert_options(&mut tx, id, &q.options).await?;
    touch_exam(&mut tx, exam_id).await?;
    tx.commit().await?;
    Ok(id)
}

impl Question {
    pub fn to_input(&self) -> QuestionInput {
        QuestionInput {
            text: self.text.clone(),
            points: self.points,
            answer_type: self.answer_type,
            options: self
                .options
                .iter()
                .map(|o| OptionInput {
                    id: None,
                    text: o.text.clone(),
                    is_correct: o.is_correct,
                })
                .collect(),
        }
    }
}
