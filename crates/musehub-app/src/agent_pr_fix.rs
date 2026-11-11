//! PR CI failure → bounded agent fix round (`docs/01-product-requirements.md` §2 S3).

use std::path::Path;
use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgQueryResult;
use uuid::Uuid;

use crate::agent_adapter;
use crate::agent_supervisor;
use crate::error::AppError;
use crate::git_http::bare_repo_path;
use crate::pipeline::PipelineJobView;
use crate::pr::{self, DEFAULT_REPO_CHECK};
use crate::runner_api;
use crate::security::{validate_git_oid, validate_pr_agent_fix_work_branch};
use crate::AppState;

/// Maximum fix rounds per PR (`§2 S3` — not unbounded self-heal). Override for tests only.
pub fn agent_fix_max_rounds() -> i64 {
    std::env::var("MUSEHUB_AGENT_FIX_MAX_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n| (1..=32).contains(&n))
        .unwrap_or(3)
}

#[derive(Serialize, Clone)]
pub struct PrCheckFailureView {
    pub check_name: String,
    pub state: String,
}

#[derive(Serialize)]
pub struct PrCiFailureView {
    pub pr_id: Uuid,
    pub pr_number: i64,
    pub head_oid: String,
    pub issue_id: Option<Uuid>,
    pub checks_failed: Vec<PrCheckFailureView>,
    pub pipeline_run_id: Option<Uuid>,
    pub pipeline_state: Option<String>,
    pub pipeline_error_code: Option<String>,
    pub failed_jobs: Vec<PipelineJobView>,
    pub fix_rounds_used: i64,
    pub fix_rounds_max: i64,
    pub agent_fix_allowed: bool,
    pub agent_fix_denial_reason: Option<String>,
}

#[derive(Deserialize)]
pub struct StartPrAgentFixRequest {
    pub budget_tokens: Option<u64>,
}

#[derive(Serialize)]
pub struct PrAgentFixRunView {
    pub id: Uuid,
    pub adapter_kind: String,
    pub state: String,
    pub pr_id: Uuid,
    pub fix_round: i32,
    pub head_oid: String,
    pub usage_tokens: u64,
    pub budget_unknown: bool,
}

#[derive(Clone)]
struct OpenPrRow {
    pr_id: Uuid,
    pr_number: i64,
    source_branch: String,
    head_oid: String,
    issue_id: Option<Uuid>,
}

async fn load_open_pr(
    pool: &sqlx::PgPool,
    repo_id: Uuid,
    number: i64,
) -> Result<OpenPrRow, AppError> {
    let row = sqlx::query_as::<_, (Uuid, i64, String, String, Option<String>, Option<Uuid>)>(
        "SELECT id, number, source_branch, state, head_oid, issue_id
         FROM pull_requests WHERE repo_id = $1 AND number = $2",
    )
    .bind(repo_id)
    .bind(number)
    .fetch_optional(pool)
    .await?;
    let Some((pr_id, pr_number, source_branch, state, head_oid, issue_id)) = row else {
        return Err(AppError::NotFound);
    };
    if state != "open" {
        return Err(AppError::Validation(
            "agent fix requires an open pull request".to_string(),
        ));
    }
    let Some(head) = head_oid else {
        return Err(AppError::Validation(
            "pull request head_oid is unknown; sync branch tip first".to_string(),
        ));
    };
    validate_git_oid(&head).map_err(AppError::Validation)?;
    validate_pr_agent_fix_work_branch(&source_branch).map_err(AppError::Validation)?;
    Ok(OpenPrRow {
        pr_id,
        pr_number,
        source_branch,
        head_oid: head,
        issue_id,
    })
}

fn verify_pr_head_at_branch(
    git_root: &Path,
    storage_key: Uuid,
    source_branch: &str,
    head_oid: &str,
) -> Result<(), String> {
    let bare = bare_repo_path(git_root, &storage_key);
    let live = pr::git_rev_parse_public(&bare, &format!("refs/heads/{source_branch}"))
        .map_err(|e| e.to_string())?;
    if live != head_oid {
        return Err("PR head is stale; sync branch tip before agent fix".to_string());
    }
    Ok(())
}

async fn build_pr_ci_failure_view(
    pool: &sqlx::PgPool,
    git_root: &Path,
    storage_key: Uuid,
    repo_id: Uuid,
    pr: OpenPrRow,
) -> Result<PrCiFailureView, AppError> {
    let pr_id = pr.pr_id;
    let head = pr.head_oid.clone();

    let fix_rounds_used_early: (i64,) =
        sqlx::query_as("SELECT COUNT(*)::bigint FROM agent_runs WHERE pr_id = $1")
            .bind(pr_id)
            .fetch_one(pool)
            .await?;

    if let Err(msg) = verify_pr_head_at_branch(git_root, storage_key, &pr.source_branch, &head) {
        return Ok(PrCiFailureView {
            pr_id,
            pr_number: pr.pr_number,
            head_oid: head,
            issue_id: pr.issue_id,
            checks_failed: vec![],
            pipeline_run_id: None,
            pipeline_state: None,
            pipeline_error_code: None,
            failed_jobs: vec![],
            fix_rounds_used: fix_rounds_used_early.0,
            fix_rounds_max: agent_fix_max_rounds(),
            agent_fix_allowed: false,
            agent_fix_denial_reason: Some(msg),
        });
    }

    let checks = sqlx::query_as::<_, (String, String)>(
        "SELECT check_name, state FROM pr_check_runs
         WHERE pr_id = $1 AND head_oid = $2 ORDER BY check_name",
    )
    .bind(pr_id)
    .bind(&head)
    .fetch_all(pool)
    .await?;
    let checks_failed: Vec<PrCheckFailureView> = checks
        .iter()
        .filter(|(_, st)| st == "failure")
        .map(|(name, st)| PrCheckFailureView {
            check_name: name.clone(),
            state: st.clone(),
        })
        .collect();

    let pipe = sqlx::query_as::<_, (Uuid, String, Option<String>)>(
        "SELECT id, state, error_code FROM pipeline_runs
         WHERE repo_id = $1 AND pr_id = $2 AND run_kind = 'pr_validation' AND source_oid = $3
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(repo_id)
    .bind(pr_id)
    .bind(&head)
    .fetch_optional(pool)
    .await?;

    let mut failed_jobs = Vec::new();
    let (pipeline_run_id, pipeline_state, pipeline_error_code) = if let Some((id, st, err)) = pipe {
        if st == "failed" {
            let jobs = sqlx::query_as::<_, (Uuid, Uuid, String, String)>(
                "SELECT id, pipeline_run_id, job_key, state FROM pipeline_jobs
                 WHERE pipeline_run_id = $1 AND repo_id = $2 ORDER BY job_key",
            )
            .bind(id)
            .bind(repo_id)
            .fetch_all(pool)
            .await?;
            failed_jobs = jobs
                .iter()
                .filter(|(_, _, _, js)| js == "failed")
                .map(|(jid, prid, jk, js)| PipelineJobView {
                    id: *jid,
                    pipeline_run_id: *prid,
                    job_key: jk.clone(),
                    state: js.clone(),
                    retry_allowed: false,
                })
                .collect();
        }
        (Some(id), Some(st), err)
    } else {
        (None, None, None)
    };

    let ci_failed = !checks_failed.is_empty() || pipeline_state.as_deref() == Some("failed");

    let fix_rounds_used: (i64,) =
        sqlx::query_as("SELECT COUNT(*)::bigint FROM agent_runs WHERE pr_id = $1")
            .bind(pr_id)
            .fetch_one(pool)
            .await?;
    let fix_rounds_max = agent_fix_max_rounds();

    let active: (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::bigint FROM agent_runs WHERE pr_id = $1
         AND state IN ('queued','running','cancel_requested','waiting_input')",
    )
    .bind(pr_id)
    .fetch_one(pool)
    .await?;

    let mut agent_fix_allowed = ci_failed;
    let mut denial: Option<String> = None;
    if !ci_failed {
        denial = Some("no CI failure recorded for current PR head".to_string());
        agent_fix_allowed = false;
    } else if pr.issue_id.is_none() {
        denial = Some("pull request has no linked issue".to_string());
        agent_fix_allowed = false;
    } else if active.0 > 0 {
        denial =
            Some("an agent fix round is already in progress for this pull request".to_string());
        agent_fix_allowed = false;
    } else if fix_rounds_used.0 >= fix_rounds_max {
        denial = Some(format!(
            "maximum agent fix rounds ({fix_rounds_max}) reached for this pull request"
        ));
        agent_fix_allowed = false;
    } else if let Some(issue_id) = pr.issue_id {
        let plan_ok = sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM implementation_plans p
             JOIN issues i ON i.id = $1 AND i.repo_id = $2
             WHERE p.state = 'accepted' AND p.issue_revision_id = i.current_revision_id",
        )
        .bind(issue_id)
        .bind(repo_id)
        .fetch_one(pool)
        .await?;
        if plan_ok.0 == 0 {
            denial = Some(
                "issue needs an accepted plan on the current revision before agent fix".to_string(),
            );
            agent_fix_allowed = false;
        }
    }

    Ok(PrCiFailureView {
        pr_id,
        pr_number: pr.pr_number,
        head_oid: head,
        issue_id: pr.issue_id,
        checks_failed,
        pipeline_run_id,
        pipeline_state,
        pipeline_error_code,
        failed_jobs,
        fix_rounds_used: fix_rounds_used.0,
        fix_rounds_max,
        agent_fix_allowed,
        agent_fix_denial_reason: denial,
    })
}

pub async fn get_pr_ci_failure(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, number)): AxumPath<(String, String, i64)>,
) -> Result<Json<PrCiFailureView>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (_, repo_id, storage_key) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let pr = load_open_pr(&state.pool, repo_id, number).await?;
    let view =
        build_pr_ci_failure_view(&state.pool, &state.git_root, storage_key, repo_id, pr).await?;
    Ok(Json(view))
}

