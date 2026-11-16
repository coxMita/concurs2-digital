use std::net::SocketAddr;

use concurs::{app, config::Config, db, routes::setup, state::AppState, web};
use sqlx::postgres::PgPoolOptions;
use tower_sessions::{ExpiredDeletion, cookie::Key};
use tower_sessions_sqlx_store::PostgresStore;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=warn,sqlx=warn")),
        )
        .init();

    let config = Config::from_env();
    tokio::fs::create_dir_all(&config.upload_dir).await?;
    web::set_asset_version(web::compute_asset_version(&config.static_dir));

    let pool = connect_with_retry(&config).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    let store = PostgresStore::new(pool.clone());
    store.migrate().await?;
    tokio::spawn(
        store
            .clone()
            .continuously_delete_expired(tokio::time::Duration::from_secs(600)),
    );

    let key_bytes = db::settings::get_or_create(&pool, "session_key", || {
        concurs::services::credentials::random_bytes::<64>().to_vec()
    })
    .await?;
    let key = Key::from(&key_bytes);

    let state = AppState::new(pool, config.clone());
    setup::announce_if_needed(&state).await?;

    let router = app::build(state, store, key);
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!("listening on {}", config.bind_addr);
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

/// Postgres may still be starting (e.g. first boot): retry for up to ~60s.
async fn connect_with_retry(config: &Config) -> Result<sqlx::PgPool, sqlx::Error> {
    let mut attempt = 0;
    loop {
        match PgPoolOptions::new()
            .max_connections(config.db_max_connections)
            .acquire_timeout(std::time::Duration::from_secs(10))
            .connect(&config.database_url)
            .await
        {
            Ok(pool) => return Ok(pool),
            Err(e) if attempt < 30 => {
                attempt += 1;
                tracing::warn!("database not ready ({e}), retrying...");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    tokio::select! { () = ctrl_c => {}, () = term => {} }
}
