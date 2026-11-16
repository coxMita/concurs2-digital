//! Per-attempt random order of questions and of options within each question.

use rand::seq::SliceRandom;
use uuid::Uuid;

use crate::db::questions::Question;

/// Returns (question id, option ids in display order) in display order.
pub fn shuffle_attempt(questions: &[Question]) -> Vec<(Uuid, Vec<Uuid>)> {
    let mut rng = rand::rng();
    let mut order: Vec<(Uuid, Vec<Uuid>)> = questions
        .iter()
        .map(|q| {
            let mut opts: Vec<Uuid> = q.options.iter().map(|o| o.id).collect();
            opts.shuffle(&mut rng);
            (q.id, opts)
        })
        .collect();
    order.shuffle(&mut rng);
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::questions::{AnswerType, OptionRow};

    #[test]
    fn is_a_permutation() {
        let qs: Vec<Question> = (0..20)
            .map(|i| Question {
                id: Uuid::new_v4(),
                exam_id: Uuid::nil(),
                position: i,
                text: String::new(),
                image_path: None,
                points: 1,
                answer_type: AnswerType::Single,
                options: (0..4)
                    .map(|_| OptionRow {
                        id: Uuid::new_v4(),
                        text: String::new(),
                        is_correct: false,
                    })
                    .collect(),
            })
            .collect();
        let order = shuffle_attempt(&qs);
        assert_eq!(order.len(), 20);
        let mut a: Vec<Uuid> = order.iter().map(|o| o.0).collect();
        let mut b: Vec<Uuid> = qs.iter().map(|q| q.id).collect();
        a.sort();
        b.sort();
        assert_eq!(a, b);
        for (qid, opts) in &order {
            let q = qs.iter().find(|q| q.id == *qid).unwrap();
            let mut x = opts.clone();
            let mut y: Vec<Uuid> = q.options.iter().map(|o| o.id).collect();
            x.sort();
            y.sort();
            assert_eq!(x, y);
        }
        // With 20 questions the chance of the identity permutation is negligible.
        let identity: Vec<Uuid> = qs.iter().map(|q| q.id).collect();
        let shuffled: Vec<Uuid> = order.iter().map(|o| o.0).collect();
        assert_ne!(identity, shuffled);
    }
}
