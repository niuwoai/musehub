//! Controlled pipeline runs (reference adapter, no shell / no Docker socket).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::Json;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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

use crate::mcp::idempotency::{
    begin_idempotent_write, complete_idempotent_write, fail_idempotent_write, IdempotencyAction,
};
use crate::pipeline_supervisor::PipelineSupervisor;
use crate::pipeline_workflow::{parse_workflow, ValidatedWorkflow, WorkflowEvent};
use crate::AppState;
use serde_json::json;

const JOB_BUILD: &str = "build";
const KIND_RELEASE: &str = "release_build";
const KIND_PR_VALIDATION: &str = "pr_validation";
const ARTIFACT_NAME: &str = "bundle.tar";
const PR_WORKFLOW_PATH: &str = ".musehub/workflows/ci.yaml";
const MAX_WORKFLOW_SOURCE_BYTES: usize = 64 * 1024;
const MAX_PIPELINE_LOG_BYTES: usize = 1024 * 1024;

#[derive(Serialize)]
pub struct PipelineRunView {
    pub id: Uuid,
    pub source_oid: String,
    pub workflow_name: String,
    pub state: String,
    pub run_attempt_id: Uuid,
    pub error_code: Option<String>,
}

#[derive(Serialize)]
pub struct PipelineRunHistoryView {
    pub id: Uuid,
    pub source_oid: String,
    pub workflow_name: String,
    pub state: String,
    pub run_attempt_id: Uuid,
    pub error_code: Option<String>,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct PipelineRunHistoryPage {
    pub items: Vec<PipelineRunHistoryView>,
    pub next_cursor: Option<Uuid>,
    pub has_more: bool,
}

#[derive(Deserialize)]
pub struct ListPipelineRunsQuery {
    limit: Option<i64>,
    before_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize)]
