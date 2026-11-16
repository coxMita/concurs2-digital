//! Pure grading rules.
//!
//! * Single-answer question: full points if the one correct option is selected (and
//!   nothing else), otherwise 0.
//! * Multiple-answer question: full points only if the selected set equals the
//!   correct set exactly, otherwise 0 (no partial credit).
//! * Final grade: round(earned / max * 100, 2).

use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use crate::db::questions::Question;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Correct,
    Wrong,
    Unanswered,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Correct => "Corect",
            Self::Wrong => "Greșit",
            Self::Unanswered => "Fără răspuns",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grade {
    pub earned: i32,
    pub max: i32,
    pub percentage: f64,
}

/// Outcome of one question given the selected option ids.
pub fn question_outcome(q: &Question, selected: Option<&HashSet<Uuid>>) -> Outcome {
    let Some(selected) = selected.filter(|s| !s.is_empty()) else {
        return Outcome::Unanswered;
    };
    let correct: HashSet<Uuid> = q
        .options
        .iter()
        .filter(|o| o.is_correct)
        .map(|o| o.id)
        .collect();
    if !correct.is_empty() && *selected == correct {
        Outcome::Correct
    } else {
        Outcome::Wrong
    }
}

pub fn percentage(earned: i32, max: i32) -> f64 {
    if max <= 0 {
        return 0.0;
    }
    (f64::from(earned) / f64::from(max) * 10_000.0).round() / 100.0
}

/// Grades the given questions (those of the attempt) against the selections
/// (question id -> selected option ids).
pub fn grade<'a>(
    questions: impl IntoIterator<Item = &'a Question>,
    selections: &HashMap<Uuid, HashSet<Uuid>>,
) -> Grade {
    let mut earned = 0;
    let mut max = 0;
    for q in questions {
        max += q.points;
        if question_outcome(q, selections.get(&q.id)) == Outcome::Correct {
            earned += q.points;
        }
    }
    Grade {
        earned,
        max,
        percentage: percentage(earned, max),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::questions::{AnswerType, OptionRow};

    fn q(points: i32, t: AnswerType, correct: &[bool]) -> Question {
        Question {
            id: Uuid::new_v4(),
            exam_id: Uuid::nil(),
            position: 1,
            text: "q".into(),
            image_path: None,
            points,
            answer_type: t,
            options: correct
                .iter()
                .map(|&c| OptionRow {
                    id: Uuid::new_v4(),
                    text: "o".into(),
                    is_correct: c,
                })
                .collect(),
        }
    }

    fn sel(q: &Question, idx: &[usize]) -> HashSet<Uuid> {
        idx.iter().map(|&i| q.options[i].id).collect()
    }

    #[test]
    fn single_answer() {
        let q1 = q(2, AnswerType::Single, &[false, true, false]);
        assert_eq!(
            question_outcome(&q1, Some(&sel(&q1, &[1]))),
            Outcome::Correct
        );
        assert_eq!(question_outcome(&q1, Some(&sel(&q1, &[0]))), Outcome::Wrong);
        assert_eq!(
            question_outcome(&q1, Some(&sel(&q1, &[0, 1]))),
            Outcome::Wrong
        );
        assert_eq!(question_outcome(&q1, None), Outcome::Unanswered);
        assert_eq!(
            question_outcome(&q1, Some(&HashSet::new())),
            Outcome::Unanswered
        );
    }

    #[test]
    fn multiple_answer_requires_exact_set() {
        let q1 = q(3, AnswerType::Multiple, &[true, true, false, true]);
        assert_eq!(
            question_outcome(&q1, Some(&sel(&q1, &[0, 1, 3]))),
            Outcome::Correct
        );
        // Partial selection scores 0.
        assert_eq!(
            question_outcome(&q1, Some(&sel(&q1, &[0, 1]))),
            Outcome::Wrong
        );
        // Superset scores 0.
        assert_eq!(
            question_outcome(&q1, Some(&sel(&q1, &[0, 1, 2, 3]))),
            Outcome::Wrong
        );
    }

    #[test]
    fn totals_and_rounding() {
        let a = q(1, AnswerType::Single, &[true, false]);
        let b = q(1, AnswerType::Single, &[true, false]);
        let c = q(1, AnswerType::Multiple, &[true, true, false]);
        let mut s = HashMap::new();
        s.insert(a.id, sel(&a, &[0]));
        s.insert(c.id, sel(&c, &[0]));
        let g = grade([&a, &b, &c], &s);
        assert_eq!(g.earned, 1);
        assert_eq!(g.max, 3);
        assert!((g.percentage - 33.33).abs() < 1e-9);
        assert!((percentage(2, 3) - 66.67).abs() < 1e-9);
        assert_eq!(percentage(0, 0), 0.0);
        assert_eq!(percentage(5, 5), 100.0);
    }
}
