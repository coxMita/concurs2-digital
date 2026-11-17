//! End-to-end tests against a real (per-test) Postgres database.
//! Run with DATABASE_URL pointing at a server where the user may create databases.

mod common;

use std::io::Cursor;

use axum::http::StatusCode;
use calamine::{Data, Reader, Xlsx, open_workbook_from_rs};
use common::*;
use concurs::db::{
    exams::{ExamStatus, ResultVisibility},
    questions::AnswerType::{Multiple, Single},
};
use sqlx::PgPool;
use uuid::Uuid;

/// Ids of a question (by text) and of some of its options (by text).
async fn ids(pool: &PgPool, exam: Uuid, qtext: &str, opts: &[&str]) -> (Uuid, Vec<Uuid>) {
    let qid: Uuid = sqlx::query_scalar("SELECT id FROM questions WHERE exam_id = $1 AND text = $2")
        .bind(exam)
        .bind(qtext)
        .fetch_one(pool)
        .await
        .unwrap();
    let opts: Vec<String> = opts.iter().map(|s| s.to_string()).collect();
    let oids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM options WHERE question_id = $1 AND text = ANY($2)")
            .bind(qid)
            .bind(&opts)
            .fetch_all(pool)
            .await
            .unwrap();
    (qid, oids)
}

async fn answer(c: &mut Client, pool: &PgPool, exam: Uuid, qtext: &str, opts: &[&str]) -> Resp {
    let (qid, oids) = ids(pool, exam, qtext, opts).await;
    let q = qid.to_string();
    let o: Vec<String> = oids.iter().map(Uuid::to_string).collect();
    let mut fields = vec![("q", q.as_str())];
    for x in &o {
        fields.push(("opt", x.as_str()));
    }
    c.hx_post("/test/raspuns", &fields).await
}

async fn selections_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM answer_selections")
        .fetch_one(pool)
        .await
        .unwrap()
}

fn three_questions() -> Vec<concurs::db::questions::QuestionInput> {
    vec![
        q("Q1", Single, 2, &[("A", true), ("B", false), ("C", false)]),
        q("Q2", Multiple, 1, &[("A", true), ("B", true), ("C", false)]),
        q("Q3", Multiple, 1, &[("A", true), ("B", true), ("C", false)]),
    ]
}

// ---- Setup ---------------------------------------------------------------------

#[sqlx::test]
async fn setup_is_disabled_once_an_admin_exists(pool: PgPool) {
    let app = spawn(pool).await;
    let mut c = Client::new(&app);
    assert_eq!(c.get("/setup").await.status, StatusCode::OK);

    let wrong = c
        .post(
            "/setup",
            &[
                ("token", "nope"),
                ("username", "prof"),
                ("password", "parola123"),
                ("password_confirm", "parola123"),
            ],
        )
        .await;
    assert_eq!(wrong.status, StatusCode::OK);
    assert!(wrong.text().contains("Codul de configurare este greșit"));

    let ok = c
        .post(
            "/setup",
            &[
                ("token", "test-setup-token"),
                ("username", "prof"),
                ("password", "parola123"),
                ("password_confirm", "parola123"),
            ],
        )
        .await;
    assert_eq!(ok.status, StatusCode::SEE_OTHER);
    assert_eq!(ok.location().as_deref(), Some("/admin"));

    // Logged in right away.
    assert_eq!(c.get("/admin/teste").await.status, StatusCode::OK);

    let mut other = Client::new(&app);
    assert_eq!(other.get("/setup").await.status, StatusCode::NOT_FOUND);
    other.get("/").await;
    let again = other
        .post(
            "/setup",
            &[
                ("token", "test-setup-token"),
                ("username", "evil"),
                ("password", "parola123"),
                ("password_confirm", "parola123"),
            ],
        )
        .await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
    let admins: i64 = sqlx::query_scalar("SELECT count(*) FROM admins")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(admins, 1);
}

