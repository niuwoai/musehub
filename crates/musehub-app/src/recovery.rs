//! Mark in-flight work orphaned after process restart (no live worker lease).

use serde::Serialize;
use sqlx::PgPool;

use crate::error::AppError;

#[derive(Debug, Default, Serialize)]
pub struct ReconcileReport {
    pub agent_runs_failed: u64,
    pub agent_runs_canceled: u64,
    pub pipeline_runs_failed: u64,
    pub pipeline_runs_canceled: u64,
}

const ERROR_CODE: &str = "orphaned_after_restart";
const UNLEASED: &str = "(lease_until IS NULL OR lease_until < CURRENT_TIMESTAMP)";

/// Clears only rows **without an active lease** (another instance may still own the work).
pub async fn reconcile_orphaned_work(pool: &PgPool) -> Result<ReconcileReport, AppError> {
    let mut report = ReconcileReport::default();

    let agent_cancel = sqlx::query(&format!(
        "UPDATE agent_runs SET state = 'canceled', error_code = $1,
         updated_at = CURRENT_TIMESTAMP
         WHERE state = 'cancel_requested' AND {UNLEASED}"
    ))
    .bind(ERROR_CODE)
    .execute(pool)
    .await?;
    report.agent_runs_canceled = agent_cancel.rows_affected();

    let agent_fail = sqlx::query(&format!(
        "UPDATE agent_runs SET state = 'failed', error_code = $1,
         updated_at = CURRENT_TIMESTAMP
         WHERE state IN ('queued', 'running') AND {UNLEASED}"
    ))
    .bind(ERROR_CODE)
    .execute(pool)
    .await?;
    report.agent_runs_failed = agent_fail.rows_affected();

    let pipe_cancel = sqlx::query(&format!(
        "UPDATE pipeline_runs SET state = 'canceled', error_code = $1,
         updated_at = CURRENT_TIMESTAMP
         WHERE state = 'cancel_requested' AND {UNLEASED}"
    ))
    .bind(ERROR_CODE)
    .execute(pool)
    .await?;
    report.pipeline_runs_canceled = pipe_cancel.rows_affected();

    let pipe_fail = sqlx::query(&format!(
        "UPDATE pipeline_runs SET state = 'failed', error_code = $1,
         updated_at = CURRENT_TIMESTAMP
         WHERE state IN ('queued', 'running') AND {UNLEASED}"
    ))
    .bind(ERROR_CODE)
    .execute(pool)
    .await?;
    report.pipeline_runs_failed = pipe_fail.rows_affected();

    sqlx::query(
        "UPDATE pipeline_jobs SET state = 'canceled', updated_at = CURRENT_TIMESTAMP
         WHERE pipeline_run_id IN (
           SELECT id FROM pipeline_runs WHERE state = 'canceled' AND error_code = $1
         ) AND state IN ('queued', 'running')",
    )
    .bind(ERROR_CODE)
    .execute(pool)
    .await?;

    sqlx::query(
        "UPDATE pipeline_jobs SET state = 'failed', updated_at = CURRENT_TIMESTAMP
         WHERE pipeline_run_id IN (
           SELECT id FROM pipeline_runs WHERE state = 'failed' AND error_code = $1
         ) AND state IN ('queued', 'running')",
    )
    .bind(ERROR_CODE)
    .execute(pool)
    .await?;

    Ok(report)
}
