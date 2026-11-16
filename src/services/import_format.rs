//! Plain-text question format used for bulk import and export.
//!
//! ```text
//! # Lines starting with '#' are comments.
//! Î: Care este capitala României?
//! P: 1
//! T: simplu
//! + București
//! - Cluj-Napoca
//!
//! Î: Care dintre numerele următoare sunt prime?
//! T: multiplu
//! + 2
//! + 3
//! - 4
//! ```
//!
//! * `Î:` (or `I:` / `Q:`) starts a question; following plain lines continue its text.
//! * `P:` points (optional, default 1). `T:` `simplu` or `multiplu` (optional: inferred
//!   from the number of correct options).
//! * `+ text` is a correct option, `- text` a wrong one (2–8 options).
//! * A text line starting with `\` is taken literally (escape for lines beginning with `+`, `-`, …).

use crate::db::questions::{AnswerType, OptionInput, Question, QuestionInput};

#[derive(Debug, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

#[derive(Default)]
struct Draft {
    line: usize,
    text: Vec<String>,
    points: Option<i32>,
    answer_type: Option<AnswerType>,
    options: Vec<OptionInput>,
}

fn strip_prefix_ci<'a>(line: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    prefixes.iter().find_map(|p| {
        let head = line.get(..p.len())?;
        if head.eq_ignore_ascii_case(p) || head == *p {
            Some(line[p.len()..].trim())
        } else {
            None
        }
    })
}

fn parse_type(v: &str) -> Option<AnswerType> {
    match v.trim().to_lowercase().as_str() {
        "simplu" | "unic" | "single" | "un singur raspuns" | "un singur răspuns" => {
            Some(AnswerType::Single)
        }
        "multiplu" | "multiple" | "mai multe" | "mai multe raspunsuri" | "mai multe răspunsuri" => {
            Some(AnswerType::Multiple)
        }
        _ => None,
    }
}

fn finish(d: Draft, out: &mut Vec<QuestionInput>, errors: &mut Vec<ParseError>) {
    let correct = d.options.iter().filter(|o| o.is_correct).count();
    let answer_type = d.answer_type.unwrap_or(if correct > 1 {
        AnswerType::Multiple
    } else {
        AnswerType::Single
    });
    let q = QuestionInput {
        text: d.text.join("\n").trim().to_string(),
        points: d.points.unwrap_or(1),
        answer_type,
        options: d.options,
    };
    match q.validate() {
        Ok(()) => out.push(q),
        Err(message) => errors.push(ParseError {
            line: d.line,
            message,
        }),
    }
}

