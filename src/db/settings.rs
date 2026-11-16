//! Generated secrets stored in `app_settings`.

use sqlx::PgPool;

/// Returns the value stored under `key`, inserting `generate()` first if absent.
/// Safe under concurrent starts (ON CONFLICT DO NOTHING, then read back).
pub async fn get_or_create(
    db: &PgPool,
    key: &str,
    generate: impl FnOnce() -> Vec<u8>,
) -> Result<Vec<u8>, sqlx::Error> {
    if let Some(v) = sqlx::query_scalar!("SELECT value FROM app_settings WHERE key = $1", key)
        .fetch_optional(db)
        .await?
    {
        return Ok(v);
    }
    let value = generate();
    sqlx::query!(
        "INSERT INTO app_settings (key, value) VALUES ($1, $2) ON CONFLICT (key) DO NOTHING",
        key,
        value
    )
    .execute(db)
    .await?;
    sqlx::query_scalar!("SELECT value FROM app_settings WHERE key = $1", key)
        .fetch_one(db)
        .await
}

pub async fn delete(db: &PgPool, key: &str) -> Result<(), sqlx::Error> {
    sqlx::query!("DELETE FROM app_settings WHERE key = $1", key)
        .execute(db)
        .await?;
    Ok(())
}
