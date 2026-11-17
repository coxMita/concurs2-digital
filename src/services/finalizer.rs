//! Attempt finalization and regrading, in batches.
//!
//! An attempt is finalized with `submitted_at = least(now(), deadline)`, so a
//! manual submit records the actual time and an expired attempt the deadline.
//! The same code runs for: manual submit, lazy finalization on student requests,
//! exam closing and the periodic background task.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{error::AppError, services::grading, state::AppState};

const BATCH: i64 = 200;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Set submitted_at and the score.
    Finalize,
    /// Recompute the score of already submitted attempts.
    Regrade,
}

/// Grades `attempts` (id, exam_id) inside `tx` and stores the results.
async fn grade_and_store(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    attempts: &[(Uuid, Uuid)],
    mode: Mode,
) -> Result<(), AppError> {
    if attempts.is_empty() {
        return Ok(());
    }
    let ids: Vec<Uuid> = attempts.iter().map(|a| a.0).collect();
    let aq = sqlx::query!(
        "SELECT attempt_id, question_id FROM attempt_questions WHERE attempt_id = ANY($1)",
        &ids
    )
    .fetch_all(&mut **tx)
    .await?;
    let sels = sqlx::query!(
        "SELECT attempt_id, question_id, option_id FROM answer_selections
         WHERE attempt_id = ANY($1)",
        &ids
    )
    .fetch_all(&mut **tx)
    .await?;

    let mut questions_of: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for r in aq {
        questions_of
            .entry(r.attempt_id)
            .or_default()
            .push(r.question_id);
    }
    let mut selections_of: HashMap<Uuid, HashMap<Uuid, HashSet<Uuid>>> = HashMap::new();
    for r in sels {
        selections_of
            .entry(r.attempt_id)
            .or_default()
            .entry(r.question_id)
            .or_default()
            .insert(r.option_id);
    }

    let mut contents = HashMap::new();
    let (mut earned, mut max, mut pct) = (Vec::new(), Vec::new(), Vec::new());
    let empty = HashMap::new();
    for (attempt_id, exam_id) in attempts {
        if !contents.contains_key(exam_id) {
            contents.insert(*exam_id, state.cache.get(&state.db, *exam_id).await?);
        }
        let content = &contents[exam_id];
        let qs = questions_of
            .get(attempt_id)
            .map(|v| {
                v.iter()
                    .filter_map(|q| content.question(*q))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let g = grading::grade(
            qs,
            selections_of.get(attempt_id).unwrap_or(&empty),
            content.exam.scoring,
        );
        earned.push(g.earned);
        max.push(g.max);
        pct.push(g.percentage);
    }

    let finalize = mode == Mode::Finalize;
    sqlx::query!(
        "UPDATE attempts a SET
            earned_points = v.e, max_points = v.m, percentage = v.p,
            submitted_at = CASE WHEN $5 THEN LEAST(now(), a.deadline) ELSE a.submitted_at END
         FROM unnest($1::uuid[], $2::float8[], $3::int[], $4::float8[]) AS v(id, e, m, p)
         WHERE a.id = v.id",
        &ids,
        &earned,
        &max,
        &pct,
        finalize
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Finalizes one in-progress attempt (manual submit or lazy expiry). Returns true
/// if this call finalized it.
pub async fn finalize_attempt(state: &AppState, attempt_id: Uuid) -> Result<bool, AppError> {
    let mut tx = state.db.begin().await?;
    let row = sqlx::query!(
        "SELECT id, exam_id FROM attempts WHERE id = $1 AND submitted_at IS NULL FOR UPDATE",
        attempt_id
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Ok(false);
    };
    grade_and_store(state, &mut tx, &[(row.id, row.exam_id)], Mode::Finalize).await?;
    tx.commit().await?;
    Ok(true)
}

/// Finalizes every expired attempt, in batches. Returns how many were finalized.
pub async fn finalize_expired(state: &AppState) -> Result<usize, AppError> {
    let mut total = 0;
    loop {
        let mut tx = state.db.begin().await?;
        let rows = sqlx::query!(
            "SELECT id, exam_id FROM attempts
             WHERE submitted_at IS NULL AND deadline <= now()
             ORDER BY deadline LIMIT $1 FOR UPDATE SKIP LOCKED",
            BATCH
        )
        .fetch_all(&mut *tx)
        .await?;
        let batch: Vec<(Uuid, Uuid)> = rows.into_iter().map(|r| (r.id, r.exam_id)).collect();
        grade_and_store(state, &mut tx, &batch, Mode::Finalize).await?;
        tx.commit().await?;
        total += batch.len();
        if batch.len() < BATCH as usize {
            return Ok(total);
        }
    }
}

/// Finalizes all in-progress attempts of an exam (used when closing it).
pub async fn finalize_exam(state: &AppState, exam_id: Uuid) -> Result<usize, AppError> {
    let mut total = 0;
    loop {
        let mut tx = state.db.begin().await?;
        let rows = sqlx::query!(
            "SELECT id FROM attempts WHERE exam_id = $1 AND submitted_at IS NULL
             LIMIT $2 FOR UPDATE",
            exam_id,
            BATCH
        )
        .fetch_all(&mut *tx)
        .await?;
        let batch: Vec<(Uuid, Uuid)> = rows.into_iter().map(|r| (r.id, exam_id)).collect();
        grade_and_store(state, &mut tx, &batch, Mode::Finalize).await?;
        tx.commit().await?;
        total += batch.len();
        if batch.len() < BATCH as usize {
            return Ok(total);
        }
    }
}

/// Recomputes scores of all submitted attempts of an exam (after answer-key edits).
pub async fn regrade_exam(state: &AppState, exam_id: Uuid) -> Result<usize, AppError> {
    let mut total = 0;
    let mut after = Uuid::nil();
    loop {
        let mut tx = state.db.begin().await?;
        let rows = sqlx::query!(
            "SELECT id FROM attempts
             WHERE exam_id = $1 AND submitted_at IS NOT NULL AND id > $2
             ORDER BY id LIMIT $3 FOR UPDATE",
            exam_id,
            after,
            BATCH
        )
        .fetch_all(&mut *tx)
        .await?;
        let batch: Vec<(Uuid, Uuid)> = rows.into_iter().map(|r| (r.id, exam_id)).collect();
        grade_and_store(state, &mut tx, &batch, Mode::Regrade).await?;
        tx.commit().await?;
        total += batch.len();
        match batch.last() {
            Some((id, _)) if batch.len() == BATCH as usize => after = *id,
            _ => return Ok(total),
        }
    }
}

/// Background task: finalizes expired attempts every `finalizer_interval_secs`.
pub fn spawn(state: AppState) {
    let every = Duration::from_secs(state.config.finalizer_interval_secs.max(1));
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            match finalize_expired(&state).await {
                Ok(0) => {}
                Ok(n) => tracing::info!("finalizer: finalized {n} expired attempts"),
                Err(e) => tracing::error!("finalizer failed: {e}"),
            }
        }
    });
}
