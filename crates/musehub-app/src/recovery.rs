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
    pub workflow_jobs_lost: u64,
    pub workflow_attempts_lost: u64,
    pub workflow_attempts_canceled: u64,
    pub workflow_jobs_canceled: u64,
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

    let lost_attempts = sqlx::query(
        "UPDATE pipeline_workflow_job_attempts a
         SET state = 'lost', error_code = $1, completed_at = CURRENT_TIMESTAMP,
             started_at = COALESCE(a.started_at, a.created_at), updated_at = CURRENT_TIMESTAMP
         FROM pipeline_workflow_jobs j
         JOIN pipeline_runs r ON r.id = j.pipeline_run_id
         WHERE a.workflow_job_id = j.id AND r.state = 'failed'
           AND r.error_code = $1 AND a.state IN ('queued', 'running')",
    )
    .bind(ERROR_CODE)
    .execute(pool)
    .await?;
    report.workflow_attempts_lost = lost_attempts.rows_affected();

    let lost_jobs = sqlx::query(
        "UPDATE pipeline_workflow_jobs j
         SET state = 'failed', error_code = $1, completed_at = CURRENT_TIMESTAMP,
             attempt_count = GREATEST(j.attempt_count, 1),
             started_at = COALESCE(j.started_at, j.created_at), updated_at = CURRENT_TIMESTAMP
         FROM pipeline_runs r
         WHERE j.pipeline_run_id = r.id AND r.state = 'failed'
           AND r.error_code = $1 AND j.state IN ('queued', 'running')",
    )
    .bind(ERROR_CODE)
    .execute(pool)
    .await?;
    report.workflow_jobs_lost = lost_jobs.rows_affected();

    let canceled_attempts = sqlx::query(
        "UPDATE pipeline_workflow_job_attempts a
         SET state = 'canceled', error_code = $1, completed_at = CURRENT_TIMESTAMP,
             started_at = COALESCE(a.started_at, a.created_at), updated_at = CURRENT_TIMESTAMP
         FROM pipeline_workflow_jobs j
         JOIN pipeline_runs r ON r.id = j.pipeline_run_id
         WHERE a.workflow_job_id = j.id AND r.state = 'canceled'
           AND r.error_code = $1 AND a.state IN ('queued', 'running')",
    )
    .bind(ERROR_CODE)
    .execute(pool)
    .await?;
    let canceled_jobs = sqlx::query(
        "UPDATE pipeline_workflow_jobs j
         SET state = 'canceled', error_code = $1, completed_at = CURRENT_TIMESTAMP,
             started_at = COALESCE(j.started_at, j.created_at), updated_at = CURRENT_TIMESTAMP
         FROM pipeline_runs r
         WHERE j.pipeline_run_id = r.id AND r.state = 'canceled'
           AND r.error_code = $1 AND j.state IN ('queued', 'running')",
    )
    .bind(ERROR_CODE)
    .execute(pool)
    .await?;
    report.workflow_jobs_canceled = canceled_jobs.rows_affected();
    report.workflow_attempts_canceled = canceled_attempts.rows_affected();

    Ok(report)
}
