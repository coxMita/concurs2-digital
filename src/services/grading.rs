//! Pure grading rules.
//!
//! * Single-answer question: full points if the one correct option is selected (and
//!   nothing else), otherwise 0.
//! * Multiple-answer question, depending on the exam's scoring mode:
//!   - all or nothing: full points only if the selected set equals the correct set;
//!   - partial (default for new exams): fraction = correct selected / all correct
//!     − wrong selected / all wrong, never below 0. Selecting every option always
//!     gives 0; one wrong cancels one correct.
//! * Points earned are rounded to 2 decimals; final grade: round(earned / max * 100, 2).

use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use crate::db::{
    exams::ScoringMode,
    questions::{AnswerType, Question},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Correct,
    /// Some, but not full, credit (partial scoring only).
    Partial,
    Wrong,
    Unanswered,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Correct => "Corect",
            Self::Partial => "Parțial",
            Self::Wrong => "Greșit",
            Self::Unanswered => "Fără răspuns",
        }
    }
}

/// Result for one question: outcome plus the fraction of its points earned (0..=1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuestionScore {
    pub outcome: Outcome,
    pub fraction: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grade {
    pub earned: f64,
    pub max: i32,
    pub percentage: f64,
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Scores one question given the selected option ids.
pub fn question_score(
    q: &Question,
    selected: Option<&HashSet<Uuid>>,
    scoring: ScoringMode,
) -> QuestionScore {
    let Some(selected) = selected.filter(|s| !s.is_empty()) else {
        return QuestionScore {
            outcome: Outcome::Unanswered,
            fraction: 0.0,
        };
    };
    let correct: HashSet<Uuid> = q
        .options
        .iter()
        .filter(|o| o.is_correct)
        .map(|o| o.id)
        .collect();
    let fraction = if correct.is_empty() {
        0.0
    } else if *selected == correct {
        1.0
    } else if q.answer_type == AnswerType::Multiple && scoring == ScoringMode::Partial {
        let total_correct = correct.len() as f64;
        let total_wrong = (q.options.len() - correct.len()) as f64;
        let hits = selected.iter().filter(|o| correct.contains(o)).count() as f64;
        let misses = selected.len() as f64 - hits;
        let penalty = if total_wrong > 0.0 {
            misses / total_wrong
        } else {
            0.0
        };
        (hits / total_correct - penalty).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let outcome = if fraction >= 1.0 {
        Outcome::Correct
    } else if fraction > 0.0 {
        Outcome::Partial
    } else {
        Outcome::Wrong
    };
    QuestionScore { outcome, fraction }
}

/// Outcome only (convenience for exports and reviews).
pub fn question_outcome(
    q: &Question,
    selected: Option<&HashSet<Uuid>>,
    scoring: ScoringMode,
) -> Outcome {
    question_score(q, selected, scoring).outcome
}

pub fn percentage(earned: f64, max: i32) -> f64 {
    if max <= 0 {
        return 0.0;
    }
    round2(earned / f64::from(max) * 100.0)
}

/// Grades the given questions (those of the attempt) against the selections
/// (question id -> selected option ids).
pub fn grade<'a>(
    questions: impl IntoIterator<Item = &'a Question>,
    selections: &HashMap<Uuid, HashSet<Uuid>>,
    scoring: ScoringMode,
) -> Grade {
    let mut earned = 0.0;
    let mut max = 0;
    for q in questions {
        max += q.points;
        earned += f64::from(q.points) * question_score(q, selections.get(&q.id), scoring).fraction;
    }
    let earned = round2(earned);
    Grade {
        earned,
        max,
        percentage: percentage(earned, max),
    }
}

/// Romanian 1–10 grade from a percentage: 1 + 9 × p / 100, rounded to 2 decimals
/// (0% -> 1, 50% -> 5.5, 100% -> 10).
pub fn grade_1_10(percentage: f64) -> f64 {
    round2(1.0 + 9.0 * percentage.clamp(0.0, 100.0) / 100.0)
}

/// Points for display: "3", "1,5", "0,67" (Romanian decimal comma).
pub fn fmt_points(p: f64) -> String {
    let p = round2(p);
    if (p - p.round()).abs() < 1e-9 {
        format!("{}", p.round() as i64)
    } else {
        format!("{p:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .replace('.', ",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::questions::OptionRow;
    use ScoringMode::{AllOrNothing, Partial};

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

    fn frac(q: &Question, idx: &[usize], mode: ScoringMode) -> f64 {
        question_score(q, Some(&sel(q, idx)), mode).fraction
    }

    #[test]
    fn single_answer() {
        let q1 = q(2, AnswerType::Single, &[false, true, false]);
        for mode in [AllOrNothing, Partial] {
            assert_eq!(
                question_outcome(&q1, Some(&sel(&q1, &[1])), mode),
                Outcome::Correct
            );
            assert_eq!(
                question_outcome(&q1, Some(&sel(&q1, &[0])), mode),
                Outcome::Wrong
            );
            assert_eq!(
                question_outcome(&q1, Some(&sel(&q1, &[0, 1])), mode),
                Outcome::Wrong
            );
            assert_eq!(question_outcome(&q1, None, mode), Outcome::Unanswered);
            assert_eq!(
                question_outcome(&q1, Some(&HashSet::new()), mode),
                Outcome::Unanswered
            );
        }
    }

    #[test]
    fn multiple_answer_all_or_nothing_requires_exact_set() {
        let q1 = q(3, AnswerType::Multiple, &[true, true, false, true]);
        let o = |idx: &[usize]| question_outcome(&q1, Some(&sel(&q1, idx)), AllOrNothing);
        assert_eq!(o(&[0, 1, 3]), Outcome::Correct);
        assert_eq!(o(&[0, 1]), Outcome::Wrong, "partial selection scores 0");
        assert_eq!(o(&[0, 1, 2, 3]), Outcome::Wrong, "superset scores 0");
    }

    /// The teacher's table: 4 options, A and B correct, C and D wrong.
    #[test]
    fn multiple_answer_partial_scoring() {
        let q1 = q(2, AnswerType::Multiple, &[true, true, false, false]);
        let (a, b, c, d) = (0, 1, 2, 3);
        assert_eq!(frac(&q1, &[a], Partial), 0.5);
        assert_eq!(frac(&q1, &[b], Partial), 0.5);
        assert_eq!(frac(&q1, &[a, b], Partial), 1.0);
        assert_eq!(
            frac(&q1, &[a, c], Partial),
            0.0,
            "one wrong cancels one correct"
        );
        assert_eq!(
            frac(&q1, &[a, b, c], Partial),
            0.5,
            "like a single correct one"
        );
        assert_eq!(
            frac(&q1, &[a, b, c, d], Partial),
            0.0,
            "everything selected"
        );
        assert_eq!(frac(&q1, &[c, d], Partial), 0.0);
        assert_eq!(
            question_outcome(&q1, Some(&sel(&q1, &[a])), Partial),
            Outcome::Partial
        );
        assert_eq!(
            question_outcome(&q1, Some(&sel(&q1, &[a, c])), Partial),
            Outcome::Wrong
        );
    }

    #[test]
    fn selecting_everything_always_scores_zero() {
        for correct in [
            vec![true, false, false],
            vec![true, true, false, false, false],
            vec![true, true, true, false],
        ] {
            let q1 = q(1, AnswerType::Multiple, &correct);
            let all: Vec<usize> = (0..correct.len()).collect();
            assert_eq!(frac(&q1, &all, Partial), 0.0);
        }
    }

    #[test]
    fn uneven_counts() {
        // 5 options, 2 correct: a wrong pick costs 1/3, a correct one is worth 1/2.
        let q1 = q(3, AnswerType::Multiple, &[true, true, false, false, false]);
        assert!((frac(&q1, &[0, 2], Partial) - (0.5 - 1.0 / 3.0)).abs() < 1e-9);
        assert!((frac(&q1, &[0, 1, 2], Partial) - (1.0 - 1.0 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn totals_and_rounding() {
        let a = q(1, AnswerType::Single, &[true, false]);
        let b = q(1, AnswerType::Single, &[true, false]);
        let c = q(1, AnswerType::Multiple, &[true, true, false]);
        let mut s = HashMap::new();
        s.insert(a.id, sel(&a, &[0]));
        s.insert(c.id, sel(&c, &[0]));
        let g = grade([&a, &b, &c], &s, AllOrNothing);
        assert_eq!(g.earned, 1.0);
        assert_eq!(g.max, 3);
        assert!((g.percentage - 33.33).abs() < 1e-9);

        let g = grade([&a, &b, &c], &s, Partial);
        assert_eq!(g.earned, 1.5);
        assert_eq!(g.percentage, 50.0);

        assert!((percentage(2.0, 3) - 66.67).abs() < 1e-9);
        assert_eq!(percentage(0.0, 0), 0.0);
        assert_eq!(percentage(5.0, 5), 100.0);
    }

    #[test]
    fn romanian_grade() {
        assert_eq!(grade_1_10(0.0), 1.0);
        assert_eq!(grade_1_10(50.0), 5.5);
        assert_eq!(grade_1_10(100.0), 10.0);
        assert_eq!(grade_1_10(33.33), 4.0);
    }

    #[test]
    fn points_formatting() {
        assert_eq!(fmt_points(3.0), "3");
        assert_eq!(fmt_points(1.5), "1,5");
        assert_eq!(fmt_points(2.0 / 3.0), "0,67");
        assert_eq!(fmt_points(0.0), "0");
    }
}
