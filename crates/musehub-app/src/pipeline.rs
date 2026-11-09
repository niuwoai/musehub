//! Controlled pipeline runs (reference adapter, no shell / no Docker socket).

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::process::Command;
use uuid::Uuid;

use crate::artifacts::{
    self, artifact_storage_dir, finalize_artifact, quarantine_artifact, safe_join,
};
use crate::error::AppError;
use crate::git_http::bare_repo_path;
use crate::security::validate_git_oid;
use musehub_agent_adapter::configure_child_process_group;

use crate::pipeline_supervisor::PipelineSupervisor;
use crate::AppState;

const JOB_BUILD: &str = "build";
const KIND_RELEASE: &str = "release_build";
const KIND_PR_VALIDATION: &str = "pr_validation";
const ARTIFACT_NAME: &str = "bundle.tar";

#[derive(Serialize)]
pub struct PipelineRunView {
    pub id: Uuid,
    pub source_oid: String,
    pub workflow_name: String,
    pub state: String,
    pub run_attempt_id: Uuid,
    pub error_code: Option<String>,
}

#[derive(Deserialize)]
pub struct StartPipelineRequest {
    pub source_oid: String,
    pub workflow_name: Option<String>,
    pub pr_id: Option<Uuid>,
    pub issue_id: Option<Uuid>,
    pub agent_run_id: Option<Uuid>,
}

pub async fn seed_repo_environment(
    pool: &PgPool,
    org_id: Uuid,
    repo_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO environments (id, org_id, repo_id, slug, name)
         VALUES ($1, $2, $3, 'test', 'Test')
         ON CONFLICT (repo_id, slug) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(repo_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn start_pr_validation_run(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, number)): AxumPath<(String, String, i64)>,
) -> Result<(StatusCode, Json<PipelineRunView>), AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id, storage_key) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let pr = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT id, source_branch, state FROM pull_requests WHERE repo_id = $1 AND number = $2",
    )
    .bind(repo_id)
    .bind(number)
    .fetch_optional(&state.pool)
    .await?;
    let Some((pr_id, source_branch, pr_state)) = pr else {
        return Err(AppError::NotFound);
    };
    if pr_state != "open" {
        return Err(AppError::Validation(
            "pr_validation requires an open pull request".to_string(),
        ));
    }
    let bare = bare_repo_path(&state.git_root, &storage_key);
    let head_oid = super::pr::git_rev_parse_public(&bare, &format!("refs/heads/{source_branch}"))?;
    validate_git_oid(&head_oid).map_err(AppError::Validation)?;
    if let Some(existing) = active_pipeline(
        &state.pool,
        repo_id,
        KIND_PR_VALIDATION,
        &head_oid,
        Some(pr_id),
    )
    .await?
    {
        return Ok((StatusCode::OK, Json(existing)));
    }
    let workflow = "product-ci".to_string();
    spawn_pipeline_run(
        &state,
        org_id,
        repo_id,
        storage_key,
        head_oid,
        workflow,
        KIND_PR_VALIDATION,
        principal_id,
        Some(pr_id),
        None,
        None,
    )
    .await
}