pub struct PipelineJobView {
    pub id: Uuid,
    pub pipeline_run_id: Uuid,
    pub job_key: String,
    pub state: String,
    pub retry_allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub artifacts: Vec<PipelineArtifactView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineArtifactView {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub sha256: String,
    pub byte_length: i64,
    #[serde(default)]
    pub download_url: String,
}

#[derive(Deserialize)]
pub struct RetryPipelineJobRequest {
    pub attempt_id: Option<Uuid>,
}

#[derive(Deserialize)]
pub struct StartPipelineRequest {
    pub source_oid: String,
    pub workflow_name: Option<String>,
    pub pr_id: Option<Uuid>,
    pub issue_id: Option<Uuid>,
    pub agent_run_id: Option<Uuid>,
}

#[derive(Deserialize)]
pub struct StartManualWorkflowRequest {
    pub source_oid: String,
}

pub async fn seed_repo_environment(
    pool: &PgPool,
    org_id: Uuid,
    repo_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO environments (id, org_id, repo_id, slug, name, requires_approval)
         VALUES ($1, $2, $3, 'test', 'Test', false)
         ON CONFLICT (repo_id, slug) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(repo_id)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO environments (id, org_id, repo_id, slug, name, requires_approval)
         VALUES ($1, $2, $3, 'production', 'Production', true)
         ON CONFLICT (repo_id, slug) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(repo_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn read_workflow_snapshot(
    bare: &std::path::Path,
    source_oid: &str,
) -> Result<ValidatedWorkflow, AppError> {
    let object = format!("{source_oid}:{PR_WORKFLOW_PATH}");
    let size = Command::new("git")
        .arg("--git-dir")
        .arg(bare)
        .args(["cat-file", "-s", &object])
        .output()
        .await
        .map_err(|error| AppError::Validation(format!("could not inspect PR workflow: {error}")))?;
    if !size.status.success() {
        return Err(AppError::Validation(format!(
            "PR head must contain {PR_WORKFLOW_PATH}"
        )));
    }
    let size = std::str::from_utf8(&size.stdout)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .ok_or_else(|| AppError::Validation("PR workflow object size is invalid".to_string()))?;
    if size > MAX_WORKFLOW_SOURCE_BYTES {
        return Err(AppError::Validation(format!(
            "PR workflow exceeds {MAX_WORKFLOW_SOURCE_BYTES} bytes"
        )));
    }
    let source = Command::new("git")
        .arg("--git-dir")
        .arg(bare)
        .args(["show", &object])
        .output()
        .await
        .map_err(|error| AppError::Validation(format!("could not read PR workflow: {error}")))?;
    if !source.status.success() {
        return Err(AppError::Validation(
            "PR workflow object is unavailable".to_string(),
        ));
    }
    let source = std::str::from_utf8(&source.stdout)
        .map_err(|_| AppError::Validation("PR workflow must be UTF-8 YAML".to_string()))?;
    parse_workflow(source).map_err(|error| AppError::Validation(error.to_string()))
}

#[allow(clippy::too_many_arguments)]
pub async fn trigger_pr_validation(
    state: &Arc<AppState>,
    org_id: Uuid,
    repo_id: Uuid,
    storage_key: Uuid,
    principal_id: Uuid,
    pr_id: Uuid,
    source_branch: &str,
    source_oid: &str,
) -> Result<(), AppError> {
    let bare = bare_repo_path(&state.git_root, &storage_key);
    let snapshot = read_workflow_snapshot(&bare, source_oid).await?;
    if !snapshot
        .definition
        .events
        .contains(&WorkflowEvent::PullRequest)
    {
        return Ok(());
    }
    let base_oid: Option<String> = sqlx::query_scalar(
        "SELECT base_oid FROM pull_requests WHERE id = $1 AND repo_id = $2 AND state = 'open'",
    )
    .bind(pr_id)
    .bind(repo_id)
    .fetch_optional(&state.pool)
    .await?
    .flatten();
    let Some(base_oid) = base_oid else {
        return Err(AppError::Validation(
            "pull request base is unavailable for validation".to_string(),
        ));
    };
    let key = format!("pull_request:{pr_id}:{source_oid}:{base_oid}");
    let _ = spawn_pipeline_run(
        state,
        org_id,
        repo_id,
        storage_key,
        source_oid.to_string(),
        "product-ci".to_string(),
        Some(snapshot),
        KIND_PR_VALIDATION,
        Some("pull_request"),
        Some(&format!("refs/heads/{source_branch}")),
        Some(&key),
        principal_id,
        Some(pr_id),
        None,
        None,
    )
    .await?;
    Ok(())
}

pub async fn trigger_git_ref_workflows(
    state: &Arc<AppState>,
    org_id: Uuid,
    repo_id: Uuid,
    storage_key: Uuid,
    principal_id: Uuid,
    updates: &[(String, String)],
) -> Result<(), AppError> {
    let bare = bare_repo_path(&state.git_root, &storage_key);
    for (git_ref, source_oid) in updates {
        let event = if git_ref.starts_with("refs/heads/") {
            WorkflowEvent::Push
        } else if git_ref.starts_with("refs/tags/") {
            WorkflowEvent::Tag
        } else {
            continue;
        };
        let workflow = match read_workflow_snapshot(&bare, source_oid).await {
            Ok(workflow) => workflow,
            Err(error) => {
                tracing::warn!(%git_ref, %source_oid, %error, "could not load workflow for Git ref update");
                continue;
            }
        };
        if workflow.definition.events.contains(&event) {
            let event_name = match event {
                WorkflowEvent::Push => "push",
                WorkflowEvent::Tag => "tag",
                _ => unreachable!(),
            };
            let trigger_key = format!("{event_name}:{git_ref}:{source_oid}");
            let _ = spawn_pipeline_run(
                state,
                org_id,
                repo_id,
                storage_key,
                source_oid.clone(),
                "product-ci".to_string(),
                Some(workflow.clone()),
                "workflow",
                Some(event_name),
                Some(git_ref),
                Some(&trigger_key),
                principal_id,
                None,
                None,
                None,
            )
            .await?;
        }
        if event == WorkflowEvent::Push {
            let branch = git_ref.trim_start_matches("refs/heads/");
            let prs = sqlx::query_as::<_, (Uuid, Option<String>)>(
                "SELECT id, base_oid FROM pull_requests
                 WHERE repo_id = $1 AND source_branch = $2 AND state = 'open'",
            )
            .bind(repo_id)
            .bind(branch)
            .fetch_all(&state.pool)
            .await?;
            for (pr_id, base_oid) in prs {
                let Some(base_oid) = base_oid else {
                    continue;
                };
                let changed = sqlx::query(
                    "UPDATE pull_requests SET head_oid = $2, row_version = row_version + 1,
                     updated_at = CURRENT_TIMESTAMP
                     WHERE id = $1 AND state = 'open' AND head_oid IS DISTINCT FROM $2",
                )
                .bind(pr_id)
                .bind(source_oid)
                .execute(&state.pool)
                .await?;
                if changed.rows_affected() == 0 {
                    continue;
                }
                crate::pr::insert_pr_checks_for_pipeline(
                    &state.pool,
                    org_id,
                    repo_id,
                    pr_id,
                    source_oid,
                )
                .await?;
                if workflow
                    .definition
                    .events
                    .contains(&WorkflowEvent::PullRequest)
                {
                    let key = format!("pull_request:{pr_id}:{source_oid}:{base_oid}");
                    let _ = spawn_pipeline_run(
                        state,
                        org_id,
                        repo_id,
                        storage_key,
                        source_oid.clone(),
                        "product-ci".to_string(),
                        Some(workflow.clone()),
                        KIND_PR_VALIDATION,
                        Some("pull_request"),
                        Some(git_ref),
                        Some(&key),
                        principal_id,
                        Some(pr_id),
                        None,
                        None,
                    )
                    .await?;
                }
            }
        }
    }
    Ok(())
}

pub async fn run_git_ref_trigger_dispatcher(state: Arc<AppState>) {
    let mut next_reconcile = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        match dispatch_one_git_ref_trigger(&state).await {
            Ok(true) => continue,
            Ok(false) => {
                if tokio::time::Instant::now() >= next_reconcile {
                    if let Err(error) = reconcile_git_ref_trigger_outbox(&state).await {
                        tracing::error!(%error, "Git ref trigger outbox reconciliation failed");
                    }
                    next_reconcile = tokio::time::Instant::now() + Duration::from_secs(60);
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(error) => {
                tracing::error!(%error, "Git ref trigger outbox dispatch failed");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

/// Reconstructs missing push events from durable Git refs after a receive-pack/outbox crash.
/// The outbox unique key makes repeated scans safe. On the first scan after enabling this
/// reconciler, refs without any historical outbox row are treated as recoverable pushes.
async fn reconcile_git_ref_trigger_outbox(state: &Arc<AppState>) -> Result<(), AppError> {
    let repositories = sqlx::query_as::<_, (Uuid, Uuid, Uuid, Uuid)>(
        "SELECT r.org_id, r.id, r.storage_key, g.principal_id
         FROM repositories r
         JOIN LATERAL (
             SELECT principal_id FROM repository_grants
             WHERE repo_id = r.id
             ORDER BY created_at, id LIMIT 1
         ) g ON TRUE
         WHERE r.state = 'ready'
         ORDER BY r.id",
    )
    .fetch_all(&state.pool)
    .await?;

    for (org_id, repo_id, storage_key, principal_id) in repositories {
        let bare = bare_repo_path(&state.git_root, &storage_key);
        let refs = match super::git_ref_snapshot(&bare).await {
            Ok(refs) => refs,
            Err(error) => {
                tracing::warn!(repo_id = %repo_id, %error, "Git ref reconciliation snapshot failed");
                continue;
            }
        };
        for (git_ref, source_oid) in refs {
            sqlx::query(
                "INSERT INTO git_ref_trigger_outbox
                 (id, org_id, repo_id, storage_key, principal_id, git_ref, source_oid)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
                 ON CONFLICT (repo_id, git_ref, source_oid) DO NOTHING",
            )
            .bind(Uuid::new_v4())
            .bind(org_id)
            .bind(repo_id)
            .bind(storage_key)
            .bind(principal_id)
            .bind(git_ref)
            .bind(source_oid)
            .execute(&state.pool)
            .await?;
        }
    }
    Ok(())
}

async fn dispatch_one_git_ref_trigger(state: &Arc<AppState>) -> Result<bool, AppError> {
    let event = sqlx::query_as::<_, (Uuid, Uuid, Uuid, Uuid, Uuid, String, String)>(
        "WITH candidate AS (
             SELECT id FROM git_ref_trigger_outbox
             WHERE state = 'pending'
                OR (state = 'processing' AND updated_at < CURRENT_TIMESTAMP - INTERVAL '2 minutes')
             ORDER BY created_at, id
             FOR UPDATE SKIP LOCKED
             LIMIT 1
         )
         UPDATE git_ref_trigger_outbox e
         SET state = 'processing', attempts = attempts + 1, updated_at = CURRENT_TIMESTAMP
         FROM candidate c WHERE e.id = c.id
         RETURNING e.id, e.org_id, e.repo_id, e.storage_key, e.principal_id, e.git_ref, e.source_oid",
    )
    .fetch_optional(&state.pool)
    .await?;
    let Some((event_id, org_id, repo_id, storage_key, principal_id, git_ref, source_oid)) = event
    else {
        return Ok(false);
    };

    let dispatch = trigger_git_ref_workflows(
        state,
        org_id,
        repo_id,
        storage_key,
        principal_id,
        &[(git_ref, source_oid)],
    )
    .await;
    match dispatch {
        Ok(()) => {
            sqlx::query(
                "UPDATE git_ref_trigger_outbox SET state = 'completed', last_error = NULL,
                 completed_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = $1",
            )
            .bind(event_id)
            .execute(&state.pool)
            .await?;
        }
        Err(error) => {
            sqlx::query(
                "UPDATE git_ref_trigger_outbox SET state = 'pending', last_error = $2,
                 updated_at = CURRENT_TIMESTAMP WHERE id = $1",
            )
            .bind(event_id)
            .bind(error.to_string())
            .execute(&state.pool)
            .await?;
            return Err(error);
        }
    }
    Ok(true)
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
    let workflow_snapshot = read_workflow_snapshot(&bare, &head_oid).await?;
    if !workflow_snapshot
        .definition
        .events
        .contains(&WorkflowEvent::PullRequest)
    {
        return Err(AppError::Validation(
            "PR workflow must declare the pull_request event".to_string(),
        ));
    }
    let workflow = "product-ci".to_string();
    spawn_pipeline_run(
        &state,
        org_id,
        repo_id,
        storage_key,
        head_oid.clone(),
        workflow,
        Some(workflow_snapshot),
        KIND_PR_VALIDATION,
        Some("pull_request"),
        Some(&format!("refs/heads/{source_branch}")),
        Some(&format!("pull_request:{pr_id}:{head_oid}")),
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
        None,
        KIND_RELEASE,
        None,
        None,
        None,
        principal_id,
        Some(pr_id),
        issue_id,
        agent_run_id,
    )
    .await
}

pub async fn start_manual_workflow_run(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo)): AxumPath<(String, String)>,
    Json(body): Json<StartManualWorkflowRequest>,
) -> Result<(StatusCode, Json<PipelineRunView>), AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id, storage_key) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    validate_git_oid(&body.source_oid).map_err(AppError::Validation)?;
    let bare = bare_repo_path(&state.git_root, &storage_key);
    let object = Command::new("git")
        .arg("--git-dir")
        .arg(&bare)
        .args(["cat-file", "-e"])
        .arg(format!("{}^{{commit}}", body.source_oid))
        .output()
        .await
        .map_err(|error| {
            AppError::Other(format!("could not inspect manual workflow source: {error}"))
        })?;
    if !object.status.success() {
        return Err(AppError::Validation(
            "source_oid is not a commit in this repository".to_string(),
        ));
    }
    let snapshot = read_workflow_snapshot(&bare, &body.source_oid).await?;
    if !snapshot.definition.events.contains(&WorkflowEvent::Manual) {
        return Err(AppError::Validation(
            "workflow does not declare the manual event".to_string(),
        ));
    }
    spawn_pipeline_run(
        &state,
        org_id,
        repo_id,
        storage_key,
        body.source_oid,
        "product-ci".to_string(),
        Some(snapshot),
        "workflow",
        Some("manual"),
        None,
        None,
        principal_id,
        None,
        None,
        None,
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
pub(crate) async fn spawn_pipeline_run(
    state: &Arc<AppState>,
    org_id: Uuid,
    repo_id: Uuid,
    storage_key: Uuid,
    source_oid: String,
    workflow: String,
    workflow_snapshot: Option<ValidatedWorkflow>,
    run_kind: &str,
    trigger_event: Option<&str>,
    trigger_ref: Option<&str>,
    trigger_key: Option<&str>,
    principal_id: Uuid,
    pr_id: Option<Uuid>,
    issue_id: Option<Uuid>,
    agent_run_id: Option<Uuid>,
) -> Result<(StatusCode, Json<PipelineRunView>), AppError> {
    let run_id = Uuid::new_v4();
    let attempt_id = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    let remote = crate::runner_api::pipeline_executor_remote();
    let instance_id = if remote {
        None
    } else {
        Some(state.instance_id)
    };
    let mut tx = state.pool.begin().await?;
    let pr_base_oid = if run_kind == KIND_PR_VALIDATION {
        let Some(pr_id) = pr_id else {
            return Err(AppError::Validation(
                "PR validation pipeline requires a pull request id".to_string(),
            ));
        };
        let row = sqlx::query_as::<_, (Option<String>,)>(
            "SELECT base_oid FROM pull_requests WHERE id = $1 AND state = 'open' FOR SHARE",
        )
        .bind(pr_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((Some(base_oid),)) = row else {
            return Err(AppError::Validation(
                "PR validation pipeline requires an open pull request base".to_string(),
            ));
        };
        Some(base_oid)
    } else {
        None
    };
    if let Some(key) = trigger_key {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(key)
            .execute(&mut *tx)
            .await?;
        let existing = sqlx::query_as::<_, (Uuid, String, String, String, Uuid, Option<String>)>(
            "SELECT id, source_oid, workflow_name, state, run_attempt_id, error_code
             FROM pipeline_runs WHERE repo_id = $1 AND trigger_key = $2",
        )
        .bind(repo_id)
        .bind(key)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((id, source_oid, workflow_name, run_state, attempt_id, error_code)) = existing {
            tx.commit().await?;
            return Ok((
                StatusCode::OK,
                Json(PipelineRunView {
                    id,
                    source_oid,
                    workflow_name,
                    state: run_state,
                    run_attempt_id: attempt_id,
                    error_code,
                }),
            ));
        }
    }
    let lease_sql = if remote {
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state,
         requested_by, run_attempt_id, pr_id, issue_id, agent_run_id, run_kind,
         trigger_event, trigger_ref, trigger_key, pr_base_oid)
         VALUES ($1,$2,$3,$4,$5,'queued',$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)"
    } else {
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state,
         requested_by, run_attempt_id, pr_id, issue_id, agent_run_id, run_kind,
         trigger_event, trigger_ref, trigger_key, pr_base_oid, lease_holder, lease_until)
         VALUES ($1,$2,$3,$4,$5,'queued',$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,
           CURRENT_TIMESTAMP + ($17::text || ' seconds')::interval)"
    };
    if remote {
        sqlx::query(lease_sql)
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
            .bind(trigger_event)
            .bind(trigger_ref)
            .bind(trigger_key)
            .bind(pr_base_oid.as_deref())
            .execute(&mut *tx)
            .await?;
    } else {
        sqlx::query(lease_sql)
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
            .bind(trigger_event)
            .bind(trigger_ref)
            .bind(trigger_key)
            .bind(pr_base_oid.as_deref())
            .bind(instance_id.unwrap())
            .bind(crate::lease::lease_secs())
            .execute(&mut *tx)
            .await?;
    }
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
    if let Some(snapshot) = workflow_snapshot {
        let definition = serde_json::to_value(&snapshot).map_err(|error| {
            AppError::Validation(format!("workflow snapshot serialization failed: {error}"))
        })?;
        sqlx::query(
            "INSERT INTO pipeline_workflow_snapshots
             (pipeline_run_id, source_oid, workflow_path, content_hash, definition)
             VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(run_id)
        .bind(&source_oid)
        .bind(PR_WORKFLOW_PATH)
        .bind(&snapshot.sha256)
        .bind(definition)
        .execute(&mut *tx)
        .await?;

        let mut workflow_job_ids = std::collections::BTreeMap::new();
        for workflow_job in &snapshot.definition.jobs {
            let workflow_job_id = Uuid::new_v4();
            workflow_job_ids.insert(workflow_job.id.clone(), workflow_job_id);
            sqlx::query(
                "INSERT INTO pipeline_workflow_jobs
                 (id, pipeline_run_id, job_key, image_ref, command)
                 VALUES ($1,$2,$3,$4,$5)",
            )
            .bind(workflow_job_id)
            .bind(run_id)
            .bind(&workflow_job.id)
            .bind(&workflow_job.image)
            .bind(
                serde_json::to_value(&workflow_job.command).map_err(|error| {
                    AppError::Validation(format!("workflow command serialization failed: {error}"))
                })?,
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO pipeline_workflow_job_attempts
                 (id, workflow_job_id, attempt_number, state)
                 VALUES ($1,$2,1,'queued')",
            )
            .bind(Uuid::new_v4())
            .bind(workflow_job_id)
            .execute(&mut *tx)
            .await?;
        }
        for workflow_job in &snapshot.definition.jobs {
            let workflow_job_id = workflow_job_ids
                .get(&workflow_job.id)
                .copied()
                .ok_or_else(|| AppError::Validation("workflow job id missing".to_string()))?;
            for dependency in &workflow_job.needs {
                let dependency_id = workflow_job_ids.get(dependency).copied().ok_or_else(|| {
                    AppError::Validation("workflow dependency missing".to_string())
                })?;
                sqlx::query(
                    "INSERT INTO pipeline_workflow_job_dependencies (job_id, needs_job_id)
                     VALUES ($1,$2)",
                )
                .bind(workflow_job_id)
                .bind(dependency_id)
                .execute(&mut *tx)
                .await?;
            }
        }
    }
    crate::audit::record_in_tx(&mut tx, Some(org_id), Some(repo_id), principal_id, None,
        "pipeline.queued", "pipeline_run", run_id, None, "allow",
        serde_json::json!({ "workflow_name": workflow, "source_oid": source_oid, "run_kind": run_kind }))
        .await?;
    tx.commit().await?;

    if !remote {
        let pool = state.pool.clone();
        let git_root = state.git_root.clone();
        let artifact_root = state.artifact_root.clone();
        let pipeline_bin = state.pipeline_adapter_bin.clone();
        let supervisor = state.pipeline_supervisor.clone();
        let instance_id = state.instance_id;
        let source_oid_spawn = source_oid.clone();
        let run_kind_owned = run_kind.to_string();
        tokio::spawn(async move {
            execute_pipeline_job(
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
    }

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

pub async fn list_pipeline_runs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo)): AxumPath<(String, String)>,
    Query(query): Query<ListPipelineRunsQuery>,
) -> Result<Json<PipelineRunHistoryPage>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (_, repo_id, _) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(AppError::Validation(
            "pipeline run limit must be 1..=100".into(),
        ));
    }
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            String,
            Uuid,
            Option<String>,
            time::OffsetDateTime,
        ),
    >(
        "SELECT id, source_oid, workflow_name, state, run_attempt_id, error_code, created_at
         FROM pipeline_runs
         WHERE repo_id = $1
           AND ($2::uuid IS NULL OR EXISTS (
               SELECT 1 FROM pipeline_runs AS cursor_row
               WHERE cursor_row.id = $2 AND cursor_row.repo_id = $1
                 AND (pipeline_runs.created_at, pipeline_runs.id) < (cursor_row.created_at, cursor_row.id)
           ))
         ORDER BY created_at DESC, id DESC LIMIT $3",
    )
    .bind(repo_id)
    .bind(query.before_id)
    .bind(limit + 1)
    .fetch_all(&state.pool)
    .await?;
    let has_more = rows.len() > limit as usize;
    let items = rows
        .into_iter()
        .take(limit as usize)
        .map(
            |(id, source_oid, workflow_name, state, run_attempt_id, error_code, created_at)| {
                PipelineRunHistoryView {
                    id,
                    source_oid,
                    workflow_name,
                    state,
                    run_attempt_id,
                    error_code,
                    created_at: created_at.to_string(),
                }
            },
        )
        .collect::<Vec<_>>();
    let next_cursor = if has_more {
        items.last().map(|item| item.id)
    } else {
        None
    };
    Ok(Json(PipelineRunHistoryPage {
        items,
        next_cursor,
        has_more,
    }))
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

pub async fn list_pipeline_jobs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, run_id)): AxumPath<(String, String, Uuid)>,
) -> Result<Json<Vec<PipelineJobView>>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (_, repo_id, _) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let run_ok =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM pipeline_runs WHERE id = $1 AND repo_id = $2")
            .bind(run_id)
            .bind(repo_id)
            .fetch_optional(&state.pool)
            .await?;
    if run_ok.is_none() {
        return Err(AppError::NotFound);
    }
    let workflow_rows = sqlx::query_as::<
        _,
        (
            Uuid,
            Uuid,
            String,
            String,
            Option<Uuid>,
            String,
            Option<serde_json::Value>,
            serde_json::Value,
        ),
    >(
        "SELECT j.id, j.pipeline_run_id, j.job_key, j.state, a.id, r.state, a.result,
         COALESCE((
           SELECT jsonb_agg(jsonb_build_object(
             'id', art.id, 'name', art.name, 'kind', art.kind,
             'sha256', art.sha256, 'byte_length', art.byte_length
           ) ORDER BY art.name)
           FROM artifacts art
           WHERE art.workflow_attempt_id = a.id AND art.state = 'ready'
         ), '[]'::jsonb)
         FROM pipeline_workflow_jobs j
         JOIN pipeline_runs r ON r.id = j.pipeline_run_id
         LEFT JOIN LATERAL (
           SELECT id, result FROM pipeline_workflow_job_attempts
           WHERE workflow_job_id = j.id ORDER BY attempt_number DESC LIMIT 1
         ) a ON TRUE
         WHERE j.pipeline_run_id = $1 ORDER BY j.job_key",
    )
    .bind(run_id)
    .fetch_all(&state.pool)
    .await?;
    if !workflow_rows.is_empty() {
        return Ok(Json(
            workflow_rows
                .into_iter()
                .map(
                    |(
                        id,
                        pipeline_run_id,
                        job_key,
                        job_state,
                        attempt_id,
                        run_state,
                        result,
                        artifacts,
                    )| {
                        let mut artifacts =
                            serde_json::from_value::<Vec<PipelineArtifactView>>(artifacts)
                                .map_err(|error| AppError::Other(error.to_string()))?;
                        for artifact in &mut artifacts {
                            artifact.download_url = format!(
                            "/api/v1/orgs/{org}/repos/{repo}/pipeline-runs/{run_id}/artifacts/{}",
                            artifact.id
                        );
                        }
                        Ok(PipelineJobView {
                            id,
                            pipeline_run_id,
                            job_key,
                            retry_allowed: job_state == "failed" && run_state == "failed",
                            state: job_state,
                            attempt_id,
                            result: result.filter(|value| {
                                value.as_object().is_some_and(|object| !object.is_empty())
                            }),
                            artifacts,
                        })
                    },
                )
                .collect::<Result<Vec<_>, AppError>>()?,
        ));
    }
    let rows = sqlx::query_as::<_, (Uuid, Uuid, String, String)>(
        "SELECT id, pipeline_run_id, job_key, state FROM pipeline_jobs
         WHERE pipeline_run_id = $1 AND repo_id = $2 ORDER BY job_key",
    )
    .bind(run_id)
    .bind(repo_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(
                |(id, pipeline_run_id, job_key, job_state)| PipelineJobView {
                    id,
                    pipeline_run_id,
                    job_key: job_key.clone(),
                    state: job_state.clone(),
                    retry_allowed: job_key != "deploy" && job_state == "failed",
                    attempt_id: None,
                    result: None,
                    artifacts: Vec::new(),
                },
            )
            .collect(),
    ))
}

