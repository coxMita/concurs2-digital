//! Test harness: builds the real router on a per-test database and drives it with
//! an in-process client that keeps the session cookie and CSRF token.

#![allow(dead_code)]

use std::path::PathBuf;

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Method, Request, StatusCode, header},
};
use concurs::{
    app,
    config::Config,
    db::{
        exams::{self, ExamInput, ExamStatus, ResultVisibility},
        questions::{self, AnswerType, OptionInput, QuestionInput},
        students::{self, NewStudent},
    },
    state::AppState,
};
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt;
use tower_sessions::cookie::Key;
use tower_sessions_sqlx_store::PostgresStore;
use uuid::Uuid;

pub struct TestApp {
    pub router: Router,
    pub state: AppState,
}

pub fn test_config() -> Config {
    let mut c = Config::from_env();
    c.cookie_secure = false;
    c.argon2_memory_kib = 1024;
    c.argon2_iterations = 1;
    c.setup_token = Some("test-setup-token".into());
    c.upload_dir = std::env::temp_dir().join(format!("concurs-test-{}", Uuid::new_v4()));
    c.static_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static");
    c
}

pub async fn spawn(pool: PgPool) -> TestApp {
    let store = PostgresStore::new(pool.clone());
    store.migrate().await.unwrap();
    let state = AppState::new(pool, test_config());
    let router = app::build(state.clone(), store, Key::generate());
    TestApp { router, state }
}

pub struct Resp {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    pub fn location(&self) -> Option<String> {
        self.headers
            .get(header::LOCATION)
            .map(|v| v.to_str().unwrap().to_string())
    }
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .map(|v| v.to_str().unwrap().to_string())
    }
}

/// A browser-like client: one cookie jar, remembers the latest CSRF token.
pub struct Client {
    router: Router,
    cookie: Option<String>,
    pub csrf: String,
}

impl Client {
    pub fn new(app: &TestApp) -> Self {
        Self {
            router: app.router.clone(),
            cookie: None,
            csrf: String::new(),
        }
    }

    async fn send(&mut self, req: Request<Body>) -> Resp {
        let res = self.router.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        for v in headers.get_all(header::SET_COOKIE) {
            let v = v.to_str().unwrap();
            let pair = v.split(';').next().unwrap();
            if v.contains("Max-Age=0") {
                self.cookie = None;
            } else {
                self.cookie = Some(pair.to_string());
            }
        }
        let body = res.into_body().collect().await.unwrap().to_bytes().to_vec();
        let text = String::from_utf8_lossy(&body);
        if let Some(i) = text.find("name=\"csrf-token\" content=\"") {
            let rest = &text[i + 27..];
            self.csrf = rest[..rest.find('"').unwrap()].to_string();
        } else if let Some(i) = text.find("name=\"_csrf\" value=\"") {
            let rest = &text[i + 20..];
            self.csrf = rest[..rest.find('"').unwrap()].to_string();
        }
        Resp {
            status,
            headers,
            body,
        }
    }

    fn base(&self, method: Method, path: &str) -> axum::http::request::Builder {
        let mut b = Request::builder()
            .method(method)
            .uri(path)
            .header("x-forwarded-for", "10.0.0.1");
        if let Some(c) = &self.cookie {
            b = b.header(header::COOKIE, c);
        }
        b
    }

    pub async fn get(&mut self, path: &str) -> Resp {
        let req = self.base(Method::GET, path).body(Body::empty()).unwrap();
        self.send(req).await
    }

    /// Regular form POST including the `_csrf` field.
    pub async fn post(&mut self, path: &str, fields: &[(&str, &str)]) -> Resp {
        let mut all: Vec<(&str, &str)> = vec![("_csrf", self.csrf.as_str())];
        all.extend_from_slice(fields);
        let body = serde_urlencoded::to_string(&all).unwrap();
        let req = self
            .base(Method::POST, path)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap();
        self.send(req).await
    }

    /// Form POST without any CSRF token.
    pub async fn post_no_csrf(&mut self, path: &str, fields: &[(&str, &str)]) -> Resp {
        let body = serde_urlencoded::to_string(fields).unwrap();
        let req = self
            .base(Method::POST, path)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap();
        self.send(req).await
    }

    /// HTMX-style POST: CSRF token in the header.
    pub async fn hx_post(&mut self, path: &str, fields: &[(&str, &str)]) -> Resp {
        let body = serde_urlencoded::to_string(fields).unwrap();
        let req = self
            .base(Method::POST, path)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header("hx-request", "true")
            .header("x-csrf-token", self.csrf.clone())
            .body(Body::from(body))
            .unwrap();
        self.send(req).await
    }
}

// ---- Fixtures --------------------------------------------------------------------

pub fn q(text: &str, t: AnswerType, points: i32, opts: &[(&str, bool)]) -> QuestionInput {
    QuestionInput {
        text: text.into(),
        points,
        answer_type: t,
        options: opts
            .iter()
            .map(|(t, c)| OptionInput {
                id: None,
                text: (*t).into(),
                is_correct: *c,
            })
            .collect(),
    }
}

/// Creates an exam with the given questions and status.
pub async fn exam(
    app: &TestApp,
    status: ExamStatus,
    visibility: ResultVisibility,
    qs: Vec<QuestionInput>,
) -> Uuid {
    let db = &app.state.db;
    let id = exams::create(
        db,
        &ExamInput {
            title: "Test",
            description: "",
            duration_minutes: 30,
            result_visibility: visibility,
        },
    )
    .await
    .unwrap();
    let items: Vec<_> = qs.into_iter().map(|q| (q, None)).collect();
    questions::insert_many(db, id, &items).await.unwrap();
    let mut tx = db.begin().await.unwrap();
    exams::set_status(&mut tx, id, status).await.unwrap();
    tx.commit().await.unwrap();
    app.state.cache.invalidate(id).await;
    id
}

/// Creates a named student with the given password.
pub async fn student(app: &TestApp, exam_id: Uuid, username: &str, password: &str) {
    let hash = app.state.hasher.hash(password.into()).await.unwrap();
    students::insert_many(
        &app.state.db,
        exam_id,
        &[NewStudent {
            full_name: Some(format!("Elev {username}")),
            username: username.into(),
            password_hash: hash,
        }],
    )
    .await
    .unwrap();
}

/// Logs a student in (fresh client) and opens the exam.
pub async fn student_client(app: &TestApp, username: &str, password: &str) -> Client {
    let mut c = Client::new(app);
    c.get("/").await;
    let r = c
        .post("/", &[("username", username), ("password", password)])
        .await;
    assert_eq!(
        r.status,
        StatusCode::SEE_OTHER,
        "login failed for {username}"
    );
    c
}

pub async fn admin_client(app: &TestApp) -> Client {
    let hash = app.state.hasher.hash("parola123".into()).await.unwrap();
    concurs::db::admins::create(&app.state.db, "prof", &hash)
        .await
        .unwrap();
    let mut c = Client::new(app);
    c.get("/admin/login").await;
    let r = c
        .post(
            "/admin/login",
            &[("username", "prof"), ("password", "parola123")],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    c.get("/admin/teste").await;
    c
}

/// Extracts all values of `name="<attr>" value="..."` from HTML.
pub fn values(html: &str, name: &str) -> Vec<String> {
    let needle = format!("name=\"{name}\" value=\"");
    html.match_indices(&needle)
        .map(|(i, _)| {
            let rest = &html[i + needle.len()..];
            rest[..rest.find('"').unwrap()].to_string()
        })
        .collect()
}
