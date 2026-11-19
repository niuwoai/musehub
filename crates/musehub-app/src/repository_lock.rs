//! PostgreSQL-backed repository write coordination shared by API and Runner processes.

use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::error::AppError;

/// Acquire a transaction-scoped advisory lock for all ref mutations in one repository.
///
/// The returned transaction must stay alive until the Git write and its database bookkeeping
/// finish. PostgreSQL releases the lock on commit, rollback, or connection loss.
pub async fn acquire_repository_write_lock(
    pool: &PgPool,
    repo_id: Uuid,
) -> Result<Transaction<'static, Postgres>, AppError> {
    let mut tx = pool.begin().await?;
    lock_repository_in_transaction(&mut tx, repo_id).await?;
    Ok(tx)
}

/// Acquire the same repository lock inside an existing transaction.
pub async fn lock_repository_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    repo_id: Uuid,
) -> Result<(), AppError> {
    let writable: bool = sqlx::query_scalar(
        "SELECT NOT pg_is_in_recovery()
         AND NOT current_setting('transaction_read_only')::boolean",
    )
    .fetch_one(&mut **tx)
    .await?;
    if !writable {
        return Err(AppError::DatabaseNotPrimary);
    }
    let lock_key = format!("musehub:repository-write:{repo_id}");
    let _: (bool,) =
        sqlx::query_as("SELECT pg_advisory_xact_lock(hashtextextended($1, 0)) IS NULL")
            .bind(lock_key)
            .fetch_one(&mut **tx)
            .await?;
    Ok(())
}
