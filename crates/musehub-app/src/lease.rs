//! Worker lease helpers: atomic claim, epoch fencing, background renew.

use std::time::Duration;

use sqlx::PgPool;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::error::AppError;

pub const LEASE_SECS: i64 = 45;
const RENEW_INTERVAL_SECS: u64 = 15;

/// Returned after a successful atomic work claim.
#[derive(Debug, Clone, Copy)]
pub struct WorkLease {
    pub epoch: i64,
}

pub async fn try_begin_agent_work(
    pool: &PgPool,
    run_id: Uuid,
    instance_id: Uuid,
) -> Result<WorkLease, AppError> {
    let row = sqlx::query_as::<_, (i64,)>(
        "UPDATE agent_runs SET
           state = 'running',
           lease_holder = $2,
           lease_until = CURRENT_TIMESTAMP + ($3::text || ' seconds')::interval,
           lease_epoch = agent_runs.lease_epoch + 1,
           updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND state = 'queued'
           AND (lease_holder IS NULL OR lease_holder = $2 OR lease_until < CURRENT_TIMESTAMP)
         RETURNING lease_epoch",
    )
    .bind(run_id)
    .bind(instance_id)
    .bind(LEASE_SECS)
    .fetch_optional(pool)
    .await?;
    row.map(|(epoch,)| WorkLease { epoch }).ok_or_else(|| {
        AppError::Conflict("agent run already claimed or lease held by peer".to_string())
    })
}

pub async fn try_begin_pipeline_work(
    pool: &PgPool,
    run_id: Uuid,
    instance_id: Uuid,
) -> Result<WorkLease, AppError> {
    let row = sqlx::query_as::<_, (i64,)>(
        "UPDATE pipeline_runs SET
           state = 'running',
           lease_holder = $2,
           lease_until = CURRENT_TIMESTAMP + ($3::text || ' seconds')::interval,
           lease_epoch = pipeline_runs.lease_epoch + 1,
           updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND state = 'queued'
           AND (lease_holder IS NULL OR lease_holder = $2 OR lease_until < CURRENT_TIMESTAMP)
         RETURNING lease_epoch",
    )
    .bind(run_id)
    .bind(instance_id)
    .bind(LEASE_SECS)
    .fetch_optional(pool)
    .await?;
    row.map(|(epoch,)| WorkLease { epoch }).ok_or_else(|| {
        AppError::Conflict("pipeline run already claimed or lease held by peer".to_string())
    })
}

pub async fn renew_agent_work_lease(
    pool: &PgPool,
    run_id: Uuid,
    instance_id: Uuid,
    epoch: i64,
) -> Result<(), AppError> {
    let updated = sqlx::query(
        "UPDATE agent_runs SET lease_until = CURRENT_TIMESTAMP + ($4::text || ' seconds')::interval
         WHERE id = $1 AND lease_holder = $2 AND lease_epoch = $3
           AND state IN ('queued','running','cancel_requested')",
    )
    .bind(run_id)
    .bind(instance_id)
    .bind(epoch)
    .bind(LEASE_SECS)
    .execute(pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::Conflict(
            "agent lease renew rejected (fencing)".to_string(),
        ));
    }
    Ok(())
}

pub async fn renew_pipeline_work_lease(
    pool: &PgPool,
    run_id: Uuid,
    instance_id: Uuid,
    epoch: i64,
) -> Result<(), AppError> {
    let updated = sqlx::query(
        "UPDATE pipeline_runs SET lease_until = CURRENT_TIMESTAMP + ($4::text || ' seconds')::interval
         WHERE id = $1 AND lease_holder = $2 AND lease_epoch = $3
           AND state IN ('queued','running','cancel_requested')",
    )
    .bind(run_id)
    .bind(instance_id)
    .bind(epoch)
    .bind(LEASE_SECS)
    .execute(pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::Conflict(
            "pipeline lease renew rejected (fencing)".to_string(),
        ));
    }
    Ok(())
}