/// Post-merge release build on current `main` (requires merged PR commit).
pub async fn start_pipeline_run(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo)): AxumPath<(String, String)>,
    Json(body): Json<StartPipelineRequest>,
) -> Result<(StatusCode, Json<PipelineRunView>), AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id, storage_key) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    validate_git_oid(&body.source_oid).map_err(AppError::Validation)?;
    let bare = bare_repo_path(&state.git_root, &storage_key);
    let live_main = super::pr::git_rev_parse_public(&bare, "refs/heads/main")?;
    if body.source_oid != live_main {
        return Err(AppError::Validation(
            "source_oid must match current main tip".to_string(),
        ));
    }
    let merged = sqlx::query_as::<_, (Uuid, Option<Uuid>, Option<Uuid>)>(
        "SELECT id, issue_id, agent_run_id FROM pull_requests
         WHERE repo_id = $1 AND state = 'merged' AND merged_oid = $2
         ORDER BY merged_at DESC NULLS LAST LIMIT 1",
    )
    .bind(repo_id)
    .bind(&body.source_oid)
    .fetch_optional(&state.pool)
    .await?;
    let Some((merged_pr_id, issue_id, agent_run_id)) = merged else {
        return Err(AppError::Validation(
            "source_oid must be a merged pull request commit on this repository".to_string(),
        ));
    };
    let pr_id = body.pr_id.unwrap_or(merged_pr_id);
    if pr_id != merged_pr_id {
        return Err(AppError::Validation(
            "pr_id does not match merge record for source_oid".to_string(),
        ));
    }
    let issue_id = body.issue_id.or(issue_id);
    let agent_run_id = body.agent_run_id.or(agent_run_id);
    let workflow = body
        .workflow_name
        .clone()
        .unwrap_or_else(|| "product-ci".to_string());
    if let Some(existing) =
        active_pipeline(&state.pool, repo_id, KIND_RELEASE, &body.source_oid, None).await?
    {
        return Ok((StatusCode::OK, Json(existing)));
    }
    spawn_pipeline_run(
        &state,
        org_id,
        repo_id,
        storage_key,
        body.source_oid.clone(),
        workflow,
        KIND_RELEASE,
        principal_id,
        Some(pr_id),
        issue_id,
        agent_run_id,
    )
    .await
}

async fn active_pipeline(
    pool: &PgPool,
    repo_id: Uuid,
    run_kind: &str,
    source_oid: &str,
    pr_id: Option<Uuid>,
) -> Result<Option<PipelineRunView>, AppError> {
    let row = if run_kind == KIND_PR_VALIDATION {
        sqlx::query_as::<_, (Uuid, String, String, String, Uuid, Option<String>)>(
            "SELECT id, source_oid, workflow_name, state, run_attempt_id, error_code
             FROM pipeline_runs
             WHERE repo_id = $1 AND run_kind = $2 AND source_oid = $3 AND pr_id = $4
               AND state IN ('queued','running')
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(repo_id)
        .bind(run_kind)
        .bind(source_oid)
        .bind(pr_id)
        .fetch_optional(pool)
        .await?
    } else {
        sqlx::query_as::<_, (Uuid, String, String, String, Uuid, Option<String>)>(
            "SELECT id, source_oid, workflow_name, state, run_attempt_id, error_code
             FROM pipeline_runs
             WHERE repo_id = $1 AND run_kind = $2 AND source_oid = $3
               AND state IN ('queued','running')
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(repo_id)
        .bind(run_kind)
        .bind(source_oid)
        .fetch_optional(pool)
        .await?
    };
    Ok(row.map(
        |(id, source_oid, workflow_name, state, attempt, error_code)| PipelineRunView {
            id,
            source_oid,
            workflow_name,
            state,
            run_attempt_id: attempt,
            error_code,
        },
    ))
}

#[allow(clippy::too_many_arguments)]
async fn spawn_pipeline_run(
    state: &Arc<AppState>,
    org_id: Uuid,
    repo_id: Uuid,
    storage_key: Uuid,
    source_oid: String,
    workflow: String,
    run_kind: &str,
    principal_id: Uuid,
    pr_id: Option<Uuid>,
    issue_id: Option<Uuid>,
    agent_run_id: Option<Uuid>,
) -> Result<(StatusCode, Json<PipelineRunView>), AppError> {
    let run_id = Uuid::new_v4();
    let attempt_id = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state,
         requested_by, run_attempt_id, pr_id, issue_id, agent_run_id, run_kind)
         VALUES ($1,$2,$3,$4,$5,'queued',$6,$7,$8,$9,$10,$11)",
    )
    .bind(run_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(&source_oid)
    .bind(&workflow)
    .bind(principal_id)
    .bind(attempt_id)
    .bind(pr_id)
    .bind(issue_id)
    .bind(agent_run_id)
    .bind(run_kind)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO pipeline_jobs (id, org_id, repo_id, pipeline_run_id, job_key, state)
         VALUES ($1,$2,$3,$4,$5,'queued')",
    )
    .bind(job_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(run_id)
    .bind(JOB_BUILD)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let pool = state.pool.clone();
    let git_root = state.git_root.clone();
    let artifact_root = state.artifact_root.clone();
    let pipeline_bin = state.pipeline_adapter_bin.clone();
    let supervisor = state.pipeline_supervisor.clone();
    let instance_id = state.instance_id;
    let source_oid_spawn = source_oid.clone();
    let run_kind_owned = run_kind.to_string();
    tokio::spawn(async move {
        run_pipeline_job(
            pool,
            git_root,
            artifact_root,
            pipeline_bin,
            supervisor,
            instance_id,
            storage_key,
            run_id,
            job_id,
            org_id,
            repo_id,
            source_oid_spawn,
            run_kind_owned,
        )
        .await;
    });

    Ok((
        StatusCode::CREATED,
        Json(PipelineRunView {
            id: run_id,
            source_oid,
            workflow_name: workflow,
            state: "queued".to_string(),
            run_attempt_id: attempt_id,
            error_code: None,
        }),
    ))
}

pub async fn get_pipeline_run(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, run_id)): AxumPath<(String, String, Uuid)>,
) -> Result<Json<PipelineRunView>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (_, repo_id, _) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let row = sqlx::query_as::<_, (Uuid, String, String, String, Option<String>)>(
        "SELECT id, source_oid, workflow_name, state, error_code FROM pipeline_runs
         WHERE id = $1 AND repo_id = $2",
    )
    .bind(run_id)
    .bind(repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((id, source_oid, workflow_name, run_state, error_code)) = row else {
        return Err(AppError::NotFound);
    };
    let attempt: (Uuid,) = sqlx::query_as("SELECT run_attempt_id FROM pipeline_runs WHERE id = $1")
        .bind(id)
        .fetch_one(&state.pool)
        .await?;
    Ok(Json(PipelineRunView {
        id,
        source_oid,
        workflow_name,
        state: run_state,
        run_attempt_id: attempt.0,
        error_code,
    }))
}