pub async fn download_pipeline_artifact(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, run_id, artifact_id)): AxumPath<(String, String, Uuid, Uuid)>,
) -> Result<Response, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id, _) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let row = sqlx::query_as::<_, (String, String, i64, String)>(
        "SELECT name, sha256, byte_length, storage_path FROM artifacts
         WHERE id = $1 AND org_id = $2 AND repo_id = $3 AND pipeline_run_id = $4
           AND workflow_attempt_id IS NOT NULL AND kind IN ('file', 'junit') AND state = 'ready'",
    )
    .bind(artifact_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(run_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    let (name, expected_sha, expected_length, storage_path) = row;
    let path = safe_join(Path::new(&storage_path), &name)?;
    let bytes = read_bounded_regular_file(&path)?;
    let digest = Sha256::digest(&bytes);
    let actual_sha = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if actual_sha != expected_sha || bytes.len() as i64 != expected_length {
        sqlx::query("UPDATE artifacts SET state = 'quarantined', updated_at = CURRENT_TIMESTAMP WHERE id = $1")
            .bind(artifact_id)
            .execute(&state.pool)
            .await?;
        return Err(AppError::Validation(
            "workflow artifact integrity check failed".to_string(),
        ));
    }
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/octet-stream")
        .header(
            "content-disposition",
            format!("attachment; filename=\"{name}\""),
        )
        .header("cache-control", "no-store")
        .body(Body::from(bytes))
        .map_err(|error| AppError::Other(format!("build artifact response: {error}")))
}

/// Retry a failed pipeline job (shared by session HTTP and MCP).
pub async fn retry_pipeline_job(
    state: &Arc<AppState>,
    repo_id: Uuid,
    job_id: Uuid,
) -> Result<PipelineJobView, AppError> {
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT pipeline_run_id, job_key, state FROM pipeline_jobs
         WHERE id = $1 AND repo_id = $2
         FOR UPDATE",
    )
    .bind(job_id)
    .bind(repo_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((run_id, job_key, job_state)) = row else {
        tx.rollback().await?;
        return retry_workflow_job(state, repo_id, job_id).await;
    };
    if job_key == "deploy" {
        return Err(AppError::Validation(
            "deploy job cannot use generic retry".to_string(),
        ));
    }
    if job_state != "failed" {
        return Err(AppError::Validation(
            "only failed jobs may be retried".to_string(),
        ));
    }
    let run_row = sqlx::query_as::<_, (String,)>(
        "SELECT state FROM pipeline_runs WHERE id = $1 AND repo_id = $2 FOR UPDATE",
    )
    .bind(run_id)
    .bind(repo_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((run_state,)) = run_row else {
        return Err(AppError::NotFound);
    };
    if run_state != "failed" {
        return Err(AppError::Conflict(
            "pipeline run must be failed to retry a failed job".to_string(),
        ));
    }
    let job_up = sqlx::query(
        "UPDATE pipeline_jobs SET state = 'queued', updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND repo_id = $2 AND job_key <> 'deploy' AND state = 'failed'",
    )
    .bind(job_id)
    .bind(repo_id)
    .execute(&mut *tx)
    .await?;
    if job_up.rows_affected() == 0 {
        return Err(AppError::Validation(
            "only failed jobs may be retried".to_string(),
        ));
    }
    let run_up = sqlx::query(
        "UPDATE pipeline_runs SET state = 'queued', error_code = NULL, updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND repo_id = $2 AND state = 'failed'",
    )
    .bind(run_id)
    .bind(repo_id)
    .execute(&mut *tx)
    .await?;
    if run_up.rows_affected() == 0 {
        return Err(AppError::Conflict(
            "pipeline run state changed; job retry aborted".to_string(),
        ));
    }
    let spawn = if !crate::runner_api::pipeline_executor_remote() {
        let (storage_key, org_id): (Uuid, Uuid) =
            sqlx::query_as("SELECT storage_key, org_id FROM repositories WHERE id = $1")
                .bind(repo_id)
                .fetch_one(&mut *tx)
                .await?;
        let source: (String,) =
            sqlx::query_as("SELECT source_oid FROM pipeline_runs WHERE id = $1")
                .bind(run_id)
                .fetch_one(&mut *tx)
                .await?;
        let run_kind: (String,) =
            sqlx::query_as("SELECT run_kind FROM pipeline_runs WHERE id = $1")
                .bind(run_id)
                .fetch_one(&mut *tx)
                .await?;
        Some((storage_key, org_id, source.0, run_kind.0))
    } else {
        None
    };
    tx.commit().await?;
    if let Some((storage_key, org_id, source_oid, run_kind)) = spawn {
        let pool = state.pool.clone();
        let git_root = state.git_root.clone();
        let artifact_root = state.artifact_root.clone();
        let pipeline_bin = state.pipeline_adapter_bin.clone();
        let supervisor = state.pipeline_supervisor.clone();
        let instance_id = state.instance_id;
        tokio::spawn(async move {
            execute_pipeline_job(
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
                source_oid,
                run_kind,
            )
            .await;
        });
    }
    Ok(PipelineJobView {
        id: job_id,
        pipeline_run_id: run_id,
        job_key,
        state: "queued".to_string(),
        retry_allowed: false,
        attempt_id: None,
        result: None,
        artifacts: Vec::new(),
    })
}

async fn retry_workflow_job(
    state: &Arc<AppState>,
    repo_id: Uuid,
    job_id: Uuid,
) -> Result<PipelineJobView, AppError> {
    let mut tx = state.pool.begin().await?;
    let run_id = sqlx::query_as::<_, (Uuid,)>(
        "SELECT pipeline_run_id FROM pipeline_workflow_jobs WHERE id = $1",
    )
    .bind(job_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?
    .0;
    let run_state = sqlx::query_as::<_, (String,)>(
        "SELECT state FROM pipeline_runs WHERE id = $1 AND repo_id = $2 FOR UPDATE",
    )
    .bind(run_id)
    .bind(repo_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?
    .0;
    let (job_key, job_state) = sqlx::query_as::<_, (String, String)>(
        "SELECT job_key, state FROM pipeline_workflow_jobs
         WHERE id = $1 AND pipeline_run_id = $2 FOR UPDATE",
    )
    .bind(job_id)
    .bind(run_id)
    .fetch_one(&mut *tx)
    .await?;
    if job_state != "failed" {
        return Err(AppError::Validation(
            "only failed workflow jobs may be retried".to_string(),
        ));
    }
    if run_state != "failed" {
        return Err(AppError::Conflict(
            "pipeline run must be failed to retry a workflow job".to_string(),
        ));
    }
    let pipeline_job = sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM pipeline_jobs WHERE pipeline_run_id = $1 AND repo_id = $2
         ORDER BY created_at LIMIT 1 FOR UPDATE",
    )
    .bind(run_id)
    .bind(repo_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?
    .0;

    // Resume the failed DAG from every non-success node. Successful nodes retain their
    // results; failed and skipped nodes receive fresh attempts and execute in DAG order.
    let pending = sqlx::query_as::<_, (Uuid, i32)>(
        "SELECT j.id, COALESCE(MAX(a.attempt_number), 0)
         FROM pipeline_workflow_jobs j
         LEFT JOIN pipeline_workflow_job_attempts a ON a.workflow_job_id = j.id
         WHERE j.pipeline_run_id = $1 AND j.state <> 'succeeded'
         GROUP BY j.id ORDER BY j.id",
    )
    .bind(run_id)
    .fetch_all(&mut *tx)
    .await?;
    for (workflow_job_id, previous_attempt) in &pending {
        let attempt_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO pipeline_workflow_job_attempts
             (id, workflow_job_id, attempt_number, state)
             VALUES ($1, $2, $3, 'queued')",
        )
        .bind(attempt_id)
        .bind(workflow_job_id)
        .bind(previous_attempt + 1)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "UPDATE pipeline_workflow_jobs SET state = 'queued', error_code = NULL,
         started_at = NULL, completed_at = NULL, attempt_count = attempt_count + 1,
         updated_at = CURRENT_TIMESTAMP
         WHERE pipeline_run_id = $1 AND state <> 'succeeded'",
    )
    .bind(run_id)
    .execute(&mut *tx)
    .await?;
    let new_run_attempt_id = Uuid::new_v4();
    let run_update = sqlx::query(
        "UPDATE pipeline_runs SET state = 'queued', error_code = NULL,
         cancel_requested = FALSE, run_attempt_id = $3, lease_holder = NULL,
         lease_until = NULL, lease_epoch = lease_epoch + 1,
         updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND repo_id = $2 AND state = 'failed'",
    )
    .bind(run_id)
    .bind(repo_id)
    .bind(new_run_attempt_id)
    .execute(&mut *tx)
    .await?;
    if run_update.rows_affected() != 1 {
        return Err(AppError::Conflict(
            "pipeline run state changed; workflow retry aborted".to_string(),
        ));
    }
    let parent_update = sqlx::query(
        "UPDATE pipeline_jobs SET state = 'queued', updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND state = 'failed'",
    )
    .bind(pipeline_job)
    .execute(&mut *tx)
    .await?;
    if parent_update.rows_affected() != 1 {
        return Err(AppError::Conflict(
            "pipeline job state changed; workflow retry aborted".to_string(),
        ));
    }
    let (storage_key, repo_org_id): (Uuid, Uuid) =
        sqlx::query_as("SELECT storage_key, org_id FROM repositories WHERE id = $1")
            .bind(repo_id)
            .fetch_one(&mut *tx)
            .await?;
    let source_oid: (String,) =
        sqlx::query_as("SELECT source_oid FROM pipeline_runs WHERE id = $1")
            .bind(run_id)
            .fetch_one(&mut *tx)
            .await?;
    let run_kind: (String,) = sqlx::query_as("SELECT run_kind FROM pipeline_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;

    if !crate::runner_api::pipeline_executor_remote() {
        let pool = state.pool.clone();
        let git_root = state.git_root.clone();
        let artifact_root = state.artifact_root.clone();
        let pipeline_bin = state.pipeline_adapter_bin.clone();
        let supervisor = state.pipeline_supervisor.clone();
        let instance_id = state.instance_id;
        tokio::spawn(async move {
            execute_pipeline_job(
                pool,
                git_root,
                artifact_root,
                pipeline_bin,
                supervisor,
                instance_id,
                storage_key,
                run_id,
                pipeline_job,
                repo_org_id,
                repo_id,
                source_oid.0,
                run_kind.0,
            )
            .await;
        });
    }
    let latest_attempt = sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM pipeline_workflow_job_attempts
         WHERE workflow_job_id = $1 ORDER BY attempt_number DESC LIMIT 1",
    )
    .bind(job_id)
    .fetch_one(&state.pool)
    .await?
    .0;
    Ok(PipelineJobView {
        id: job_id,
        pipeline_run_id: run_id,
        job_key,
        state: "queued".to_string(),
        retry_allowed: false,
        attempt_id: Some(latest_attempt),
        result: None,
        artifacts: Vec::new(),
    })
}

pub async fn retry_pipeline_job_http(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, job_id)): AxumPath<(String, String, Uuid)>,
    Json(body): Json<RetryPipelineJobRequest>,
) -> Result<Json<PipelineJobView>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id, _) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let attempt_id = body.attempt_id.ok_or_else(|| {
        AppError::Validation("attempt_id is required for pipeline job retry".to_string())
    })?;
    let canonical = json!({
        "job_id": job_id.to_string(),
        "repo_id": repo_id.to_string(),
    });
    let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
    let action = begin_idempotent_write(
        &state.pool,
        principal_id,
        Some(org_id),
        "http_pipeline_job_retry",
        &attempt_id.to_string(),
        &canonical_json,
    )
    .await?;
    if let IdempotencyAction::Replay(stored) = action {
        return Ok(Json(serde_json::from_value(stored).map_err(|_| {
            AppError::Other("idempotency replay decode failed".to_string())
        })?));
    }
    let IdempotencyAction::Proceed { record_id } = action else {
        return Err(AppError::Other("idempotency".into()));
    };
    match retry_pipeline_job(&state, repo_id, job_id).await {
        Ok(view) => {
            let stored = serde_json::to_value(&view).map_err(|e| AppError::Other(e.to_string()))?;
            let _ = complete_idempotent_write(&state.pool, record_id, &stored, Some(job_id)).await;
            Ok(Json(view))
        }
        Err(e) => {
            let _ = fail_idempotent_write(&state.pool, record_id).await;
            Err(e)
        }
    }
}

