//! In-memory cache of an exam's settings, questions and options, used by the hot
//! student paths, grading and exports. Invalidated on every admin edit.

use std::{collections::HashMap, sync::Arc, time::Duration};

use moka::future::Cache;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    db::{
        exams::{self, Exam},
        questions::{self, Question},
    },
    error::AppError,
};

pub struct ExamContent {
    pub exam: Exam,
    /// Canonical order.
    pub questions: Vec<Question>,
    index: HashMap<Uuid, usize>,
}

impl ExamContent {
    pub fn new(exam: Exam, questions: Vec<Question>) -> Self {
        let index = questions
            .iter()
            .enumerate()
            .map(|(i, q)| (q.id, i))
            .collect();
        Self {
            exam,
            questions,
            index,
        }
    }

    pub fn question(&self, id: Uuid) -> Option<&Question> {
        self.index.get(&id).map(|&i| &self.questions[i])
    }

    /// 1-based canonical number of a question.
    pub fn canonical_number(&self, id: Uuid) -> Option<usize> {
        self.index.get(&id).map(|i| i + 1)
    }
}

#[derive(Clone)]
pub struct ExamCache {
    inner: Cache<Uuid, Arc<ExamContent>>,
}

impl Default for ExamCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ExamCache {
    pub fn new() -> Self {
        Self {
            inner: Cache::builder()
                .max_capacity(64)
                .time_to_idle(Duration::from_secs(3600))
                .build(),
        }
    }

    pub async fn get(&self, db: &PgPool, exam_id: Uuid) -> Result<Arc<ExamContent>, AppError> {
        self.inner
            .try_get_with(exam_id, async {
                let exam = exams::get(db, exam_id).await?.ok_or(AppError::NotFound)?;
                let questions = questions::list_full(db, exam_id).await?;
                Ok::<_, AppError>(Arc::new(ExamContent::new(exam, questions)))
            })
            .await
            .map_err(|e: Arc<AppError>| match &*e {
                AppError::NotFound => AppError::NotFound,
                other => AppError::Internal(other.to_string()),
            })
    }

    pub async fn invalidate(&self, exam_id: Uuid) {
        self.inner.invalidate(&exam_id).await;
    }
}