#[allow(clippy::too_many_arguments)]
async fn insert_fix_run(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org_id: Uuid,
    repo_id: Uuid,
    issue_id: Uuid,
    principal_id: Uuid,
    adapter: &str,
    branch: &str,
    budget_tokens: Option<u64>,
    message: &str,
    pr_id: Uuid,
    head: &str,
    fix_round: i32,
    remote_agent: bool,
    instance_id: Uuid,
    run_id: Uuid,
) -> Result<PgQueryResult, sqlx::Error> {
    if remote_agent {
        sqlx::query(
            "INSERT INTO agent_runs (
                id, org_id, repo_id, issue_id, requested_by, adapter_kind, work_branch, state,
                budget_tokens_limit, task_message, pr_id, head_oid, fix_round
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,'queued',$8,$9,$10,$11,$12)",
        )
        .bind(run_id)
        .bind(org_id)
        .bind(repo_id)
        .bind(issue_id)
        .bind(principal_id)
        .bind(adapter)
        .bind(branch)
        .bind(budget_tokens.map(|limit| limit as i64))
        .bind(message)
        .bind(pr_id)
        .bind(head)
        .bind(fix_round)
        .execute(&mut **tx)
        .await
    } else {
        sqlx::query(
            "INSERT INTO agent_runs (
                id, org_id, repo_id, issue_id, requested_by, adapter_kind, work_branch, state,
                budget_tokens_limit, task_message, lease_holder, lease_until, pr_id, head_oid, fix_round
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,'queued',$8,$9,$10,
                CURRENT_TIMESTAMP + ($11::text || ' seconds')::interval,$12,$13,$14)",
        )
        .bind(run_id)
        .bind(org_id)
        .bind(repo_id)
        .bind(issue_id)
        .bind(principal_id)
        .bind(adapter)
        .bind(branch)
        .bind(budget_tokens.map(|limit| limit as i64))
        .bind(message)
        .bind(instance_id)
        .bind(crate::lease::lease_secs())
        .bind(pr_id)
        .bind(head)
        .bind(fix_round)
        .execute(&mut **tx)
        .await
    }
}