#[derive(Deserialize)]
pub struct PipelineJobLogsQuery {
    pub attempt_id: Uuid,
    pub cursor: Option<i32>,
    pub max_bytes: Option<usize>,
}

pub async fn get_pipeline_job_logs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, job_id)): AxumPath<(String, String, Uuid)>,
    Query(query): Query<PipelineJobLogsQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (_, repo_id, _) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let max_bytes = query.max_bytes.unwrap_or(8_192).clamp(1, 262_144);
    let body = crate::mcp::contract_domain::job_logs(
        &state.pool,
        repo_id,
        job_id,
        query.attempt_id,
        query.cursor,
        max_bytes,
    )
    .await?;
    Ok(Json(body))
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
pub async fn execute_pipeline_job(
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
    let work_lease = match crate::lease::try_begin_pipeline_work(&pool, run_id, instance_id).await {
        Ok(lease) => lease,
        Err(_) => {
            if let Some(lease) =
                crate::lease::lookup_pipeline_work_lease(&pool, run_id, instance_id).await
            {
                lease
            } else {
                let _ = sqlx::query(
                    "UPDATE pipeline_runs SET state = 'failed', error_code = 'lease_held_by_peer',
                     updated_at = CURRENT_TIMESTAMP WHERE id = $1 AND state = 'queued'",
                )
                .bind(run_id)
                .execute(&pool)
                .await;
                return;
            }
        }
    };
    let _renew_guard = crate::lease::pipeline_lease_renew_guard(
        pool.clone(),
        run_id,
        instance_id,
        work_lease.epoch,
    );
    let _ = sqlx::query(
        "UPDATE pipeline_jobs SET state = 'running', updated_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(job_id)
    .execute(&pool)
    .await;

    if cancel_flag(&pool, run_id).await {
        let _ = mark_canceled(&pool, run_id, job_id, instance_id, work_lease.epoch).await;
        return;
    }
    if run_kind != KIND_RELEASE {
        execute_pr_validation_job(
            &pool,
            &git_root,
            &artifact_root,
            &pipeline_bin,
            &supervisor,
            storage_key,
            instance_id,
            run_id,
            job_id,
            org_id,
            repo_id,
            &source_oid,
            work_lease.epoch,
        )
        .await;
        supervisor.finish(run_id).await;
        return;
    }

    let artifact_id = Uuid::new_v4();
    let dir = artifact_storage_dir(&artifact_root, org_id, repo_id, artifact_id);
    if std::fs::create_dir_all(&dir).is_err() {
        let _ = mark_failed(
            &pool,
            run_id,
            job_id,
            instance_id,
            work_lease.epoch,
            "artifact_dir_failed",
        )
        .await;
        return;
    }
    let file_path = match safe_join(&dir, ARTIFACT_NAME) {
        Ok(path) => path,
        Err(_) => {
            let _ = mark_failed(
                &pool,
                run_id,
                job_id,
                instance_id,
                work_lease.epoch,
                "artifact_path_failed",
            )
            .await;
            return;
        }
    };
    let _ = sqlx::query(
        "INSERT INTO artifacts (id, org_id, repo_id, pipeline_run_id, name, kind, sha256, byte_length,
         storage_path, state)
         VALUES ($1,$2,$3,$4,$5,'file','',0,$6,'uploading')",
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
        .env("MUSEHUB_PIPELINE_RUN_ID", run_id.to_string())
        .env("MUSEHUB_PIPELINE_REPO_ID", repo_id.to_string());
    if let Ok(reg) = std::env::var("MUSEHUB_OCI_REGISTRY") {
        cmd.env("MUSEHUB_OCI_REGISTRY", reg);
    }
    if let Ok(hold) = std::env::var("MUSEHUB_PIPELINE_HOLD_SECS") {
        cmd.env("MUSEHUB_PIPELINE_HOLD_SECS", hold);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_child_process_group(&mut cmd);
    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(_) => {
            let _ = quarantine_artifact(&pool, artifact_id).await;
            let _ = mark_failed(
                &pool,
                run_id,
                job_id,
                instance_id,
                work_lease.epoch,
                "adapter_spawn_failed",
            )
            .await;
            return;
        }
    };
    if let Some(pid) = child.id() {
        supervisor.register(run_id, pid).await;
    }

    let output = match wait_pipeline_child(child, &pool, run_id).await {
        Ok(out) => out,
        Err(ChildWaitOutcome::Canceled) => {
            let _ = supervisor.cancel(run_id).await;
            let _ = quarantine_artifact(&pool, artifact_id).await;
            let _ = mark_canceled(&pool, run_id, job_id, instance_id, work_lease.epoch).await;
            supervisor.finish(run_id).await;
            return;
        }
        Err(ChildWaitOutcome::TimedOut) => {
            let _ = supervisor.cancel(run_id).await;
            let _ = quarantine_artifact(&pool, artifact_id).await;
            let _ = mark_failed(
                &pool,
                run_id,
                job_id,
                instance_id,
                work_lease.epoch,
                "pipeline_timeout",
            )
            .await;
            supervisor.finish(run_id).await;
            return;
        }
        Err(ChildWaitOutcome::IoFailed) => {
            let _ = quarantine_artifact(&pool, artifact_id).await;
            let _ = mark_failed(
                &pool,
                run_id,
                job_id,
                instance_id,
                work_lease.epoch,
                "pipeline_adapter_failed",
            )
            .await;
            supervisor.finish(run_id).await;
            return;
        }
    };
    if !output.status.success() {
        let _ = quarantine_artifact(&pool, artifact_id).await;
        let _ = mark_failed(
            &pool,
            run_id,
            job_id,
            instance_id,
            work_lease.epoch,
            "pipeline_adapter_failed",
        )
        .await;
        return;
    }
    let oci_from_adapter = parse_pipeline_stdout(&output.stdout);
    match artifacts::sha256_file(&file_path) {
        Ok((sha, len)) => {
            if finalize_artifact(&pool, artifact_id, &sha, len as i64)
                .await
                .is_err()
            {
                let _ = std::fs::remove_file(&file_path);
                let _ = mark_failed(
                    &pool,
                    run_id,
                    job_id,
                    instance_id,
                    work_lease.epoch,
                    "artifact_db_finalize_failed",
                )
                .await;
                return;
            }
        }
        Err(_) => {
            let _ = quarantine_artifact(&pool, artifact_id).await;
            let _ = std::fs::remove_file(&file_path);
            let _ = mark_failed(
                &pool,
                run_id,
                job_id,
                instance_id,
                work_lease.epoch,
                "artifact_verify_failed",
            )
            .await;
            return;
        }
    }
    if crate::require_pipeline_oci_from_env()
        && run_kind == KIND_RELEASE
        && oci_from_adapter.is_none()
    {
        let _ = mark_failed(
            &pool,
            run_id,
            job_id,
            instance_id,
            work_lease.epoch,
            "pipeline_oci_missing",
        )
        .await;
        return;
    }
    if let Some(oci_ref) = oci_from_adapter {
        let oci_path = match safe_join(&dir, artifacts::ARTIFACT_OCI_REF_NAME) {
            Ok(p) => p,
            Err(_) => {
                let _ = mark_failed(
                    &pool,
                    run_id,
                    job_id,
                    instance_id,
                    work_lease.epoch,
                    "oci_artifact_path_failed",
                )
                .await;
                return;
            }
        };
        if !oci_path.is_file() {
            let _ = std::fs::write(&oci_path, &oci_ref);
        }
        if crate::security::validate_oci_image_ref(&oci_ref).is_err() {
            let _ = mark_failed(
                &pool,
                run_id,
                job_id,
                instance_id,
                work_lease.epoch,
                "pipeline_oci_invalid",
            )
            .await;
            return;
        }
        let oci_id = Uuid::new_v4();
        let (sha, len) = match artifacts::sha256_file(&oci_path) {
            Ok(v) => v,
            Err(_) => {
                let _ = mark_failed(
                    &pool,
                    run_id,
                    job_id,
                    instance_id,
                    work_lease.epoch,
                    "oci_artifact_verify_failed",
                )
                .await;
                return;
            }
        };
        let _ = sqlx::query(
            "INSERT INTO artifacts (id, org_id, repo_id, pipeline_run_id, name, kind, sha256, byte_length,
             storage_path, state, oci_image_ref)
             VALUES ($1,$2,$3,$4,$5,'oci_image',$6,$7,$8,'ready',$9)",
        )
        .bind(oci_id)
        .bind(org_id)
        .bind(repo_id)
        .bind(run_id)
        .bind(artifacts::ARTIFACT_OCI_REF_NAME)
        .bind(&sha)
        .bind(len as i64)
        .bind(dir.to_string_lossy().as_ref())
        .bind(&oci_ref)
        .execute(&pool)
        .await;
    }
    let job_ok = crate::lease::finalize_pipeline_job_state(
        &pool,
        job_id,
        run_id,
        instance_id,
        work_lease.epoch,
        "succeeded",
    )
    .await
    .unwrap_or(false);
    if job_ok {
        if let Ok((attempt_id,)) =
            sqlx::query_as("SELECT run_attempt_id FROM pipeline_runs WHERE id = $1")
                .bind(run_id)
                .fetch_one(&pool)
                .await
        {
            let _ = crate::mcp::contract_domain::append_pipeline_job_log(
                &pool,
                org_id,
                repo_id,
                job_id,
                attempt_id,
                "pipeline job completed successfully",
            )
            .await;
        }
    }
    let run_ok = crate::lease::finalize_pipeline_run_state(
        &pool,
        run_id,
        instance_id,
        work_lease.epoch,
        "succeeded",
        None,
    )
    .await
    .unwrap_or(false);
    if !job_ok || !run_ok {
        let _ = quarantine_artifact(&pool, artifact_id).await;
        let _ = std::fs::remove_file(&file_path);
    }
    supervisor.finish(run_id).await;
}

