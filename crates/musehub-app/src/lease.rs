//! Worker lease helpers for single-control-plane instances.

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;

const LEASE_SECS: i64 = 120;

pub async fn claim_agent_run_lease(
    pool: &PgPool,
    run_id: Uuid,
    instance_id: Uuid,
) -> Result<(), AppError> {
    let updated = sqlx::query(
        "UPDATE agent_runs SET lease_holder = $2,
         lease_until = CURRENT_TIMESTAMP + ($3::text || ' seconds')::interval,
         updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND state IN ('queued','running','cancel_requested')
           AND (lease_until IS NULL OR lease_until < CURRENT_TIMESTAMP OR lease_holder = $2)",
    )
    .bind(run_id)
    .bind(instance_id)
    .bind(LEASE_SECS)
    .execute(pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::Conflict(
            "agent run lease held by another instance".to_string(),
        ));
    }
    Ok(())
}

pub async fn renew_agent_run_lease(
    pool: &PgPool,
    run_id: Uuid,
    instance_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE agent_runs SET lease_until = CURRENT_TIMESTAMP + ($3::text || ' seconds')::interval
         WHERE id = $1 AND lease_holder = $2 AND state IN ('queued','running','cancel_requested')",
    )
    .bind(run_id)
    .bind(instance_id)
    .bind(LEASE_SECS)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn claim_pipeline_run_lease(
    pool: &PgPool,
    run_id: Uuid,
    instance_id: Uuid,
) -> Result<(), AppError> {
    let updated = sqlx::query(
        "UPDATE pipeline_runs SET lease_holder = $2,
         lease_until = CURRENT_TIMESTAMP + ($3::text || ' seconds')::interval,
         updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND state IN ('queued','running','cancel_requested')
           AND (lease_until IS NULL OR lease_until < CURRENT_TIMESTAMP OR lease_holder = $2)",
    )
    .bind(run_id)
    .bind(instance_id)
    .bind(LEASE_SECS)
    .execute(pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::Conflict(
            "pipeline run lease held by another instance".to_string(),
        ));
    }
    Ok(())
}

pub fn parse_instance_id() -> Uuid {
    std::env::var("MUSEHUB_INSTANCE_ID")
        .ok()
        .and_then(|s| Uuid::parse_str(&s).ok())
        .unwrap_or_else(Uuid::new_v4)
}
