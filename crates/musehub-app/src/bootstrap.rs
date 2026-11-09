use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{hash_password, normalize_handle};
use crate::error::AppError;
use crate::security::validate_bootstrap_password;

/// Seeds default org `demo` and admin `admin` when the database is empty.
pub async fn seed_if_empty(pool: &PgPool, bootstrap_password: &str) -> Result<(), AppError> {
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM principals")
        .fetch_one(pool)
        .await?;
    if count.0 > 0 {
        return Ok(());
    }
    if bootstrap_password.trim().is_empty() {
        return Err(AppError::Validation(
            "database empty: set MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD once (min 16 chars)".to_string(),
        ));
    }
    validate_bootstrap_password(bootstrap_password).map_err(AppError::Validation)?;
    let org_id = Uuid::new_v4();
    let principal_id = Uuid::new_v4();
    let password_hash = hash_password(bootstrap_password)?;
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO organizations (id, slug, slug_normalized, display_name, status)
         VALUES ($1, 'demo', 'demo', 'Demo Org', 'active')",
    )
    .bind(org_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO principals (id, kind, handle, handle_normalized, display_name, status)
         VALUES ($1, 'human', 'admin', $2, 'Admin', 'active')",
    )
    .bind(principal_id)
    .bind(normalize_handle("admin"))
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO user_credentials (id, principal_id, password_hash, password_changed_at)
         VALUES ($1, $2, $3, CURRENT_TIMESTAMP)",
    )
    .bind(Uuid::new_v4())
    .bind(principal_id)
    .bind(password_hash)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO memberships (id, org_id, principal_id, role, status)
         VALUES ($1, $2, $3, 'owner', 'active')",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(principal_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}