#[allow(clippy::too_many_arguments)]
async fn execute_pr_validation_job(
    pool: &PgPool,
    git_root: &std::path::Path,
    artifact_root: &std::path::Path,
    pipeline_bin: &std::path::Path,
    supervisor: &PipelineSupervisor,
    storage_key: Uuid,
    instance_id: Uuid,
    run_id: Uuid,
    job_id: Uuid,
    org_id: Uuid,
    repo_id: Uuid,
    source_oid: &str,
    lease_epoch: i64,
) {
    let snapshot = sqlx::query_as::<_, (String, String, String, serde_json::Value)>(
        "SELECT s.source_oid, s.workflow_path, s.content_hash, s.definition
         FROM pipeline_workflow_snapshots s
         WHERE s.pipeline_run_id = $1",
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await;
    let Ok(Some((snapshot_oid, workflow_path, content_hash, definition))) = snapshot else {
        let _ = mark_failed(
            pool,
            run_id,
            job_id,
            instance_id,
            lease_epoch,
            "workflow_snapshot_missing",
        )
        .await;
        return;
    };
    if snapshot_oid != source_oid
        || workflow_path != PR_WORKFLOW_PATH
        || definition.get("sha256").and_then(|value| value.as_str()) != Some(content_hash.as_str())
    {
        let _ = mark_failed(
            pool,
            run_id,
            job_id,
            instance_id,
            lease_epoch,
            "workflow_snapshot_mismatch",
        )
        .await;
        return;
    }
    let configured_bin = std::env::var_os("MUSEHUB_PIPELINE_JOB_ADAPTER_BIN")
        .map(PathBuf::from)
        .or_else(|| {
            (pipeline_bin.file_name().and_then(|name| name.to_str())
                == Some("musehub-pipeline-job"))
            .then(|| pipeline_bin.to_path_buf())
        });
    let Some(executor_bin) = configured_bin.filter(|path| path.is_absolute()) else {
        let _ = mark_failed(
            pool,
            run_id,
            job_id,
            instance_id,
            lease_epoch,
            "pipeline_job_executor_unavailable",
        )
        .await;
        return;
    };
    let workflow_json = match serde_json::to_string(&definition) {
        Ok(json) => json,
        Err(_) => {
            let _ = mark_failed(
                pool,
                run_id,
                job_id,
                instance_id,
                lease_epoch,
                "workflow_snapshot_invalid",
            )
            .await;
            return;
        }
    };
    let selected_attempts = sqlx::query_as::<_, (String, Uuid)>(
        "SELECT j.job_key, a.id
         FROM pipeline_workflow_jobs j
         JOIN LATERAL (
           SELECT id FROM pipeline_workflow_job_attempts
           WHERE workflow_job_id = j.id AND state = 'queued'
           ORDER BY attempt_number DESC LIMIT 1
         ) a ON TRUE
         WHERE j.pipeline_run_id = $1 AND j.state = 'queued'
         ORDER BY j.job_key",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await;
    let Ok(selected_attempts) = selected_attempts else {
        let _ = mark_failed(
            pool,
            run_id,
            job_id,
            instance_id,
            lease_epoch,
            "workflow_job_state_unavailable",
        )
        .await;
        return;
    };
    let succeeded_keys = sqlx::query_as::<_, (String,)>(
        "SELECT job_key FROM pipeline_workflow_jobs
         WHERE pipeline_run_id = $1 AND state = 'succeeded' ORDER BY job_key",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await;
    let Ok(succeeded_keys) = succeeded_keys else {
        let _ = mark_failed(
            pool,
            run_id,
            job_id,
            instance_id,
            lease_epoch,
            "workflow_job_state_unavailable",
        )
        .await;
        return;
    };
    let selected_json = serde_json::to_string(
        &selected_attempts
            .iter()
            .map(|row| &row.0)
            .collect::<Vec<_>>(),
    )
    .unwrap_or_default();
    let attempt_ids_json = serde_json::to_string(
        &selected_attempts
            .iter()
            .cloned()
            .collect::<std::collections::BTreeMap<_, _>>(),
    )
    .unwrap_or_default();
    let succeeded_json =
        serde_json::to_string(&succeeded_keys.iter().map(|row| &row.0).collect::<Vec<_>>())
            .unwrap_or_default();
    let bare = bare_repo_path(git_root, &storage_key);

    let mut cmd = Command::new(&executor_bin);
    cmd.env_clear()
        .env("MUSEHUB_PIPELINE_WORKFLOW_JSON", workflow_json)
        .env("MUSEHUB_PIPELINE_WORKFLOW_SELECTED_KEYS", selected_json)
        .env("MUSEHUB_PIPELINE_WORKFLOW_SUCCEEDED_KEYS", succeeded_json)
        .env("MUSEHUB_PIPELINE_WORKFLOW_ATTEMPT_IDS", attempt_ids_json)
        .env("MUSEHUB_PIPELINE_SOURCE_OID", source_oid)
        .env("MUSEHUB_PIPELINE_RUN_ID", run_id.to_string())
        .env("MUSEHUB_PIPELINE_ORG_ID", org_id.to_string())
        .env("MUSEHUB_PIPELINE_REPO_ID", repo_id.to_string())
        // Workflow executions reached here are PR validations; the workflow cannot choose this boundary.
        .env("MUSEHUB_PIPELINE_TRUST_CLASS", "untrusted")
        .env(
            "MUSEHUB_ARTIFACT_ROOT",
            artifact_root.to_string_lossy().as_ref(),
        )
        .env("MUSEHUB_RUNNER_INSTANCE_ID", instance_id.to_string())
        .env("MUSEHUB_PIPELINE_BARE_GIT", bare.to_string_lossy().as_ref());
    for name in [
        "MUSEHUB_JOB_DOCKER_BIN",
        "DOCKER_HOST",
        "DOCKER_TLS_VERIFY",
        "DOCKER_CERT_PATH",
        "MUSEHUB_JOB_WORKSPACE_ROOT",
        "MUSEHUB_JOB_WORKSPACE_HOST_ROOT",
        "MUSEHUB_JOB_ALLOWED_IMAGES",
    ] {
        if let Ok(value) = std::env::var(name) {
            cmd.env(name, value);
        }
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_child_process_group(&mut cmd);
    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(_) => {
            let _ = mark_failed(
                pool,
                run_id,
                job_id,
                instance_id,
                lease_epoch,
                "pipeline_job_executor_spawn_failed",
            )
            .await;
            return;
        }
    };
    if let Some(pid) = child.id() {
        supervisor.register(run_id, pid).await;
    }
    let output = match wait_pipeline_child(child, pool, run_id).await {
        Ok(output) => output,
        Err(ChildWaitOutcome::Canceled) => {
            let _ = supervisor.cancel(run_id).await;
            cleanup_pipeline_job_containers(&executor_bin, instance_id, run_id).await;
            let _ = mark_workflow_jobs_terminal(pool, run_id, "canceled", "run_canceled").await;
            let _ = mark_canceled(pool, run_id, job_id, instance_id, lease_epoch).await;
            return;
        }
        Err(ChildWaitOutcome::TimedOut) => {
            let _ = supervisor.cancel(run_id).await;
            cleanup_pipeline_job_containers(&executor_bin, instance_id, run_id).await;
            let _ = mark_workflow_jobs_terminal(pool, run_id, "failed", "pipeline_timeout").await;
            let _ = mark_failed(
                pool,
                run_id,
                job_id,
                instance_id,
                lease_epoch,
                "pipeline_timeout",
            )
            .await;
            return;
        }
        Err(ChildWaitOutcome::IoFailed) => {
            cleanup_pipeline_job_containers(&executor_bin, instance_id, run_id).await;
            let _ = mark_workflow_jobs_terminal(
                pool,
                run_id,
                "failed",
                "pipeline_job_executor_io_failed",
            )
            .await;
            let _ = mark_failed(
                pool,
                run_id,
                job_id,
                instance_id,
                lease_epoch,
                "pipeline_job_executor_io_failed",
            )
            .await;
            return;
        }
    };
    if let Err(error) = persist_workflow_job_results(pool, run_id, &output.stdout).await {
        tracing::warn!(run_id = %run_id, %error, "failed to persist workflow job results");
    }
    if let Err(error) = persist_pr_job_output(
        pool,
        org_id,
        repo_id,
        job_id,
        run_id,
        &output.stdout,
        &output.stderr,
    )
    .await
    {
        tracing::warn!(run_id = %run_id, %error, "failed to persist PR validation output");
    }
    if let Err(error) = persist_workflow_artifacts(
        pool,
        artifact_root,
        org_id,
        repo_id,
        run_id,
        &definition,
        &selected_attempts,
    )
    .await
    {
        tracing::warn!(run_id = %run_id, %error, "failed to persist workflow artifacts");
        let _ = mark_failed(
            pool,
            run_id,
            job_id,
            instance_id,
            lease_epoch,
            "workflow_artifact_invalid",
        )
        .await;
        return;
    }
    if !output.status.success() {
        let _ = mark_failed(
            pool,
            run_id,
            job_id,
            instance_id,
            lease_epoch,
            "workflow_job_failed",
        )
        .await;
        return;
    }
    let job_ok = crate::lease::finalize_pipeline_job_state(
        pool,
        job_id,
        run_id,
        instance_id,
        lease_epoch,
        "succeeded",
    )
    .await
    .unwrap_or(false);
    if !job_ok {
        return;
    }
    if crate::lease::finalize_pipeline_run_state(
        pool,
        run_id,
        instance_id,
        lease_epoch,
        "succeeded",
        None,
    )
    .await
    .unwrap_or(false)
    {
        let _ = finalize_pr_check_for_run(pool, run_id, "success").await;
    }
}

#[derive(Debug, Serialize)]
struct JunitSummary {
    tests: u64,
    failures: u64,
    errors: u64,
    skipped: u64,
}

async fn persist_workflow_artifacts(
    pool: &PgPool,
    artifact_root: &Path,
    org_id: Uuid,
    repo_id: Uuid,
    run_id: Uuid,
    snapshot: &serde_json::Value,
    selected_attempts: &[(String, Uuid)],
) -> Result<(), AppError> {
    let workflow = serde_json::from_value::<ValidatedWorkflow>(snapshot.clone())
        .map_err(|error| AppError::Validation(format!("workflow snapshot is invalid: {error}")))?;
    for (job_key, attempt_id) in selected_attempts {
        if !workflow_attempt_was_started(artifact_root, run_id, *attempt_id)? {
            continue;
        }
        let job = workflow
            .definition
            .jobs
            .iter()
            .find(|job| &job.id == job_key)
            .ok_or_else(|| {
                AppError::Validation("workflow job snapshot is incomplete".to_string())
            })?;
        let result = persist_workflow_attempt_artifacts(
            pool,
            artifact_root,
            org_id,
            repo_id,
            run_id,
            *attempt_id,
            job,
        )
        .await;
        cleanup_workflow_pending_attempt(artifact_root, run_id, *attempt_id);
        result?;
    }
    Ok(())
}

async fn persist_workflow_attempt_artifacts(
    pool: &PgPool,
    artifact_root: &Path,
    org_id: Uuid,
    repo_id: Uuid,
    run_id: Uuid,
    attempt_id: Uuid,
    job: &crate::pipeline_workflow::WorkflowJob,
) -> Result<(), AppError> {
    let mut artifacts = job
        .artifacts
        .iter()
        .map(|artifact| (artifact.name.as_str(), "file", artifact.name.as_str()))
        .collect::<Vec<_>>();
    if job.reports.is_some() {
        artifacts.push(("junit-report.xml", "junit", "reports/junit.xml"));
    }
    for (name, kind, relative_path) in artifacts {
        let staged = workflow_staged_file(artifact_root, run_id, attempt_id, relative_path)?;
        let bytes = read_bounded_regular_file(&staged)?;
        let summary = if kind == "junit" {
            Some(
                serde_json::to_value(parse_junit_report(&bytes)?).map_err(|error| {
                    AppError::Other(format!("failed to encode JUnit summary: {error}"))
                })?,
            )
        } else {
            None
        };
        let digest = Sha256::digest(&bytes);
        let sha256 = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let byte_length = i64::try_from(bytes.len())
            .map_err(|_| AppError::Validation("workflow artifact size is invalid".to_string()))?;
        let artifact_id = Uuid::new_v4();
        let directory = artifact_storage_dir(artifact_root, org_id, repo_id, artifact_id);
        std::fs::create_dir_all(&directory).map_err(|error| {
            AppError::Other(format!("create workflow artifact directory: {error}"))
        })?;
        let destination = safe_join(&directory, name)?;
        let temporary = directory.join(format!(".{artifact_id}.uploading"));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| {
                AppError::Other(format!("create workflow artifact staging file: {error}"))
            })?;
        use std::io::Write;
        if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
            let _ = std::fs::remove_file(&temporary);
            return Err(AppError::Other(format!("write workflow artifact: {error}")));
        }
        drop(file);
        if let Err(error) = std::fs::rename(&temporary, &destination) {
            let _ = std::fs::remove_file(&temporary);
            return Err(AppError::Other(format!(
                "publish workflow artifact: {error}"
            )));
        }

        let db_result = async {
            let mut tx = pool.begin().await?;
            sqlx::query(
                "INSERT INTO artifacts
                 (id, org_id, repo_id, pipeline_run_id, workflow_attempt_id, name, kind,
                  sha256, byte_length, storage_path, state)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'ready')",
            )
            .bind(artifact_id)
            .bind(org_id)
            .bind(repo_id)
            .bind(run_id)
            .bind(attempt_id)
            .bind(name)
            .bind(kind)
            .bind(&sha256)
            .bind(byte_length)
            .bind(directory.to_string_lossy().as_ref())
            .execute(&mut *tx)
            .await?;
            if let Some(summary) = summary {
                sqlx::query(
                    "UPDATE pipeline_workflow_job_attempts
                     SET result = jsonb_set(result, '{junit}', $2, true),
                         updated_at = CURRENT_TIMESTAMP
                     WHERE id = $1",
                )
                .bind(attempt_id)
                .bind(summary)
                .execute(&mut *tx)
                .await?;
            }
            tx.commit().await?;
            Ok::<(), AppError>(())
        }
        .await;
        if let Err(error) = db_result {
            let _ = std::fs::remove_file(&destination);
            let _ = std::fs::remove_dir(&directory);
            return Err(error);
        }
    }
    Ok(())
}

