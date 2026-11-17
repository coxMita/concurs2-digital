//! Excel exports: student credentials and exam results.

use rust_xlsxwriter::{Color, Format, FormatAlign, FormatBorder, Workbook, XlsxError};

use crate::services::grading::{Outcome, fmt_points, grade_1_10};

fn header_format() -> Format {
    Format::new()
        .set_bold()
        .set_font_color(Color::White)
        .set_background_color(Color::RGB(0x8D2047))
        .set_align(FormatAlign::VerticalCenter)
        .set_border_bottom(FormatBorder::Thin)
}

pub struct Credential {
    pub name: String,
    pub username: String,
    pub password: String,
}

/// Name, Username, Password: one row per student, ready to print and cut.
pub fn credentials_xlsx(exam_title: &str, rows: &[Credential]) -> Result<Vec<u8>, XlsxError> {
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    ws.set_name("Conturi")?;
    let head = header_format();
    let cell = Format::new()
        .set_border(FormatBorder::Dashed)
        .set_font_size(13)
        .set_align(FormatAlign::VerticalCenter);
    let mono = cell.clone().set_font_name("Consolas").set_bold();
    let title = Format::new().set_bold().set_font_size(14);
    ws.write_string_with_format(0, 0, exam_title, &title)?;
    for (c, h) in ["Nume", "Utilizator", "Parolă"].iter().enumerate() {
        ws.write_string_with_format(2, c as u16, *h, &head)?;
    }
    for (i, r) in rows.iter().enumerate() {
        let row = 3 + i as u32;
        ws.set_row_height(row, 26)?;
        ws.write_string_with_format(row, 0, &r.name, &cell)?;
        ws.write_string_with_format(row, 1, &r.username, &mono)?;
        ws.write_string_with_format(row, 2, &r.password, &mono)?;
    }
    ws.set_column_width(0, 34)?;
    ws.set_column_width(1, 24)?;
    ws.set_column_width(2, 16)?;
    ws.set_freeze_panes(3, 0)?;
    wb.save_to_buffer()
}

/// One student row of the results sheet.
pub struct ResultRow {
    pub student_id: uuid::Uuid,
    pub name: String,
    pub username: String,
    pub percentage: Option<f64>,
    pub earned: Option<f64>,
    pub max: Option<i32>,
    pub status: &'static str,
    pub started_at: String,
    pub submitted_at: String,
    /// Outcome per question in canonical order (None = not part of the attempt).
    pub outcomes: Vec<Option<Outcome>>,
    /// Share of each question's points earned (0..=1), same order as `outcomes`.
    pub fractions: Vec<f64>,
    /// Chosen options per question, e.g. "A) 2; C) 4" (letters in canonical order).
    pub answers: Vec<String>,
    /// Integrity signals (None = never started).
    pub focus_losses: Option<i32>,
    pub away_seconds: Option<i64>,
    pub copy_attempts: Option<i32>,
    pub screenshot_attempts: Option<i32>,
}

pub struct QuestionInfo {
    pub text: String,
    pub answer_type: &'static str,
    pub points: i32,
    /// All options, one per line: "A) ...", "B) ..." (canonical order).
    pub options: String,
    /// Correct options, e.g. "A) București".
    pub correct: String,
}

/// Option letter by canonical position: 0 -> 'A'.
pub fn letter(i: usize) -> char {
    char::from(b'A' + u8::try_from(i % 26).unwrap_or(0))
}

/// "A) text" with long texts shortened for spreadsheet cells.
pub fn labeled(i: usize, text: &str, max_chars: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let short: String = if one_line.chars().count() > max_chars {
        let mut t: String = one_line.chars().take(max_chars.saturating_sub(1)).collect();
        t.push('…');
        t
    } else {
        one_line
    };
    format!("{}) {short}", letter(i))
}

