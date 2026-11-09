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
    if let Some(lease) = try_claim_queued_pipeline(pool, run_id, instance_id).await? {
        return Ok(lease);
    }
    try_reclaim_expired_pipeline(pool, run_id, instance_id).await
}

async fn try_claim_queued_pipeline(
    pool: &PgPool,
    run_id: Uuid,
    instance_id: Uuid,
) -> Result<Option<WorkLease>, AppError> {
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
    Ok(row.map(|(epoch,)| WorkLease { epoch }))
}

/// After worker loss or lease TTL, a new instance may reclaim the same run id (epoch bumps).
pub async fn try_reclaim_expired_pipeline(
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
         WHERE id = $1 AND state IN ('running', 'cancel_requested')
           AND lease_until < CURRENT_TIMESTAMP
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

pub async fn finalize_pipeline_run_state(
    pool: &PgPool,
    run_id: Uuid,
    instance_id: Uuid,
    epoch: i64,
    state: &str,
    error_code: Option<&str>,
) -> Result<bool, AppError> {
    let updated = sqlx::query(
        "UPDATE pipeline_runs SET state = $4, error_code = COALESCE($5, error_code),
         updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND lease_holder = $2 AND lease_epoch = $3
           AND state IN ('running', 'cancel_requested')",
    )
    .bind(run_id)
    .bind(instance_id)
    .bind(epoch)
    .bind(state)
    .bind(error_code)
    .execute(pool)
    .await?;
    Ok(updated.rows_affected() == 1)
}

pub async fn finalize_pipeline_job_state(
    pool: &PgPool,
    job_id: Uuid,
    run_id: Uuid,
    instance_id: Uuid,
    epoch: i64,
    state: &str,
) -> Result<bool, AppError> {
    let updated = sqlx::query(
        "UPDATE pipeline_jobs SET state = $4, updated_at = CURRENT_TIMESTAMP
         WHERE id = $1
           AND EXISTS (
             SELECT 1 FROM pipeline_runs
             WHERE id = $2 AND lease_holder = $3 AND lease_epoch = $5
           )",
    )
    .bind(job_id)
    .bind(run_id)
    .bind(instance_id)
    .bind(state)
    .bind(epoch)
    .execute(pool)
    .await?;
    Ok(updated.rows_affected() == 1)
}

pub async fn bump_pipeline_run_epoch_for_test(pool: &PgPool, run_id: Uuid) -> Result<i64, AppError> {
    let row = sqlx::query_as::<_, (i64,)>(
        "UPDATE pipeline_runs SET lease_epoch = lease_epoch + 1, updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 RETURNING lease_epoch",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
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

/// Terminal writes require matching holder + epoch (stale worker cannot publish state).
#[allow(clippy::too_many_arguments)]
pub async fn finalize_agent_run_state(
    pool: &PgPool,
    run_id: Uuid,
    instance_id: Uuid,
    epoch: i64,
    state: &str,
    error_code: Option<&str>,
    result_commit_oid: Option<&str>,
    usage_tokens: Option<i64>,
) -> Result<bool, AppError> {
    let updated = sqlx::query(
        "UPDATE agent_runs SET state = $4, error_code = $5, result_commit_oid = COALESCE($6, result_commit_oid),
         usage_tokens = COALESCE($7, usage_tokens), updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND lease_holder = $2 AND lease_epoch = $3
           AND state IN ('running','cancel_requested')",
    )
    .bind(run_id)
    .bind(instance_id)
    .bind(epoch)
    .bind(state)
    .bind(error_code)
    .bind(result_commit_oid)
    .bind(usage_tokens)
    .execute(pool)
    .await?;
    Ok(updated.rows_affected() == 1)
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

pub fn parse_instance_id() -> Uuid {
    std::env::var("MUSEHUB_INSTANCE_ID")
        .ok()
        .and_then(|s| Uuid::parse_str(&s).ok())
        .unwrap_or_else(Uuid::new_v4)
}

pub async fn renew_agent_work_lease_for_test(
    pool: &PgPool,
    run_id: Uuid,
    instance_id: Uuid,
    epoch: i64,
) -> Result<(), AppError> {
    renew_agent_work_lease(pool, run_id, instance_id, epoch).await
}

/// Simulate a new epoch (e.g. after reconcile or peer reclaim) without a live worker.
pub async fn bump_agent_run_epoch_for_test(pool: &PgPool, run_id: Uuid) -> Result<i64, AppError> {
    let row = sqlx::query_as::<_, (i64,)>(
        "UPDATE agent_runs SET lease_epoch = lease_epoch + 1, updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 RETURNING lease_epoch",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}