#[sqlx::test]
async fn state_changing_requests_require_csrf(pool: PgPool) {
    let app = spawn(pool).await;
    let mut c = Client::new(&app);
    c.get("/admin/login").await;
    let r = c
        .post_no_csrf(
            "/admin/login",
            &[("username", "prof"), ("password", "parola123")],
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

// ---- Grading -------------------------------------------------------------------

#[sqlx::test]
async fn grading_single_and_multiple_with_partial_selections(pool: PgPool) {
    let app = spawn(pool.clone()).await;
    let exam = exam(
        &app,
        ExamStatus::Open,
        ResultVisibility::Score,
        three_questions(),
    )
    .await;
    student(&app, exam, "ana", "pw-ana-1").await;
    student(&app, exam, "ion", "pw-ion-1").await;

    // ana: Q1 correct (2p), Q2 exact set (1p), Q3 partial (0p) -> 3/4 = 75%
    let mut a = student_client(&app, "ana", "pw-ana-1").await;
    a.get("/test").await;
    assert_eq!(
        answer(&mut a, &pool, exam, "Q1", &["A"]).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        answer(&mut a, &pool, exam, "Q2", &["A", "B"]).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        answer(&mut a, &pool, exam, "Q3", &["A"]).await.status,
        StatusCode::NO_CONTENT
    );
    let r = a.post("/test/finalizeaza", &[]).await;
    assert_eq!(r.location().as_deref(), Some("/rezultat"));

    // ion: Q1 wrong, Q2 superset (0p), Q3 exact (1p) -> 1/4 = 25%
    let mut i = student_client(&app, "ion", "pw-ion-1").await;
    i.get("/test").await;
    answer(&mut i, &pool, exam, "Q1", &["B"]).await;
    answer(&mut i, &pool, exam, "Q2", &["A", "B", "C"]).await;
    answer(&mut i, &pool, exam, "Q3", &["B", "A"]).await;
    i.post("/test/finalizeaza", &[]).await;

    let rows: Vec<(String, i32, i32, f64)> = sqlx::query_as(
        "SELECT s.username, a.earned_points, a.max_points, a.percentage
         FROM attempts a JOIN students s ON s.id = a.student_id ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows[0], ("ana".into(), 3, 4, 75.0));
    assert_eq!(rows[1], ("ion".into(), 1, 4, 25.0));

    let page = a.get("/rezultat").await.text();
    assert!(page.contains("75,00%"), "score shown");
}

// ---- Shuffle -------------------------------------------------------------------

async fn displayed_order(c: &mut Client, n: usize) -> Vec<(String, Vec<String>)> {
    let mut out = vec![];
    for i in 1..=n {
        let html = c.get(&format!("/test?q={i}")).await.text();
        out.push((values(&html, "q").remove(0), values(&html, "opt")));
    }
    out
}

#[sqlx::test]
async fn shuffle_is_stored_and_stable(pool: PgPool) {
    let app = spawn(pool.clone()).await;
    let qs = (1..=12)
        .map(|i| {
            q(
                &format!("Întrebarea {i}"),
                Single,
                1,
                &[("a", true), ("b", false), ("c", false), ("d", false)],
            )
        })
        .collect();
    let exam = exam(&app, ExamStatus::Open, ResultVisibility::Score, qs).await;
    student(&app, exam, "ana", "pw-ana-1").await;

    let mut c = student_client(&app, "ana", "pw-ana-1").await;
    let first = displayed_order(&mut c, 12).await;
    let again = displayed_order(&mut c, 12).await;
    assert_eq!(first, again, "stable across refreshes");

    let mut relogged = student_client(&app, "ana", "pw-ana-1").await;
    assert_eq!(
        first,
        displayed_order(&mut relogged, 12).await,
        "stable across re-login"
    );

    // Matches what is stored in the database.
    let stored: Vec<(Uuid, Vec<Uuid>)> = sqlx::query_as(
        "SELECT question_id, option_order FROM attempt_questions ORDER BY display_position",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let stored: Vec<(String, Vec<String>)> = stored
        .into_iter()
        .map(|(q, o)| (q.to_string(), o.iter().map(Uuid::to_string).collect()))
        .collect();
    assert_eq!(first, stored);

    // It is actually shuffled (12! orders: identity is practically impossible).
    let canonical: Vec<String> =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM questions ORDER BY position")
            .fetch_all(&pool)
            .await
            .unwrap()
            .iter()
            .map(Uuid::to_string)
            .collect();
    let shown: Vec<String> = first.iter().map(|x| x.0.clone()).collect();
    assert_ne!(shown, canonical);
}

// ---- Answer validation -----------------------------------------------------------

#[sqlx::test]
async fn answers_after_the_deadline_are_rejected(pool: PgPool) {
    let app = spawn(pool.clone()).await;
    let exam = exam(
        &app,
        ExamStatus::Open,
        ResultVisibility::Score,
        three_questions(),
    )
    .await;
    student(&app, exam, "ana", "pw-ana-1").await;
    let mut c = student_client(&app, "ana", "pw-ana-1").await;
    c.get("/test").await;
    sqlx::query("UPDATE attempts SET deadline = now() - interval '5 seconds'")
        .execute(&pool)
        .await
        .unwrap();

    let r = answer(&mut c, &pool, exam, "Q1", &["A"]).await;
    assert_eq!(r.header("hx-redirect").as_deref(), Some("/rezultat"));
    assert_eq!(selections_count(&pool).await, 0);

    // Finalized at the deadline, with the score computed.
    let (at_deadline, pct): (bool, Option<f64>) =
        sqlx::query_as("SELECT submitted_at = deadline, percentage FROM attempts")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(at_deadline);
    assert_eq!(pct, Some(0.0));
}

#[sqlx::test]
async fn the_database_guard_rejects_late_writes_even_if_the_app_check_passes(pool: PgPool) {
    let app = spawn(pool.clone()).await;
    let exam = exam(
        &app,
        ExamStatus::Open,
        ResultVisibility::Score,
        three_questions(),
    )
    .await;
    student(&app, exam, "ana", "pw-ana-1").await;
    let mut c = student_client(&app, "ana", "pw-ana-1").await;
    c.get("/test").await;
    let attempt: Uuid = sqlx::query_scalar("SELECT id FROM attempts")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE attempts SET deadline = now() - interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    let (qid, oids) = ids(&pool, exam, "Q1", &["A"]).await;
    let saved = concurs::db::attempts::save_answer(&pool, attempt, qid, &oids)
        .await
        .unwrap();
    assert!(!saved);
    assert_eq!(selections_count(&pool).await, 0);
}

#[sqlx::test]
async fn foreign_options_and_questions_are_rejected(pool: PgPool) {
    let app = spawn(pool.clone()).await;
    let exam_a = exam(
        &app,
        ExamStatus::Open,
        ResultVisibility::Score,
        three_questions(),
    )
    .await;
    let exam_b = exam(
        &app,
        ExamStatus::Open,
        ResultVisibility::Score,
        vec![q("Other", Single, 1, &[("X", true), ("Y", false)])],
    )
    .await;
    student(&app, exam_a, "ana", "pw-ana-1").await;
    let mut c = student_client(&app, "ana", "pw-ana-1").await;
    c.get("/test").await;

    // Option of another question of the same exam.
    let (q1, _) = ids(&pool, exam_a, "Q1", &[]).await;
    let (_, q2_opts) = ids(&pool, exam_a, "Q2", &["A"]).await;
    let r = c
        .hx_post(
            "/test/raspuns",
            &[("q", &q1.to_string()), ("opt", &q2_opts[0].to_string())],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // Question (and option) of another exam: not part of this student's attempt.
    let (other_q, other_opts) = ids(&pool, exam_b, "Other", &["X"]).await;
    let r = c
        .hx_post(
            "/test/raspuns",
            &[
                ("q", &other_q.to_string()),
                ("opt", &other_opts[0].to_string()),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // Garbage ids.
    let r = c.hx_post("/test/raspuns", &[("q", "not-a-uuid")]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // Another student's attempt cannot be targeted: the attempt comes from the session,
    // and the SQL guard ties the question to that attempt.
    student(&app, exam_b, "ion", "pw-ion-1").await;
    let mut ion = student_client(&app, "ion", "pw-ion-1").await;
    ion.get("/test").await;
    let ion_attempt: Uuid = sqlx::query_scalar(
        "SELECT a.id FROM attempts a JOIN students s ON s.id = a.student_id WHERE s.username = 'ion'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let (q1, q1_opts) = ids(&pool, exam_a, "Q1", &["A"]).await;
    let saved = concurs::db::attempts::save_answer(&pool, ion_attempt, q1, &q1_opts)
        .await
        .unwrap();
    assert!(
        !saved,
        "question of exam A cannot be written into an exam B attempt"
    );

    assert_eq!(selections_count(&pool).await, 0);
}

#[sqlx::test]
async fn single_answer_question_rejects_two_options(pool: PgPool) {
    let app = spawn(pool.clone()).await;
    let exam = exam(
        &app,
        ExamStatus::Open,
        ResultVisibility::Score,
        three_questions(),
    )
    .await;
    student(&app, exam, "ana", "pw-ana-1").await;
    let mut c = student_client(&app, "ana", "pw-ana-1").await;
    c.get("/test").await;
    let r = answer(&mut c, &pool, exam, "Q1", &["A", "B"]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(selections_count(&pool).await, 0);
    // Multiple-answer questions accept several.
    let r = answer(&mut c, &pool, exam, "Q2", &["A", "B"]).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(selections_count(&pool).await, 2);
}

// ---- Confidentiality of the answer key ---------------------------------------------

fn strip_volatile(html: &str) -> String {
    // The countdown attributes change with time.
    html.lines()
        .filter(|l| !l.contains("data-deadline") && !l.contains("data-remaining"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[sqlx::test]
async fn answer_key_never_reaches_the_student_before_submit(pool: PgPool) {
    let app = spawn(pool.clone()).await;
    let exam = exam(
        &app,
        ExamStatus::Open,
        ResultVisibility::ScoreAndAnswers,
        three_questions(),
    )
    .await;
    student(&app, exam, "ana", "pw-ana-1").await;
    let mut c = student_client(&app, "ana", "pw-ana-1").await;
    c.get("/test").await;
    answer(&mut c, &pool, exam, "Q1", &["B"]).await;

    let mut before = vec![];
    for i in 1..=3 {
        let html = c.get(&format!("/test?q={i}")).await.text();
        assert!(!html.contains("is_correct"));
        assert!(!html.contains("option--correct"));
        assert!(!html.contains("option--wrong"));
        before.push(strip_volatile(&html));
    }
    before.push(strip_volatile(&c.get("/test/finalizeaza").await.text()));

    // Flip the whole answer key: if anything derived from it were rendered, the pages
    // would change.
    sqlx::query("UPDATE options SET is_correct = NOT is_correct")
        .execute(&pool)
        .await
        .unwrap();
    app.state.cache.invalidate(exam).await;
    let mut after = vec![];
    for i in 1..=3 {
        after.push(strip_volatile(&c.get(&format!("/test?q={i}")).await.text()));
    }
    after.push(strip_volatile(&c.get("/test/finalizeaza").await.text()));
    assert_eq!(before, after);

    // Results are not available before submitting.
    let r = c.get("/rezultat").await;
    assert_eq!(r.location().as_deref(), Some("/test"));
}

async fn submitted_result_page(visibility: ResultVisibility, pool: PgPool) -> String {
    let app = spawn(pool.clone()).await;
    let exam = exam(&app, ExamStatus::Open, visibility, three_questions()).await;
    student(&app, exam, "ana", "pw-ana-1").await;
    let mut c = student_client(&app, "ana", "pw-ana-1").await;
    c.get("/test").await;
    answer(&mut c, &pool, exam, "Q1", &["A"]).await;
    c.post("/test/finalizeaza", &[]).await;
    c.get("/rezultat").await.text()
}

#[sqlx::test]
async fn visibility_none_shows_only_confirmation(pool: PgPool) {
    let html = submitted_result_page(ResultVisibility::None, pool).await;
    assert!(html.contains("Testul a fost trimis"));
    assert!(!html.contains("score-ring"));
    assert!(!html.contains("Nota:"));
    assert!(!html.contains("option--correct"));
    assert!(!html.contains("Răspuns corect"));
}

#[sqlx::test]
async fn visibility_score_shows_percentage_only(pool: PgPool) {
    let html = submitted_result_page(ResultVisibility::Score, pool).await;
    assert!(html.contains("Nota: 50,00%"));
    assert!(!html.contains("option--correct"));
    assert!(!html.contains("Răspuns corect"));
    assert!(!html.contains("Răspunsurile tale"));
}

#[sqlx::test]
async fn visibility_score_and_answers_shows_review(pool: PgPool) {
    let html = submitted_result_page(ResultVisibility::ScoreAndAnswers, pool).await;
    assert!(html.contains("Nota: 50,00%"));
    assert!(html.contains("Răspunsurile tale"));
    assert!(html.contains("option--correct"));
    assert!(html.contains("Răspuns corect"));
}

// ---- Sessions, closing ---------------------------------------------------------------

#[sqlx::test]
async fn a_new_login_invalidates_the_previous_session(pool: PgPool) {
    let app = spawn(pool.clone()).await;
    let exam = exam(
        &app,
        ExamStatus::Open,
        ResultVisibility::Score,
        three_questions(),
    )
    .await;
    student(&app, exam, "ana", "pw-ana-1").await;
    let mut first = student_client(&app, "ana", "pw-ana-1").await;
    assert_eq!(first.get("/test").await.status, StatusCode::OK);
    let mut second = student_client(&app, "ana", "pw-ana-1").await;
    assert_eq!(second.get("/test").await.status, StatusCode::OK);
    let r = first.get("/test").await;
    assert_eq!(r.location().as_deref(), Some("/"));
    let count: i32 = sqlx::query_scalar("SELECT login_count FROM attempts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count, 2,
        "re-login during the exam is recorded for the dashboard"
    );
}

#[sqlx::test]
async fn closing_an_exam_finalizes_attempts(pool: PgPool) {
    let app = spawn(pool.clone()).await;
    let exam = exam(
        &app,
        ExamStatus::Open,
        ResultVisibility::Score,
        three_questions(),
    )
    .await;
    student(&app, exam, "ana", "pw-ana-1").await;
    let mut s = student_client(&app, "ana", "pw-ana-1").await;
    s.get("/test").await;
    answer(&mut s, &pool, exam, "Q1", &["A"]).await;

    let mut admin = admin_client(&app).await;
    let r = admin
        .post(
            &format!("/admin/teste/{exam}/stare"),
            &[("status", "closed")],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let pct: Option<f64> =
        sqlx::query_scalar("SELECT percentage FROM attempts WHERE submitted_at IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(pct, Some(50.0));
    assert_eq!(
        s.get("/test").await.location().as_deref(),
        Some("/rezultat")
    );
}

#[sqlx::test]
async fn waiting_room_until_the_exam_opens(pool: PgPool) {
    let app = spawn(pool.clone()).await;
    let exam = exam(
        &app,
        ExamStatus::Draft,
        ResultVisibility::Score,
        three_questions(),
    )
    .await;
    student(&app, exam, "ana", "pw-ana-1").await;
    let mut s = student_client(&app, "ana", "pw-ana-1").await;
    assert_eq!(
        s.get("/test").await.location().as_deref(),
        Some("/asteptare")
    );
    assert!(
        s.get("/asteptare")
            .await
            .text()
            .contains("Testul nu a început încă")
    );
    assert_eq!(s.get("/asteptare/stare").await.header("hx-redirect"), None);

    let mut tx = pool.begin().await.unwrap();
    concurs::db::exams::set_status(&mut tx, exam, ExamStatus::Open)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    app.state.cache.invalidate(exam).await;
    assert_eq!(
        s.get("/asteptare/stare")
            .await
            .header("hx-redirect")
            .as_deref(),
        Some("/test")
    );
}

// ---- Excel export ----------------------------------------------------------------------

fn cell(r: &calamine::Range<Data>, row: u32, col: u32) -> String {
    match r.get_value((row, col)) {
        Some(Data::String(s)) => s.clone(),
        Some(Data::Float(f)) => format!("{f}"),
        Some(Data::Int(i)) => format!("{i}"),
        Some(Data::Empty) | None => String::new(),
        Some(other) => format!("{other}"),
    }
}

#[sqlx::test]
async fn results_excel_has_the_right_columns_and_values(pool: PgPool) {
    let app = spawn(pool.clone()).await;
    let exam = exam(
        &app,
        ExamStatus::Open,
        ResultVisibility::Score,
        vec![
            q("Q1", Single, 1, &[("A", true), ("B", false)]),
            q("Q2", Multiple, 1, &[("A", true), ("B", true), ("C", false)]),
            q("Q3", Single, 2, &[("A", true), ("B", false)]),
        ],
    )
    .await;
    student(&app, exam, "a1", "pw-a1-xx").await;
    student(&app, exam, "b2", "pw-b2-xx").await;
    let mut s = student_client(&app, "a1", "pw-a1-xx").await;
    s.get("/test").await;
    answer(&mut s, &pool, exam, "Q1", &["A"]).await; // correct
    answer(&mut s, &pool, exam, "Q2", &["A"]).await; // partial -> wrong
    s.post("/test/finalizeaza", &[]).await; // Q3 unanswered

    let mut admin = admin_client(&app).await;
    let r = admin
        .get(&format!("/admin/teste/{exam}/rezultate.xlsx"))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.header("content-type").unwrap().contains("spreadsheetml"));

    let mut wb: Xlsx<_> = open_workbook_from_rs(Cursor::new(r.body)).unwrap();
    assert_eq!(
        wb.sheet_names(),
        vec!["Rezultate", "Întrebări", "Statistici"]
    );

    let res = wb.worksheet_range("Rezultate").unwrap();
    let header: Vec<String> = (0..10).map(|c| cell(&res, 0, c)).collect();
    assert_eq!(
        header,
        [
            "Nume",
            "Utilizator",
            "Nota (%)",
            "Punctaj",
            "Status",
            "Început la",
            "Terminat la",
            "Î1",
            "Î2",
            "Î3"
        ]
    );
    // Row 1: a1 (sorted by name), 1 of 4 points -> 25%.
    assert_eq!(cell(&res, 1, 0), "Elev a1");
    assert_eq!(cell(&res, 1, 1), "a1");
    assert_eq!(res.get_value((1, 2)), Some(&Data::Float(25.0)));
    assert_eq!(cell(&res, 1, 3), "1 / 4");
    assert_eq!(cell(&res, 1, 4), "Terminat");
    assert!(!cell(&res, 1, 5).is_empty());
    assert!(!cell(&res, 1, 6).is_empty());
    assert_eq!(cell(&res, 1, 7), "Corect: A) A");
    assert_eq!(cell(&res, 1, 8), "Greșit: A) A");
    assert_eq!(cell(&res, 1, 9), "Fără răspuns");
    // Row 2: b2 never started.
    assert_eq!(cell(&res, 2, 1), "b2");
    assert_eq!(cell(&res, 2, 2), "");
    assert_eq!(cell(&res, 2, 4), "Neînceput");
    assert_eq!(cell(&res, 2, 7), "");

    let qs = wb.worksheet_range("Întrebări").unwrap();
    assert_eq!(cell(&qs, 1, 0), "Î1");
    assert_eq!(cell(&qs, 1, 1), "Q1");
    assert_eq!(cell(&qs, 2, 2), "Mai multe răspunsuri corecte");
    assert_eq!(cell(&qs, 2, 4), "A) A\nB) B\nC) C");
    assert_eq!(cell(&qs, 2, 5), "A) A; B) B");

    let st = wb.worksheet_range("Statistici").unwrap();
    assert_eq!(cell(&st, 0, 5), "Corect (%)");
    assert_eq!(st.get_value((1, 5)), Some(&Data::Float(100.0)));
    assert_eq!(st.get_value((2, 5)), Some(&Data::Float(0.0)));
}

#[sqlx::test]
async fn credentials_are_generated_and_hashed(pool: PgPool) {
    let app = spawn(pool.clone()).await;
    let exam = exam(
        &app,
        ExamStatus::Draft,
        ResultVisibility::Score,
        three_questions(),
    )
    .await;
    let mut admin = admin_client(&app).await;
    let r = admin
        .post(
            &format!("/admin/teste/{exam}/elevi/genereaza"),
            &[("names", "Ana Popescu\nAna Popescu\n"), ("count", "2")],
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let html = r.text();
    let users = values(&html, "u");
    let pws = values(&html, "p");
    assert_eq!(
        users,
        ["ana.popescu", "ana.popescu.2", "elev001", "elev002"]
    );
    assert!(pws.iter().all(|p| p.len() == 8));

    // Only hashes are stored.
    let hashes: Vec<String> = sqlx::query_scalar("SELECT password_hash FROM students")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(hashes.iter().all(|h| h.starts_with("$argon2id$")));
    assert!(!hashes.iter().any(|h| pws.contains(h)));

    // The generated credentials work.
    let mut s = student_client(&app, "elev002", &pws[3]).await;
    assert_eq!(s.get("/test").await.location().as_deref(), Some("/nume"));
}

// ---- Integrity signals -------------------------------------------------------------

#[sqlx::test]
async fn integrity_events_are_recorded_and_reported(pool: PgPool) {
    let app = spawn(pool.clone()).await;
    let exam = exam(
        &app,
        ExamStatus::Open,
        ResultVisibility::Score,
        three_questions(),
    )
    .await;
    student(&app, exam, "ana", "pw-ana-1").await;
    let mut s = student_client(&app, "ana", "pw-ana-1").await;
    let page = s.get("/test").await.text();
    assert!(page.contains("sunt înregistrate"), "students are warned");
    assert!(page.contains("class=\"watermark\""));
    assert!(
        page.contains("Elev ana · ana"),
        "watermark names the student"
    );

    // Leave twice in a row counts once; the time away is measured by the server.
    for kind in ["leave", "leave"] {
        let r = s.hx_post("/test/eveniment", &[("kind", kind)]).await;
        assert_eq!(r.status, StatusCode::NO_CONTENT);
    }
    sqlx::query("UPDATE attempts SET away_since = now() - interval '75 seconds'")
        .execute(&pool)
        .await
        .unwrap();
    s.hx_post("/test/eveniment", &[("kind", "return")]).await;
    s.hx_post("/test/eveniment", &[("kind", "copy")]).await;
    s.hx_post("/test/eveniment", &[("kind", "copy")]).await;
    s.hx_post("/test/eveniment", &[("kind", "screenshot")])
        .await;
    // Unknown kinds are ignored.
    assert_eq!(
        s.hx_post("/test/eveniment", &[("kind", "nonsense")])
            .await
            .status,
        StatusCode::NO_CONTENT
    );

    let (losses, away_ms, copies, shots, away_since): (i32, i64, i32, i32, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as(
            "SELECT focus_losses, focus_away_ms, copy_attempts, screenshot_attempts, away_since FROM attempts",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(losses, 1);
    assert!((74_000..80_000).contains(&away_ms), "away {away_ms} ms");
    assert_eq!((copies, shots), (2, 1));
    assert!(away_since.is_none());

    // Leaving and coming back by reloading the page (e.g. tab closed) also closes the interval.
    s.hx_post("/test/eveniment", &[("kind", "leave")]).await;
    s.get("/test").await;
    let (losses, away_since): (i32, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT focus_losses, away_since FROM attempts")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(losses, 2);
    assert!(away_since.is_none());

    // The teacher sees it live and in Excel.
    let mut admin = admin_client(&app).await;
    let live = admin
        .get(&format!("/admin/teste/{exam}/live/tabel"))
        .await
        .text();
    assert!(
        live.contains("2 (1 min 15 s)") || live.contains("2 (1 min 1"),
        "{live}"
    );
    assert!(live.contains("Copiere 2×"));
    assert!(live.contains("Captură 1×"));

    s.post("/test/finalizeaza", &[]).await;
    // After submitting, events are ignored.
    s.hx_post("/test/eveniment", &[("kind", "copy")]).await;
    let copies: i32 = sqlx::query_scalar("SELECT copy_attempts FROM attempts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(copies, 2);

    let x = admin
        .get(&format!("/admin/teste/{exam}/rezultate.xlsx"))
        .await;
    let mut wb: Xlsx<_> = open_workbook_from_rs(Cursor::new(x.body)).unwrap();
    let res = wb.worksheet_range("Rezultate").unwrap();
    // 7 fixed columns + 3 questions, then the integrity columns.
    assert_eq!(cell(&res, 0, 10), "Ieșiri din pagină");
    assert_eq!(cell(&res, 0, 11), "Timp în afara paginii (s)");
    assert_eq!(cell(&res, 0, 12), "Încercări de copiere");
    assert_eq!(cell(&res, 0, 13), "Capturi de ecran (PrintScreen)");
    assert_eq!(res.get_value((1, 10)), Some(&Data::Float(2.0)));
    assert_eq!(res.get_value((1, 12)), Some(&Data::Float(2.0)));
    assert_eq!(res.get_value((1, 13)), Some(&Data::Float(1.0)));
}

#[sqlx::test]
async fn reset_password_is_shown_under_the_student_once(pool: PgPool) {
    let app = spawn(pool.clone()).await;
    let exam = exam(
        &app,
        ExamStatus::Open,
        ResultVisibility::Score,
        three_questions(),
    )
    .await;
    student(&app, exam, "ana", "old-pass-1").await;
    let sid: Uuid = sqlx::query_scalar("SELECT id FROM students")
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut admin = admin_client(&app).await;
    admin.get(&format!("/admin/teste/{exam}/elevi")).await;
    let r = admin
        .post(&format!("/admin/teste/{exam}/elevi/{sid}/parola"), &[])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);

    let page = admin
        .get(&format!("/admin/teste/{exam}/elevi"))
        .await
        .text();
    let start = page
        .find("class=\"new-password\">")
        .expect("password shown")
        + 21;
    let pw = page[start..start + 8].to_string();
    assert!(
        page.contains("class=\"reset-row\""),
        "shown under the student's row"
    );

    // Shown only once.
    let again = admin
        .get(&format!("/admin/teste/{exam}/elevi"))
        .await
        .text();
    assert!(!again.contains("new-password\">"));

    // The new password works, the old one does not.
    let mut s = Client::new(&app);
    s.get("/").await;
    let old = s
        .post("/", &[("username", "ana"), ("password", "old-pass-1")])
        .await;
    assert_eq!(old.status, StatusCode::UNAUTHORIZED);
    student_client(&app, "ana", &pw).await;
}