pub fn results_xlsx(questions: &[QuestionInfo], rows: &[ResultRow]) -> Result<Vec<u8>, XlsxError> {
    let mut wb = Workbook::new();
    let head = header_format();
    let pct = Format::new().set_num_format("0.00");
    let ok = Format::new()
        .set_background_color(Color::RGB(0xC6EFCE))
        .set_font_color(Color::RGB(0x1F5A37));
    let bad = Format::new()
        .set_background_color(Color::RGB(0xF4C7C3))
        .set_font_color(Color::RGB(0x7F2C1F));
    let none = Format::new()
        .set_background_color(Color::RGB(0xE7E9EB))
        .set_font_color(Color::RGB(0x5D7480));
    let warn = Format::new()
        .set_background_color(Color::RGB(0xFBF3D1))
        .set_font_color(Color::RGB(0x4A3418));
    let wrap = Format::new().set_text_wrap().set_align(FormatAlign::Top);

    // Sheet 1: Rezultate
    {
        let ws = wb.add_worksheet();
        ws.set_name("Rezultate")?;
        let fixed = [
            "Nume",
            "Utilizator",
            "Nota (%)",
            "Nota (1–10)",
            "Punctaj",
            "Status",
            "Început la",
            "Terminat la",
        ];
        for (c, h) in fixed.iter().enumerate() {
            ws.write_string_with_format(0, c as u16, *h, &head)?;
        }
        let q0 = fixed.len() as u16;
        for i in 0..questions.len() {
            ws.write_string_with_format(0, q0 + i as u16, format!("Î{}", i + 1), &head)?;
        }
        // Integrity columns come after the questions.
        let g0 = q0 + questions.len() as u16;
        for (j, h) in [
            "Ieșiri din pagină",
            "Timp în afara paginii (s)",
            "Încercări de copiere",
            "Capturi de ecran (PrintScreen)",
        ]
        .iter()
        .enumerate()
        {
            ws.write_string_with_format(0, g0 + j as u16, *h, &head)?;
        }
        for (r, row) in rows.iter().enumerate() {
            let r = r as u32 + 1;
            ws.write_string(r, 0, &row.name)?;
            ws.write_string(r, 1, &row.username)?;
            if let Some(p) = row.percentage {
                ws.write_number_with_format(r, 2, p, &pct)?;
                ws.write_number_with_format(r, 3, grade_1_10(p), &pct)?;
            }
            if let (Some(e), Some(m)) = (row.earned, row.max) {
                ws.write_string(r, 4, format!("{} / {m}", fmt_points(e)))?;
            }
            ws.write_string(r, 5, row.status)?;
            ws.write_string(r, 6, &row.started_at)?;
            ws.write_string(r, 7, &row.submitted_at)?;
            for (i, o) in row.outcomes.iter().enumerate() {
                let c = q0 + i as u16;
                let chosen = row.answers.get(i).map(String::as_str).unwrap_or("");
                match o {
                    Some(Outcome::Correct) => {
                        ws.write_string_with_format(r, c, format!("Corect: {chosen}"), &ok)?
                    }
                    Some(Outcome::Partial) => {
                        let share = row.fractions.get(i).copied().unwrap_or(0.0) * 100.0;
                        ws.write_string_with_format(
                            r,
                            c,
                            format!("Parțial {}%: {chosen}", fmt_points(share)),
                            &warn,
                        )?
                    }
                    Some(Outcome::Wrong) => {
                        ws.write_string_with_format(r, c, format!("Greșit: {chosen}"), &bad)?
                    }
                    Some(Outcome::Unanswered) => {
                        ws.write_string_with_format(r, c, "Fără răspuns", &none)?
                    }
                    None => ws,
                };
            }
            let ints = [
                row.focus_losses.map(i64::from),
                row.away_seconds,
                row.copy_attempts.map(i64::from),
                row.screenshot_attempts.map(i64::from),
            ];
            for (j, v) in ints.iter().enumerate() {
                if let Some(v) = v {
                    let c = g0 + j as u16;
                    if *v > 0 {
                        ws.write_number_with_format(r, c, *v as f64, &warn)?;
                    } else {
                        ws.write_number(r, c, *v as f64)?;
                    }
                }
            }
        }
        ws.set_column_width(0, 28)?;
        ws.set_column_width(1, 18)?;
        ws.set_column_width(2, 10)?;
        ws.set_column_width(3, 11)?;
        ws.set_column_width(4, 10)?;
        ws.set_column_width(5, 15)?;
        ws.set_column_width(6, 17)?;
        ws.set_column_width(7, 17)?;
        for i in 0..questions.len() {
            ws.set_column_width(q0 + i as u16, 26)?;
        }
        for j in 0..4 {
            ws.set_column_width(g0 + j, 16)?;
        }
        ws.set_freeze_panes(1, 1)?;
    }

    // Sheet 2: Întrebări
    {
        let ws = wb.add_worksheet();
        ws.set_name("Întrebări")?;
        for (c, h) in [
            "Nr.",
            "Întrebare",
            "Tip răspuns",
            "Punctaj",
            "Variante",
            "Răspuns(uri) corect(e)",
        ]
        .iter()
        .enumerate()
        {
            ws.write_string_with_format(0, c as u16, *h, &head)?;
        }
        for (i, q) in questions.iter().enumerate() {
            let r = i as u32 + 1;
            ws.write_string(r, 0, format!("Î{}", i + 1))?;
            ws.write_string_with_format(r, 1, &q.text, &wrap)?;
            ws.write_string(r, 2, q.answer_type)?;
            ws.write_number(r, 3, f64::from(q.points))?;
            ws.write_string_with_format(r, 4, &q.options, &wrap)?;
            ws.write_string_with_format(r, 5, &q.correct, &wrap)?;
        }
        ws.set_column_width(0, 6)?;
        ws.set_column_width(1, 60)?;
        ws.set_column_width(2, 26)?;
        ws.set_column_width(3, 9)?;
        ws.set_column_width(4, 45)?;
        ws.set_column_width(5, 35)?;
        ws.set_freeze_panes(1, 1)?;
    }

    // Sheet 3: Statistici
    {
        let ws = wb.add_worksheet();
        ws.set_name("Statistici")?;
        for (c, h) in [
            "Nr.",
            "Întrebare",
            "Corecte",
            "Greșite",
            "Fără răspuns",
            "Corect (%)",
            "Parțiale",
            "Punctaj mediu (%)",
        ]
        .iter()
        .enumerate()
        {
            ws.write_string_with_format(0, c as u16, *h, &head)?;
        }
        for (i, q) in questions.iter().enumerate() {
            let r = i as u32 + 1;
            let (mut c, mut part, mut w, mut n) = (0u32, 0u32, 0u32, 0u32);
            let mut share_sum = 0.0;
            for row in rows {
                let o = row.outcomes.get(i).copied().flatten();
                if o.is_some() {
                    share_sum += row.fractions.get(i).copied().unwrap_or(0.0);
                }
                match o {
                    Some(Outcome::Correct) => c += 1,
                    Some(Outcome::Partial) => part += 1,
                    Some(Outcome::Wrong) => w += 1,
                    Some(Outcome::Unanswered) => n += 1,
                    None => {}
                }
            }
            // Partial answers count as answered but not as fully correct.
            let total = c + part + w + n;
            ws.write_string(r, 0, format!("Î{}", i + 1))?;
            ws.write_string_with_format(r, 1, &q.text, &wrap)?;
            ws.write_number(r, 2, f64::from(c))?;
            ws.write_number(r, 3, f64::from(w))?;
            ws.write_number(r, 4, f64::from(n))?;
            let p = if total == 0 {
                0.0
            } else {
                (f64::from(c) / f64::from(total) * 10_000.0).round() / 100.0
            };
            ws.write_number_with_format(r, 5, p, &pct)?;
            ws.write_number(r, 6, f64::from(part))?;
            let avg = if total == 0 {
                0.0
            } else {
                (share_sum / f64::from(total) * 10_000.0).round() / 100.0
            };
            ws.write_number_with_format(r, 7, avg, &pct)?;
        }
        ws.set_column_width(0, 6)?;
        ws.set_column_width(1, 60)?;
        for c in 2..8 {
            ws.set_column_width(c, 13)?;
        }
        ws.set_freeze_panes(1, 1)?;
    }

    wb.save_to_buffer()
}