pub async fn cancel_pipeline_run(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, run_id)): AxumPath<(String, String, Uuid)>,
) -> Result<Json<PipelineRunView>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (_, repo_id, _) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    sqlx::query(
        "UPDATE pipeline_runs SET cancel_requested = TRUE, state = 'cancel_requested',
         updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND repo_id = $2 AND state IN ('queued','running')",
    )
    .bind(run_id)
    .bind(repo_id)
    .execute(&state.pool)
    .await?;
    let _ = state.pipeline_supervisor.cancel(run_id).await;
    get_pipeline_run(State(state), headers, AxumPath((org, repo, run_id))).await
}

#[allow(clippy::too_many_arguments)]
async fn run_pipeline_job(
    pool: PgPool,
    git_root: PathBuf,
    artifact_root: PathBuf,
    pipeline_bin: PathBuf,
    supervisor: Arc<PipelineSupervisor>,
    instance_id: Uuid,
    storage_key: Uuid,
    run_id: Uuid,
    job_id: Uuid,
    org_id: Uuid,
    repo_id: Uuid,
    source_oid: String,
    run_kind: String,
) {
    if crate::lease::claim_pipeline_run_lease(&pool, run_id, instance_id)
        .await
        .is_err()
    {
        mark_failed(&pool, run_id, job_id, "lease_held_by_peer").await;
        return;
    }

    let _ = sqlx::query(
        "UPDATE pipeline_runs SET state = 'running', updated_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(run_id)
    .execute(&pool)
    .await;
    let _ = sqlx::query(
        "UPDATE pipeline_jobs SET state = 'running', updated_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(job_id)
    .execute(&pool)
    .await;

    if cancel_flag(&pool, run_id).await {
        mark_canceled(&pool, run_id, job_id).await;
        return;
    }

    let artifact_id = Uuid::new_v4();
    let dir = artifact_storage_dir(&artifact_root, org_id, repo_id, artifact_id);
    if std::fs::create_dir_all(&dir).is_err() {
        mark_failed(&pool, run_id, job_id, "artifact_dir_failed").await;
        return;
    }
    let file_path = match safe_join(&dir, ARTIFACT_NAME) {
        Ok(path) => path,
        Err(_) => {
            mark_failed(&pool, run_id, job_id, "artifact_path_failed").await;
            return;
        }
    };
    let _ = sqlx::query(
        "INSERT INTO artifacts (id, org_id, repo_id, pipeline_run_id, name, sha256, byte_length,
         storage_path, state)
         VALUES ($1,$2,$3,$4,$5,'',0,$6,'uploading')",
    )
    .bind(artifact_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(run_id)
    .bind(ARTIFACT_NAME)
    .bind(dir.to_string_lossy().as_ref())
    .execute(&pool)
    .await;

    let bare = bare_repo_path(&git_root, &storage_key);
    let mut cmd = Command::new(&pipeline_bin);
    cmd.env("MUSEHUB_PIPELINE_SOURCE_OID", &source_oid)
        .env("MUSEHUB_PIPELINE_ARTIFACT_PATH", &file_path)
        .env("MUSEHUB_PIPELINE_BARE_GIT", bare.to_str().unwrap_or("."))
        .env("MUSEHUB_PIPELINE_RUN_KIND", &run_kind)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_child_process_group(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(_) => {
            let _ = quarantine_artifact(&pool, artifact_id).await;
            mark_failed(&pool, run_id, job_id, "adapter_spawn_failed").await;
            return;
        }
    };
    if let Some(pid) = child.id() {
        supervisor.register(run_id, pid).await;
    }

    let deadline = Duration::from_secs(120);
    let finished = tokio::time::timeout(deadline, child.wait()).await;
    if cancel_flag(&pool, run_id).await {
        let _ = supervisor.cancel(run_id).await;
        let _ = child.kill().await;
        let _ = quarantine_artifact(&pool, artifact_id).await;
        mark_canceled(&pool, run_id, job_id).await;
        supervisor.finish(run_id).await;
        return;
    }
    let status = match finished {
        Ok(Ok(status)) => status,
        _ => {
            let _ = supervisor.cancel(run_id).await;
            let _ = child.kill().await;
            let _ = quarantine_artifact(&pool, artifact_id).await;
            mark_failed(&pool, run_id, job_id, "pipeline_timeout").await;
            supervisor.finish(run_id).await;
            return;
        }
    };
    if !status.success() {
        let _ = quarantine_artifact(&pool, artifact_id).await;
        mark_failed(&pool, run_id, job_id, "pipeline_adapter_failed").await;
        return;
    }
    match artifacts::sha256_file(&file_path) {
        Ok((sha, len)) => {
            if finalize_artifact(&pool, artifact_id, &sha, len as i64)
                .await
                .is_err()
            {
                let _ = std::fs::remove_file(&file_path);
                mark_failed(&pool, run_id, job_id, "artifact_db_finalize_failed").await;
                return;
            }
        }
        Err(_) => {
            let _ = quarantine_artifact(&pool, artifact_id).await;
            let _ = std::fs::remove_file(&file_path);
            mark_failed(&pool, run_id, job_id, "artifact_verify_failed").await;
            return;
        }
    }
    let _ = sqlx::query(
        "UPDATE pipeline_jobs SET state = 'succeeded', updated_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(job_id)
    .execute(&pool)
    .await;
    let _ = sqlx::query(
        "UPDATE pipeline_runs SET state = 'succeeded', updated_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(run_id)
    .execute(&pool)
    .await;
    supervisor.finish(run_id).await;
}

async fn cancel_flag(pool: &PgPool, run_id: Uuid) -> bool {
    matches!(
        sqlx::query_as::<_, (bool,)>("SELECT cancel_requested FROM pipeline_runs WHERE id = $1")
            .bind(run_id)
            .fetch_optional(pool)
            .await,
        Ok(Some((true,)))
    )
}

async fn mark_failed(pool: &PgPool, run_id: Uuid, job_id: Uuid, code: &str) {
    let _ = sqlx::query(
        "UPDATE pipeline_jobs SET state = 'failed', updated_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(job_id)
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "UPDATE pipeline_runs SET state = 'failed', error_code = $2, updated_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(run_id)
    .bind(code)
    .execute(pool)
    .await;
}

async fn mark_canceled(pool: &PgPool, run_id: Uuid, job_id: Uuid) {
    let _ = sqlx::query(
        "UPDATE pipeline_jobs SET state = 'canceled', updated_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(job_id)
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "UPDATE pipeline_runs SET state = 'canceled', updated_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(run_id)
    .execute(pool)
    .await;
}