fn workflow_attempt_was_started(
    artifact_root: &Path,
    run_id: Uuid,
    attempt_id: Uuid,
) -> Result<bool, AppError> {
    let directory = artifact_root
        .join(".workflow-pending")
        .join(run_id.to_string())
        .join(attempt_id.to_string());
    match std::fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(AppError::Other(format!(
                "inspect workflow attempt output: {error}"
            )))
        }
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(AppError::Validation(
                "workflow attempt output path is unsafe".to_string(),
            ))
        }
    }
    let marker = workflow_staged_file(artifact_root, run_id, attempt_id, ".executed")?;
    let bytes = read_bounded_regular_file(&marker)?;
    if bytes != b"started\n" {
        return Err(AppError::Validation(
            "workflow attempt output marker is invalid".to_string(),
        ));
    }
    Ok(true)
}

fn workflow_staged_file(
    artifact_root: &Path,
    run_id: Uuid,
    attempt_id: Uuid,
    relative: &str,
) -> Result<PathBuf, AppError> {
    if !valid_workflow_output_path(relative) {
        return Err(AppError::Validation(
            "workflow artifact path is invalid".to_string(),
        ));
    }
    let mut components = vec![
        ".workflow-pending".to_string(),
        run_id.to_string(),
        attempt_id.to_string(),
    ];
    components.extend(relative.split('/').map(str::to_string));
    let mut path = artifact_root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        path.push(component);
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| {
            AppError::Validation("declared workflow artifact is missing".to_string())
        })?;
        let is_final = index + 1 == components.len();
        if metadata.file_type().is_symlink()
            || (is_final && !metadata.is_file())
            || (!is_final && !metadata.is_dir())
        {
            return Err(AppError::Validation(
                "workflow artifact path is unsafe".to_string(),
            ));
        }
    }
    Ok(path)
}