/// Parses the whole text. Returns all questions, or every error found (with line numbers).
pub fn parse(input: &str) -> Result<Vec<QuestionInput>, Vec<ParseError>> {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    let mut current: Option<Draft> = None;

    for (i, raw) in input.lines().enumerate() {
        let line_no = i + 1;
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some(rest) = strip_prefix_ci(trimmed, &["Î:", "î:", "I:", "Q:"]) {
            if let Some(d) = current.take() {
                finish(d, &mut out, &mut errors);
            }
            current = Some(Draft {
                line: line_no,
                text: if rest.is_empty() {
                    vec![]
                } else {
                    vec![rest.to_string()]
                },
                ..Default::default()
            });
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        let Some(d) = current.as_mut() else {
            errors.push(ParseError {
                line: line_no,
                message: "Textul trebuie să înceapă cu o întrebare („Î: ...”).".into(),
            });
            continue;
        };
        if let Some(escaped) = trimmed.strip_prefix('\\') {
            // "\" escapes a text line that would otherwise look like a directive/option.
            if d.options.is_empty() {
                d.text.push(escaped.to_string());
            } else {
                errors.push(ParseError {
                    line: line_no,
                    message: "Linie neașteptată după variante.".into(),
                });
            }
        } else if let Some(v) = strip_prefix_ci(trimmed, &["P:"]) {
            match v.parse::<i32>() {
                Ok(p) => d.points = Some(p),
                Err(_) => errors.push(ParseError {
                    line: line_no,
                    message: format!("Punctaj invalid: „{v}”."),
                }),
            }
        } else if let Some(v) = strip_prefix_ci(trimmed, &["T:"]) {
            match parse_type(v) {
                Some(t) => d.answer_type = Some(t),
                None => errors.push(ParseError {
                    line: line_no,
                    message: format!("Tip necunoscut „{v}”: folosiți „simplu” sau „multiplu”."),
                }),
            }
        } else if let Some(text) = trimmed.strip_prefix('+') {
            d.options.push(OptionInput {
                id: None,
                text: text.trim().to_string(),
                is_correct: true,
            });
        } else if let Some(text) = trimmed.strip_prefix('-') {
            d.options.push(OptionInput {
                id: None,
                text: text.trim().to_string(),
                is_correct: false,
            });
        } else if d.options.is_empty() {
            d.text.push(line.to_string());
        } else {
            errors.push(ParseError {
                line: line_no,
                message: "Linie neașteptată după variante. Variantele încep cu „+” sau „-”.".into(),
            });
        }
    }
    if let Some(d) = current.take() {
        finish(d, &mut out, &mut errors);
    }
    if out.is_empty() && errors.is_empty() {
        errors.push(ParseError {
            line: 1,
            message: "Nu a fost găsită nicio întrebare.".into(),
        });
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        errors.sort_by_key(|e| e.line);
        Err(errors)
    }
}

/// Serializes questions to the same format (images are not included).
pub fn export(questions: &[Question]) -> String {
    let mut s = String::from(
        "# Întrebări exportate. Format: „Î:” întrebare, „P:” punctaj, „T:” simplu/multiplu,\n# „+” variantă corectă, „-” variantă greșită.\n\n",
    );
    for q in questions {
        let mut lines = q.text.lines().filter(|l| !l.trim().is_empty());
        s.push_str("Î: ");
        s.push_str(lines.next().unwrap_or("").trim_end());
        s.push('\n');
        for l in lines {
            // Avoid continuation lines being read back as directives or options.
            let t = l.trim_start();
            if t.starts_with(['+', '-', '#', '\\'])
                || strip_prefix_ci(t, &["Î:", "î:", "I:", "Q:", "P:", "T:"]).is_some()
            {
                s.push('\\');
            }
            s.push_str(l.trim_end());
            s.push('\n');
        }
        s.push_str(&format!("P: {}\n", q.points));
        s.push_str(match q.answer_type {
            AnswerType::Single => "T: simplu\n",
            AnswerType::Multiple => "T: multiplu\n",
        });
        for o in &q.options {
            s.push(if o.is_correct { '+' } else { '-' });
            s.push(' ');
            s.push_str(&o.text.split_whitespace().collect::<Vec<_>>().join(" "));
            s.push('\n');
        }
        s.push('\n');
    }
    s
}

pub const EXAMPLE: &str = "Î: Care este capitala României?
P: 1
T: simplu
+ București
- Cluj-Napoca
- Iași

Î: Care dintre numerele următoare sunt prime?
T: multiplu
+ 2
+ 3
- 4
- 9
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::questions::OptionRow;
    use uuid::Uuid;

    #[test]
    fn parses_example() {
        let qs = parse(EXAMPLE).unwrap();
        assert_eq!(qs.len(), 2);
        assert_eq!(qs[0].text, "Care este capitala României?");
        assert_eq!(qs[0].answer_type, AnswerType::Single);
        assert_eq!(qs[0].options.len(), 3);
        assert!(qs[0].options[0].is_correct);
        assert_eq!(qs[1].answer_type, AnswerType::Multiple);
        assert_eq!(qs[1].options.iter().filter(|o| o.is_correct).count(), 2);
    }

    #[test]
    fn multiline_text_and_inferred_type() {
        let qs = parse("Î: Prima linie\na doua linie\nP: 3\n+ a\n+ b\n- c\n").unwrap();
        assert_eq!(qs[0].text, "Prima linie\na doua linie");
        assert_eq!(qs[0].points, 3);
        assert_eq!(qs[0].answer_type, AnswerType::Multiple);
    }

    #[test]
    fn reports_errors_with_lines() {
        let errs = parse("Î: x\nT: simplu\n+ a\n+ b\n\nÎ: y\n+ only one\n").unwrap_err();
        assert_eq!(errs.len(), 2);
        assert_eq!(errs[0].line, 1);
        assert_eq!(errs[1].line, 6);
        assert!(parse("").is_err());
        assert!(parse("+ a\n").is_err());
    }

    #[test]
    fn export_round_trips() {
        let q = Question {
            id: Uuid::new_v4(),
            exam_id: Uuid::new_v4(),
            position: 1,
            text: "Ce este\n- o capcană?".into(),
            image_path: None,
            points: 2,
            answer_type: AnswerType::Multiple,
            options: vec![
                OptionRow {
                    id: Uuid::new_v4(),
                    text: "Da".into(),
                    is_correct: true,
                },
                OptionRow {
                    id: Uuid::new_v4(),
                    text: "Nu".into(),
                    is_correct: false,
                },
                OptionRow {
                    id: Uuid::new_v4(),
                    text: "Poate".into(),
                    is_correct: true,
                },
            ],
        };
        let text = export(std::slice::from_ref(&q));
        let back = parse(&text).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].text, "Ce este\n- o capcană?");
        assert_eq!(back[0].points, 2);
        assert_eq!(back[0].answer_type, AnswerType::Multiple);
        let opts: Vec<_> = back[0]
            .options
            .iter()
            .map(|o| (o.text.as_str(), o.is_correct))
            .collect();
        assert_eq!(opts, vec![("Da", true), ("Nu", false), ("Poate", true)]);
    }
}