pub async fn start_pr_agent_fix(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, number)): AxumPath<(String, String, i64)>,
    Json(body): Json<StartPrAgentFixRequest>,
) -> Result<(StatusCode, Json<PrAgentFixRunView>), AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id, storage_key) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;

    let pr = load_open_pr(&state.pool, repo_id, number).await?;
    let ctx = build_pr_ci_failure_view(
        &state.pool,
        &state.git_root,
        storage_key,
        repo_id,
        pr.clone(),
    )
    .await?;
    if !ctx.agent_fix_allowed {
        return Err(AppError::Validation(
            ctx.agent_fix_denial_reason
                .unwrap_or_else(|| "agent fix not allowed".to_string()),
        ));
    }

    let mut tx = state.pool.begin().await?;
    let _ = sqlx::query("SELECT id FROM pull_requests WHERE id = $1 FOR UPDATE")
        .bind(pr.pr_id)
        .fetch_one(&mut *tx)
        .await?;

    let active: (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::bigint FROM agent_runs WHERE pr_id = $1
         AND state IN ('queued','running','cancel_requested','waiting_input')",
    )
    .bind(pr.pr_id)
    .fetch_one(&mut *tx)
    .await?;
    if active.0 > 0 {
        return Err(AppError::Validation(
            "an agent fix round is already in progress for this pull request".to_string(),
        ));
    }
    let issue_id = ctx
        .issue_id
        .ok_or_else(|| AppError::Validation("pull request has no linked issue".to_string()))?;

    let next_round: (i32,) =
        sqlx::query_as("SELECT COALESCE(MAX(fix_round), 0) + 1 FROM agent_runs WHERE pr_id = $1")
            .bind(pr.pr_id)
            .fetch_one(&mut *tx)
            .await?;
    let fix_round = next_round.0;
    if fix_round as i64 > agent_fix_max_rounds() {
        return Err(AppError::Validation(format!(
            "maximum agent fix rounds ({}) reached for this pull request",
            agent_fix_max_rounds()
        )));
    }

    let check_summary = if ctx.checks_failed.is_empty() {
        DEFAULT_REPO_CHECK.to_string()
    } else {
        ctx.checks_failed
            .iter()
            .map(|c| c.check_name.as_str())
            .collect::<Vec<_>>()
            .join(",")
    };
    let pipe_note = ctx
        .pipeline_error_code
        .as_deref()
        .unwrap_or(ctx.pipeline_state.as_deref().unwrap_or("none"));
    let job_note = if ctx.failed_jobs.is_empty() {
        "none".to_string()
    } else {
        ctx.failed_jobs
            .iter()
            .map(|j| format!("{}:{}", j.job_key, j.state))
            .collect::<Vec<_>>()
            .join(";")
    };
    let message = format!(
        "PR #{} CI fix round {fix_round} @ {}: failed_checks={check_summary}; pipeline={pipe_note}; jobs={job_note}",
        pr.pr_number, ctx.head_oid
    );

    let run_id = Uuid::new_v4();
    let branch = pr.source_branch.clone();
    let head = ctx.head_oid.clone();
    let head_for_run = head.clone();
    let remote_agent = runner_api::agent_executor_remote();
    let instance_id = state.instance_id;
    let adapter = agent_adapter::adapter_kind_for_request(None);

    insert_fix_run(
        &mut tx,
        org_id,
        repo_id,
        issue_id,
        principal_id,
        adapter,
        &branch,
        body.budget_tokens,
        &message,
        pr.pr_id,
        &head,
        fix_round,
        remote_agent,
        instance_id,
        run_id,
    )
    .await
    .map_err(|e| {
        if let sqlx::Error::Database(db) = &e {
            if db.constraint() == Some("uk_agent_runs_pr_fix_round")
                || db.constraint() == Some("uk_agent_runs_pr_inflight")
            {
                return AppError::Validation(
                    "agent fix round already in progress or duplicate round".to_string(),
                );
            }
        }
        AppError::Other(e.to_string())
    })?;

    tx.commit().await?;

    if !remote_agent {
        let supervisor = state.supervisor.clone();
        let pool = state.pool.clone();
        let git_root = state.git_root.clone();
        let reference_bin = state.reference_adapter_bin.clone();
        let budget_tokens = body.budget_tokens;
        tokio::spawn(async move {
            agent_supervisor::run_agent_job(
                pool,
                supervisor,
                instance_id,
                run_id,
                repo_id,
                storage_key,
                git_root,
                reference_bin,
                branch,
                message,
                std::time::Duration::from_secs(120),
                budget_tokens,
                Some(head_for_run),
            )
            .await;
        });
    }

    Ok((
        StatusCode::CREATED,
        Json(PrAgentFixRunView {
            id: run_id,
            adapter_kind: adapter.to_string(),
            state: "queued".to_string(),
            pr_id: pr.pr_id,
            fix_round,
            head_oid: head,
            usage_tokens: 0,
            budget_unknown: true,
        }),
    ))
}