fn valid_workflow_output_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && !value.starts_with('/')
        && !value.contains('\\')
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".." && part != ".git")
}

fn read_bounded_regular_file(path: &Path) -> Result<Vec<u8>, AppError> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| AppError::Validation("workflow artifact is missing".to_string()))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > artifacts::MAX_ARTIFACT_BYTES
    {
        return Err(AppError::Validation(
            "workflow artifact is too large or unsafe".to_string(),
        ));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|error| AppError::Other(format!("open workflow artifact: {error}")))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(artifacts::MAX_ARTIFACT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| AppError::Other(format!("read workflow artifact: {error}")))?;
    if bytes.len() as u64 > artifacts::MAX_ARTIFACT_BYTES {
        return Err(AppError::Validation(
            "workflow artifact exceeds size limit".to_string(),
        ));
    }
    Ok(bytes)
}

fn parse_junit_report(bytes: &[u8]) -> Result<JunitSummary, AppError> {
    use quick_xml::events::Event;
    if bytes.is_empty() || bytes.len() as u64 > artifacts::MAX_ARTIFACT_BYTES {
        return Err(AppError::Validation(
            "JUnit report is empty or too large".to_string(),
        ));
    }
    let mut reader = quick_xml::Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut stack = Vec::<String>::new();
    let mut summary = JunitSummary {
        tests: 0,
        failures: 0,
        errors: 0,
        skipped: 0,
    };
    let mut root = None::<String>;
    let mut root_closed = false;
    let mut events = 0usize;
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|_| AppError::Validation("JUnit report XML is invalid".to_string()))?;
        events += 1;
        if events > 1_000_000 {
            return Err(AppError::Validation(
                "JUnit report has too many XML nodes".to_string(),
            ));
        }
        match event {
            Event::Start(element) => {
                let name = element.name().as_ref().to_string();
                if stack.is_empty() {
                    if root.is_some() || !matches!(name.as_str(), "testsuite" | "testsuites") {
                        return Err(AppError::Validation(
                            "JUnit report root element is invalid".to_string(),
                        ));
                    }
                    root = Some(name.clone());
                }
                count_junit_element(&name, &stack, &mut summary);
                stack.push(name);
                if stack.len() > 128 {
                    return Err(AppError::Validation(
                        "JUnit report XML is too deeply nested".to_string(),
                    ));
                }
            }
            Event::Empty(element) => {
                let name = element.name().as_ref().to_string();
                if stack.is_empty() {
                    if root.is_some() || !matches!(name.as_str(), "testsuite" | "testsuites") {
                        return Err(AppError::Validation(
                            "JUnit report root element is invalid".to_string(),
                        ));
                    }
                    root = Some(name);
                    root_closed = true;
                } else {
                    count_junit_element(&name, &stack, &mut summary);
                }
            }
            Event::End(element) => {
                let name = element.name().as_ref().to_string();
                if stack.pop().as_deref() != Some(name.as_str()) {
                    return Err(AppError::Validation(
                        "JUnit report XML is unbalanced".to_string(),
                    ));
                }
                if stack.is_empty() {
                    root_closed = true;
                }
            }
            Event::DocType(_) => {
                return Err(AppError::Validation(
                    "JUnit report must not declare a DTD".to_string(),
                ));
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !stack.is_empty() || root.is_none() || !root_closed {
        return Err(AppError::Validation(
            "JUnit report XML is incomplete".to_string(),
        ));
    }
    Ok(summary)
}

fn count_junit_element(name: &str, stack: &[String], summary: &mut JunitSummary) {
    if name == "testcase" {
        summary.tests = summary.tests.saturating_add(1);
    } else if stack.iter().any(|element| element == "testcase") {
        match name {
            "failure" => summary.failures = summary.failures.saturating_add(1),
            "error" => summary.errors = summary.errors.saturating_add(1),
            "skipped" => summary.skipped = summary.skipped.saturating_add(1),
            _ => {}
        }
    }
}

fn cleanup_workflow_pending_attempt(artifact_root: &Path, run_id: Uuid, attempt_id: Uuid) {
    let pending = artifact_root
        .join(".workflow-pending")
        .join(run_id.to_string())
        .join(attempt_id.to_string());
    if std::fs::symlink_metadata(&pending)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
    {
        let _ = std::fs::remove_dir_all(pending);
    }
}

async fn persist_pr_job_output(
    pool: &PgPool,
    org_id: Uuid,
    repo_id: Uuid,
    job_id: Uuid,
    run_id: Uuid,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<(), AppError> {
    let (attempt_id,): (Uuid,) =
        sqlx::query_as("SELECT run_attempt_id FROM pipeline_runs WHERE id = $1")
            .bind(run_id)
            .fetch_one(pool)
            .await?;
    let mut bytes = Vec::with_capacity(
        stdout
            .len()
            .saturating_add(stderr.len())
            .min(MAX_PIPELINE_LOG_BYTES),
    );
    for part in [stdout, b"\n--- stderr ---\n", stderr] {
        let remaining = MAX_PIPELINE_LOG_BYTES.saturating_sub(bytes.len());
        bytes.extend_from_slice(&part[..part.len().min(remaining)]);
    }
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if stdout.len().saturating_add(stderr.len()) > bytes.len() {
        text.push_str("\n[stored log truncated at 1 MiB]");
    }
    crate::mcp::contract_domain::append_pipeline_job_log(
        pool, org_id, repo_id, job_id, attempt_id, &text,
    )
    .await
}

async fn finalize_pr_check_for_run(
    pool: &PgPool,
    run_id: Uuid,
    conclusion: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE pr_check_runs c SET state = $2
         FROM pipeline_runs r
         JOIN pull_requests p ON p.id = r.pr_id
         WHERE r.id = $1 AND r.pr_id IS NOT NULL
           AND c.pr_id = p.id AND c.head_oid = r.source_oid
           AND p.head_oid = r.source_oid AND c.state = 'pending'",
    )
    .bind(run_id)
    .bind(conclusion)
    .execute(pool)
    .await?;
    Ok(())
}

fn truncate_log_text(value: &str, max_bytes: usize) -> &str {
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

async fn persist_workflow_job_logs(
    pool: &PgPool,
    run_id: Uuid,
    stdout: &[u8],
) -> Result<(), AppError> {
    let prefix = format!("MUSEHUB_WORKFLOW_LOG:{run_id}:");
    for line in stdout.split(|byte| *byte == b'\n') {
        let Ok(line) = std::str::from_utf8(line) else {
            continue;
        };
        let Some(payload) = line.strip_prefix(&prefix) else {
            continue;
        };
        let Ok(event) = serde_json::from_str::<serde_json::Value>(payload) else {
            continue;
        };
        let Some(job_key) = event.get("job_key").and_then(|value| value.as_str()) else {
            continue;
        };
        let (Some(job_stdout), Some(job_stderr)) = (
            event.get("stdout").and_then(|value| value.as_str()),
            event.get("stderr").and_then(|value| value.as_str()),
        ) else {
            continue;
        };
        let row = sqlx::query_as::<_, (Uuid, Uuid, Uuid, Uuid)>(
            "SELECT j.id, r.org_id, r.repo_id, a.id
             FROM pipeline_workflow_jobs j
             JOIN pipeline_runs r ON r.id = j.pipeline_run_id
             JOIN LATERAL (
               SELECT id FROM pipeline_workflow_job_attempts
               WHERE workflow_job_id = j.id AND state IN ('queued','running')
               ORDER BY attempt_number DESC LIMIT 1
             ) a ON TRUE
             WHERE j.pipeline_run_id = $1 AND j.job_key = $2",
        )
        .bind(run_id)
        .bind(job_key)
        .fetch_optional(pool)
        .await?;
        let Some((job_id, org_id, repo_id, attempt_id)) = row else {
            continue;
        };
        let mut text = String::with_capacity(
            job_stdout
                .len()
                .saturating_add(job_stderr.len())
                .min(MAX_PIPELINE_LOG_BYTES),
        );
        text.push_str("--- stdout ---\n");
        text.push_str(truncate_log_text(job_stdout, MAX_PIPELINE_LOG_BYTES / 2));
        text.push_str("\n--- stderr ---\n");
        let remaining = MAX_PIPELINE_LOG_BYTES.saturating_sub(text.len());
        text.push_str(truncate_log_text(job_stderr, remaining));
        if job_stdout.len().saturating_add(job_stderr.len()) > MAX_PIPELINE_LOG_BYTES {
            text.push_str("\n[workflow job log truncated at 1 MiB]");
        }
        crate::mcp::contract_domain::append_pipeline_workflow_job_log(
            pool, org_id, repo_id, job_id, attempt_id, &text,
        )
        .await?;
    }
    Ok(())
}

async fn persist_workflow_job_results(
    pool: &PgPool,
    run_id: Uuid,
    stdout: &[u8],
) -> Result<(), AppError> {
    if let Err(error) = persist_workflow_job_logs(pool, run_id, stdout).await {
        tracing::warn!(run_id = %run_id, %error, "failed to persist workflow job logs");
    }
    let prefix = format!("MUSEHUB_WORKFLOW_RESULT:{run_id}:");
    let mut reported = std::collections::BTreeSet::new();
    for line in stdout.split(|byte| *byte == b'\n') {
        let Ok(line) = std::str::from_utf8(line) else {
            continue;
        };
        let Some(result) = line.strip_prefix(&prefix) else {
            continue;
        };
        let Some((job_key, state)) = result.rsplit_once(':') else {
            continue;
        };
        if !matches!(state, "succeeded" | "failed") || job_key.is_empty() {
            continue;
        }
        if !reported.insert(job_key.to_string()) {
            continue;
        }
        let error_code = (state == "failed").then_some("workflow_job_failed");
        sqlx::query(
            "UPDATE pipeline_workflow_jobs
             SET state = $3, attempt_count = GREATEST(attempt_count, 1),
                 error_code = $4,
                 started_at = COALESCE(started_at, created_at),
                 completed_at = CURRENT_TIMESTAMP
             WHERE pipeline_run_id = $1 AND job_key = $2 AND state IN ('queued','running')",
        )
        .bind(run_id)
        .bind(job_key)
        .bind(state)
        .bind(error_code)
        .execute(pool)
        .await?;
        sqlx::query(
            "UPDATE pipeline_workflow_job_attempts a
             SET state = $3, error_code = $4,
                 started_at = COALESCE(a.started_at, a.created_at),
                 completed_at = CURRENT_TIMESTAMP
             FROM pipeline_workflow_jobs j
             WHERE a.workflow_job_id = j.id AND j.pipeline_run_id = $1
               AND j.job_key = $2 AND a.state = 'queued'
               AND a.attempt_number = (
                 SELECT MAX(current.attempt_number) FROM pipeline_workflow_job_attempts current
                 WHERE current.workflow_job_id = j.id
               )",
        )
        .bind(run_id)
        .bind(job_key)
        .bind(state)
        .bind(error_code)
        .execute(pool)
        .await?;
    }
    sqlx::query(
        "UPDATE pipeline_workflow_jobs
         SET state = 'skipped', error_code = $2, completed_at = CURRENT_TIMESTAMP
         WHERE pipeline_run_id = $1 AND state IN ('queued','running')",
    )
    .bind(run_id)
    .bind("dependency_failed")
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE pipeline_workflow_job_attempts a
         SET state = 'canceled', error_code = 'dependency_failed', completed_at = CURRENT_TIMESTAMP
         FROM pipeline_workflow_jobs j
         WHERE a.workflow_job_id = j.id AND j.pipeline_run_id = $1
           AND a.state = 'queued' AND j.state = 'skipped'",
    )
    .bind(run_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn mark_workflow_jobs_terminal(
    pool: &PgPool,
    run_id: Uuid,
    state: &str,
    error_code: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE pipeline_workflow_jobs
         SET state = $2, error_code = $3,
             attempt_count = CASE WHEN state = 'running' THEN GREATEST(attempt_count, 1) ELSE attempt_count END,
             started_at = CASE WHEN state = 'running' THEN COALESCE(started_at, created_at) ELSE started_at END,
             completed_at = CURRENT_TIMESTAMP
         WHERE pipeline_run_id = $1 AND state IN ('queued','running')",
    )
    .bind(run_id)
    .bind(state)
    .bind(error_code)
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE pipeline_workflow_job_attempts a
         SET state = $2, error_code = $3,
             started_at = COALESCE(a.started_at, a.created_at), completed_at = CURRENT_TIMESTAMP
         FROM pipeline_workflow_jobs j
         WHERE a.workflow_job_id = j.id AND j.pipeline_run_id = $1 AND a.state = 'queued'",
    )
    .bind(run_id)
    .bind(if state == "canceled" {
        "canceled"
    } else {
        "failed"
    })
    .bind(error_code)
    .execute(pool)
    .await?;
    Ok(())
}

async fn cleanup_pipeline_job_containers(
    executor_bin: &std::path::Path,
    instance_id: Uuid,
    run_id: Uuid,
) {
    let docker_bin = std::env::var_os("MUSEHUB_JOB_DOCKER_BIN");
    if let Some(docker_bin) = docker_bin {
        let mut command = Command::new(executor_bin);
        command
            .env_clear()
            .env("MUSEHUB_JOB_DOCKER_BIN", docker_bin)
            .env("MUSEHUB_RUNNER_INSTANCE_ID", instance_id.to_string());
        for name in ["DOCKER_HOST", "DOCKER_TLS_VERIFY", "DOCKER_CERT_PATH"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let result = command
            .arg("--cancel")
            .arg(run_id.to_string())
            .output()
            .await;
        if !matches!(result, Ok(output) if output.status.success()) {
            tracing::error!(run_id = %run_id, "failed to stop PR workflow containers");
        }
    }
    if let Some(root) = std::env::var_os("MUSEHUB_JOB_WORKSPACE_ROOT") {
        let run_workspace = PathBuf::from(root).join(run_id.to_string());
        if run_workspace.is_absolute()
            && !run_workspace
                .components()
                .any(|part| part == std::path::Component::ParentDir)
        {
            if let Err(error) = tokio::fs::remove_dir_all(run_workspace).await {
                if error.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(run_id = %run_id, %error, "failed to clean PR workflow workspace");
                }
            }
        }
    }
}

enum ChildWaitOutcome {
    Canceled,
    TimedOut,
    IoFailed,
}

async fn wait_pipeline_child(
    child: tokio::process::Child,
    pool: &PgPool,
    run_id: Uuid,
) -> Result<std::process::Output, ChildWaitOutcome> {
    let deadline = Duration::from_secs(300);
    let cancel_watch = async {
        loop {
            tokio::time::sleep(Duration::from_millis(400)).await;
            if cancel_flag(pool, run_id).await {
                break;
            }
        }
    };
    let waited = tokio::time::timeout(deadline, async {
        tokio::select! {
            out = child.wait_with_output() => Either::Left(out),
            () = cancel_watch => Either::Right(()),
        }
    })
    .await;
    match waited {
        Ok(Either::Left(Ok(output))) => Ok(output),
        Ok(Either::Left(Err(_))) => Err(ChildWaitOutcome::IoFailed),
        Ok(Either::Right(())) => Err(ChildWaitOutcome::Canceled),
        Err(_) => Err(ChildWaitOutcome::TimedOut),
    }
}

enum Either<L, R> {
    Left(L),
    Right(R),
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

async fn mark_failed(
    pool: &PgPool,
    run_id: Uuid,
    job_id: Uuid,
    instance_id: Uuid,
    epoch: i64,
    code: &str,
) -> bool {
    let _ = crate::lease::finalize_pipeline_job_state(
        pool,
        job_id,
        run_id,
        instance_id,
        epoch,
        "failed",
    )
    .await;
    let finalized = crate::lease::finalize_pipeline_run_state(
        pool,
        run_id,
        instance_id,
        epoch,
        "failed",
        Some(code),
    )
    .await
    .unwrap_or(false);
    if finalized {
        let _ = finalize_pr_check_for_run(pool, run_id, "failure").await;
    }
    finalized
}

async fn mark_succeeded_stale_blocked(
    pool: &PgPool,
    run_id: Uuid,
    job_id: Uuid,
    instance_id: Uuid,
    epoch: i64,
) -> bool {
    let run_ok = crate::lease::finalize_pipeline_run_state(
        pool,
        run_id,
        instance_id,
        epoch,
        "succeeded",
        None,
    )
    .await
    .unwrap_or(false);
    if !run_ok {
        return false;
    }
    crate::lease::finalize_pipeline_job_state(pool, job_id, run_id, instance_id, epoch, "succeeded")
        .await
        .unwrap_or(false)
}

fn parse_pipeline_stdout(stdout: &[u8]) -> Option<String> {
    for line in stdout.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) == Some("oci_image") {
            return v
                .get("oci_image_ref")
                .and_then(|x| x.as_str())
                .map(String::from);
        }
    }
    None
}

