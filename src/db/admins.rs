use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub struct Admin {
    pub id: Uuid,
    pub username: String,
    pub created_at: DateTime<Utc>,
}

pub async fn any_exists(db: &PgPool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM admins) AS "e!""#)
        .fetch_one(db)
        .await
}

/// Returns (id, password_hash) for login.
pub async fn credentials(
    db: &PgPool,
    username: &str,
) -> Result<Option<(Uuid, String)>, sqlx::Error> {
    Ok(sqlx::query!(
        "SELECT id, password_hash FROM admins WHERE username = $1",
        username
    )
    .fetch_optional(db)
    .await?
    .map(|r| (r.id, r.password_hash)))
}

pub async fn password_hash(db: &PgPool, id: Uuid) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar!("SELECT password_hash FROM admins WHERE id = $1", id)
        .fetch_optional(db)
        .await
}

pub async fn list(db: &PgPool) -> Result<Vec<Admin>, sqlx::Error> {
    sqlx::query_as!(
        Admin,
        "SELECT id, username, created_at FROM admins ORDER BY created_at"
    )
    .fetch_all(db)
    .await
}

/// Inserts an admin; returns None if the username is taken.
pub async fn create(
    db: &PgPool,
    username: &str,
    password_hash: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        "INSERT INTO admins (username, password_hash) VALUES ($1, $2)
         ON CONFLICT (username) DO NOTHING RETURNING id",
        username,
        password_hash
    )
    .fetch_optional(db)
    .await
}

/// Creates the first admin atomically: succeeds only if no admin exists yet.
pub async fn create_first(
    tx: &mut Transaction<'_, Postgres>,
    username: &str,
    password_hash: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    // Serialize concurrent /setup submissions.
    sqlx::query!("SELECT pg_advisory_xact_lock(4242)")
        .execute(&mut **tx)
        .await?;
    sqlx::query_scalar!(
        "INSERT INTO admins (username, password_hash)
         SELECT $1, $2 WHERE NOT EXISTS (SELECT 1 FROM admins)
         RETURNING id",
        username,
        password_hash
    )
    .fetch_optional(&mut **tx)
    .await
}

pub async fn set_password(db: &PgPool, id: Uuid, password_hash: &str) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE admins SET password_hash = $2 WHERE id = $1",
        id,
        password_hash
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Deletes another admin, never the last one. Returns true if deleted.
pub async fn delete(db: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let r = sqlx::query!(
        "DELETE FROM admins WHERE id = $1 AND (SELECT count(*) FROM admins) > 1",
        id
    )
    .execute(db)
    .await?;
    Ok(r.rows_affected() == 1)
}
