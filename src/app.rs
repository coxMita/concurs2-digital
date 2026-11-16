//! Router and middleware stack.

use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{HeaderName, HeaderValue, header},
    middleware,
    response::IntoResponse,
    routing::{get, post},
};
use tower::ServiceBuilder;
use tower_governor::{
    GovernorLayer, governor::GovernorConfigBuilder, key_extractor::SmartIpKeyExtractor,
};
use tower_http::{
    catch_panic::CatchPanicLayer, compression::CompressionLayer, limit::RequestBodyLimitLayer,
    services::ServeDir, set_header::SetResponseHeaderLayer, trace::TraceLayer,
};
use tower_sessions::{Expiry, SessionManagerLayer, cookie::Key, cookie::SameSite};
use tower_sessions_sqlx_store::PostgresStore;

use crate::{
    auth::csrf,
    error::AppError,
    routes::{self, admin},
    state::AppState,
};

const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
font-src 'self'; connect-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'; object-src 'none'";

pub fn build(state: AppState, store: PostgresStore, key: Key) -> Router {
    let sessions = SessionManagerLayer::new(store)
        .with_name("concurs_sid")
        .with_secure(state.config.cookie_secure)
        .with_http_only(true)
        .with_same_site(SameSite::Lax)
        .with_expiry(Expiry::OnInactivity(time::Duration::hours(12)))
        .with_signed(key);

    // Generous per-IP limit on login POSTs only (a whole school may share one IP).
    let per_ip = state.config.login_per_ip_per_min.max(1);
    let governor = Arc::new(
        GovernorConfigBuilder::default()
            .key_extractor(SmartIpKeyExtractor)
            .period(Duration::from_millis(60_000 / u64::from(per_ip)))
            .burst_size(per_ip)
            .finish()
            .expect("valid governor config"),
    );
    let limiter = governor.limiter().clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            limiter.retain_recent();
        }
    });
    let login_limit =
        GovernorLayer::new(governor).error_handler(|_| AppError::TooManyRequests.into_response());

    let login_posts = Router::new()
        .route("/", post(routes::student::login))
        .route("/admin/login", post(routes::auth::admin_login))
        .layer(login_limit);

    let admin_routes = admin::router();

    let static_files = Router::new()
        .nest_service("/static", ServeDir::new(&state.config.static_dir))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        ));
    let uploads = Router::new()
        .nest_service("/uploads", ServeDir::new(&state.config.upload_dir))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=86400"),
        ));

    let body_limit = state.config.max_upload_bytes + 64 * 1024;

    let dynamic = Router::new()
        .route("/", get(routes::student::login_page))
        .route(
            "/setup",
            get(routes::setup::show).post(routes::setup::submit),
        )
        .route("/admin/login", get(routes::auth::admin_login_page))
        .route("/logout", post(routes::auth::logout))
        .route(
            "/nume",
            get(routes::student::name_page).post(routes::student::name_submit),
        )
        .route("/asteptare", get(routes::student::waiting))
        .route("/asteptare/stare", get(routes::student::waiting_status))
        .route("/test", get(routes::student::exam))
        .route("/test/raspuns", post(routes::student::save_answer))
        .route(
            "/test/finalizeaza",
            get(routes::student::confirm).post(routes::student::submit),
        )
        .route("/rezultat", get(routes::student::result))
        .merge(login_posts)
        .nest("/admin", admin_routes)
        .fallback(|| async { AppError::NotFound })
        .layer(middleware::from_fn(csrf::verify))
        .layer(DefaultBodyLimit::max(body_limit))
        .layer(sessions)
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ));

    let hsts = if state.config.hsts() {
        HeaderValue::from_static("max-age=31536000")
    } else {
        HeaderValue::from_static("max-age=0")
    };

    Router::new()
        .merge(static_files)
        .merge(uploads)
        .merge(dynamic)
        .route("/healthz", get(|| async { "ok" }))
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(CatchPanicLayer::custom(|_| {
                    AppError::Internal("handler panicked".into()).into_response()
                }))
                // Hard cap; the friendlier per-extractor limit is DefaultBodyLimit above.
                .layer(RequestBodyLimitLayer::new(body_limit * 2))
                .layer(CompressionLayer::new())
                .layer(SetResponseHeaderLayer::if_not_present(
                    header::CONTENT_SECURITY_POLICY,
                    HeaderValue::from_static(CSP),
                ))
                .layer(SetResponseHeaderLayer::overriding(
                    header::X_FRAME_OPTIONS,
                    HeaderValue::from_static("DENY"),
                ))
                .layer(SetResponseHeaderLayer::overriding(
                    header::X_CONTENT_TYPE_OPTIONS,
                    HeaderValue::from_static("nosniff"),
                ))
                .layer(SetResponseHeaderLayer::overriding(
                    header::REFERRER_POLICY,
                    HeaderValue::from_static("same-origin"),
                ))
                .layer(SetResponseHeaderLayer::overriding(
                    header::STRICT_TRANSPORT_SECURITY,
                    hsts,
                ))
                .layer(SetResponseHeaderLayer::overriding(
                    HeaderName::from_static("permissions-policy"),
                    HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
                )),
        )
        .with_state(state)
}