async fn mark_canceled(
    pool: &PgPool,
    run_id: Uuid,
    job_id: Uuid,
    instance_id: Uuid,
    epoch: i64,
) -> bool {
    let _ = crate::lease::finalize_pipeline_job_state(
        pool,
        job_id,
        run_id,
        instance_id,
        epoch,
        "canceled",
    )
    .await;
    let finalized = crate::lease::finalize_pipeline_run_state(
        pool,
        run_id,
        instance_id,
        epoch,
        "canceled",
        None,
    )
    .await
    .unwrap_or(false);
    if finalized {
        let _ = finalize_pr_check_for_run(pool, run_id, "failure").await;
    }
    finalized
}

pub async fn try_publish_stale_pipeline_success_for_test(
    pool: &PgPool,
    run_id: Uuid,
    job_id: Uuid,
    instance_id: Uuid,
    epoch: i64,
) -> bool {
    mark_succeeded_stale_blocked(pool, run_id, job_id, instance_id, epoch).await
}

#[cfg(test)]
mod junit_tests {
    use super::parse_junit_report;

    #[test]
    fn parses_junit_counts_and_rejects_unsafe_or_invalid_xml() {
        let report = br#"<testsuites><testsuite><testcase/><testcase><failure/></testcase><testcase><error/><skipped/></testcase></testsuite></testsuites>"#;
        let summary = parse_junit_report(report).expect("valid JUnit report");
        assert_eq!(
            (
                summary.tests,
                summary.failures,
                summary.errors,
                summary.skipped
            ),
            (3, 1, 1, 1)
        );

        assert!(parse_junit_report(b"<!DOCTYPE testsuite><testsuite/>").is_err());
        assert!(parse_junit_report(b"<unexpected/>").is_err());
        assert!(parse_junit_report(b"<testsuite>").is_err());
    }
}