pub(crate) struct RenewAbortOnDrop(JoinHandle<()>);
impl Drop for RenewAbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub fn spawn_agent_lease_renewal(
    pool: PgPool,
    run_id: Uuid,
    instance_id: Uuid,
    epoch: i64,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let interval = Duration::from_secs(RENEW_INTERVAL_SECS);
        loop {
            tokio::time::sleep(interval).await;
            let terminal =
                sqlx::query_as::<_, (String,)>("SELECT state FROM agent_runs WHERE id = $1")
                    .bind(run_id)
                    .fetch_optional(&pool)
                    .await
                    .ok()
                    .flatten();
            let Some((state,)) = terminal else {
                break;
            };
            if !matches!(state.as_str(), "queued" | "running" | "cancel_requested") {
                break;
            }
            if renew_agent_work_lease(&pool, run_id, instance_id, epoch)
                .await
                .is_err()
            {
                break;
            }
        }
    })
}

pub(crate) fn agent_lease_renew_guard(
    pool: PgPool,
    run_id: Uuid,
    instance_id: Uuid,
    epoch: i64,
) -> RenewAbortOnDrop {
    RenewAbortOnDrop(spawn_agent_lease_renewal(pool, run_id, instance_id, epoch))
}

pub(crate) fn pipeline_lease_renew_guard(
    pool: PgPool,
    run_id: Uuid,
    instance_id: Uuid,
    epoch: i64,
) -> RenewAbortOnDrop {
    RenewAbortOnDrop(spawn_pipeline_lease_renewal(
        pool,
        run_id,
        instance_id,
        epoch,
    ))
}

pub fn spawn_pipeline_lease_renewal(
    pool: PgPool,
    run_id: Uuid,
    instance_id: Uuid,
    epoch: i64,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let interval = Duration::from_secs(RENEW_INTERVAL_SECS);
        loop {
            tokio::time::sleep(interval).await;
            let terminal =
                sqlx::query_as::<_, (String,)>("SELECT state FROM pipeline_runs WHERE id = $1")
                    .bind(run_id)
                    .fetch_optional(&pool)
                    .await
                    .ok()
                    .flatten();
            let Some((state,)) = terminal else {
                break;
            };
            if !matches!(state.as_str(), "queued" | "running" | "cancel_requested") {
                break;
            }
            if renew_pipeline_work_lease(&pool, run_id, instance_id, epoch)
                .await
                .is_err()
            {
                break;
            }
        }
    })
}

/// Pre-queues a tentative lease so boot reconcile does not steal before the worker claims.
pub async fn reserve_agent_run_lease(
    pool: &PgPool,
    run_id: Uuid,
    instance_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE agent_runs SET lease_holder = $2,
         lease_until = CURRENT_TIMESTAMP + ($3::text || ' seconds')::interval
         WHERE id = $1 AND state = 'queued' AND lease_holder IS NULL",
    )
    .bind(run_id)
    .bind(instance_id)
    .bind(LEASE_SECS)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn reserve_pipeline_run_lease(
    pool: &PgPool,
    run_id: Uuid,
    instance_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE pipeline_runs SET lease_holder = $2,
         lease_until = CURRENT_TIMESTAMP + ($3::text || ' seconds')::interval
         WHERE id = $1 AND state = 'queued' AND lease_holder IS NULL",
    )
    .bind(run_id)
    .bind(instance_id)
    .bind(LEASE_SECS)
    .execute(pool)
    .await?;
    Ok(())
}

pub fn parse_instance_id() -> Uuid {
    std::env::var("MUSEHUB_INSTANCE_ID")
        .ok()
        .and_then(|s| Uuid::parse_str(&s).ok())
        .unwrap_or_else(Uuid::new_v4)
}

/// Test-only: drive renew immediately (deterministic TTL tests).
pub async fn renew_agent_work_lease_for_test(
    pool: &PgPool,
    run_id: Uuid,
    instance_id: Uuid,
    epoch: i64,
) -> Result<(), AppError> {
    renew_agent_work_lease(pool, run_id, instance_id, epoch).await
}

pub fn _lease_secs_for_tests() -> i64 {
    LEASE_SECS
}
