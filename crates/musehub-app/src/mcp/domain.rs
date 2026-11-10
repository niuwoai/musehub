use std::sync::Arc;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::agent_supervisor;
use crate::error::AppError;
use crate::git_http::{bare_repo_path, git_clone_url, init_bare_repository};
use crate::mcp::auth::RepoAccess;
use crate::mcp::contract_domain::{copy_criteria_to_revision, CriterionInput};
use crate::pipeline;
use crate::pr;
use crate::security;
use crate::AppState;

pub async fn issue_create(
    state: &Arc<AppState>,
    access: &RepoAccess,
    principal_id: Uuid,
    title: &str,
    body: &str,
    criteria: &[CriterionInput],
) -> Result<Value, AppError> {
    if title.trim().is_empty() {
        return Err(AppError::Validation("title required".to_string()));
    }
    let mut tx = state.pool.begin().await?;
    let number: (i64,) = sqlx::query_as(
        "UPDATE repositories SET next_issue_number = next_issue_number + 1
         WHERE id = $1 RETURNING next_issue_number - 1",
    )
    .bind(access.repo_id)
    .fetch_one(&mut *tx)
    .await?;
    let title = title.trim().to_string();
    let issue_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO issues (id, org_id, repo_id, number, author_id, title, body)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(issue_id)
    .bind(access.org_id)
    .bind(access.repo_id)
    .bind(number.0)
    .bind(principal_id)
    .bind(&title)
    .bind(body)
    .execute(&mut *tx)
    .await?;
    let revision_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO issue_revisions (id, org_id, repo_id, issue_id, revision, title, body, author_id)
         VALUES ($1, $2, $3, $4, 1, $5, $6, $7)",
    )
    .bind(revision_id)
    .bind(access.org_id)
    .bind(access.repo_id)
    .bind(issue_id)
    .bind(&title)
    .bind(body)
    .bind(principal_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE issues SET current_revision_id = $2 WHERE id = $1")
        .bind(issue_id)
        .bind(revision_id)
        .execute(&mut *tx)
        .await?;
    for (idx, c) in criteria.iter().enumerate() {
        let desc = c.description.trim();
        if desc.is_empty() {
            continue;
        }
        sqlx::query(
            "INSERT INTO issue_acceptance_criteria
             (id, org_id, repo_id, issue_revision_id, ordinal, description, verification_method, required)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(Uuid::new_v4())
        .bind(access.org_id)
        .bind(access.repo_id)
        .bind(revision_id)
        .bind((idx + 1) as i32)
        .bind(desc)
        .bind(c.verification_method.trim())
        .bind(c.required)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(json!({
        "id": issue_id.to_string(),
        "number": number.0,
        "title": title,
        "state": "open",
        "current_revision_id": revision_id.to_string()
    }))
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, rmcp::schemars::JsonSchema)]
pub struct PlanStepInput {
    pub key: String,
    pub title: String,
    pub description: String,
    pub criterion_ordinals: Vec<i32>,
}

pub fn validate_plan_steps(
    steps: &[PlanStepInput],
    valid_ordinals: &[i32],
) -> Result<(), AppError> {
    let mut keys = std::collections::HashSet::new();
    for step in steps {
        if step.key.trim().is_empty() || step.title.trim().is_empty() {
            return Err(AppError::Validation(
                "plan step key and title are required".to_string(),
            ));
        }
        if !keys.insert(step.key.clone()) {
            return Err(AppError::Validation("duplicate plan step key".to_string()));
        }
        for ord in &step.criterion_ordinals {
            if *ord < 1 {
                return Err(AppError::Validation(
                    "criterion_ordinals must be positive integers".to_string(),
                ));
            }
            if !valid_ordinals.is_empty() && !valid_ordinals.contains(ord) {
                return Err(AppError::Validation(format!(
                    "criterion ordinal {ord} not found on issue revision"
                )));
            }
        }
    }
    Ok(())
}

pub async fn plan_create(
    state: &Arc<AppState>,
    access: &RepoAccess,
    issue_revision_id: Uuid,
    body: &str,
    steps: &[PlanStepInput],
) -> Result<Value, AppError> {
    let revision = sqlx::query_as::<_, (Uuid,)>(
        "SELECT r.id FROM issue_revisions r
         JOIN issues i ON i.id = r.issue_id
         WHERE r.id = $1 AND i.repo_id = $2",
    )
    .bind(issue_revision_id)
    .bind(access.repo_id)
    .fetch_optional(&state.pool)
    .await?;
    if revision.is_none() {
        return Err(AppError::NotFound);
    }
    let criteria_ordinals = sqlx::query_as::<_, (i32,)>(
        "SELECT ordinal FROM issue_acceptance_criteria WHERE issue_revision_id = $1 ORDER BY ordinal",
    )
    .bind(issue_revision_id)
    .fetch_all(&state.pool)
    .await?;
    let valid: Vec<i32> = criteria_ordinals.into_iter().map(|(o,)| o).collect();
    validate_plan_steps(steps, &valid)?;
    let steps_json = serde_json::to_value(steps).map_err(|e| AppError::Other(e.to_string()))?;
    let version: (i32,) = sqlx::query_as(
        "SELECT COALESCE(MAX(version), 0) + 1 FROM implementation_plans WHERE issue_revision_id = $1",
    )
    .bind(issue_revision_id)
    .fetch_one(&state.pool)
    .await?;
    let plan_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO implementation_plans (id, org_id, repo_id, issue_revision_id, version, body, steps, state)
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'draft')",
    )
    .bind(plan_id)
    .bind(access.org_id)
    .bind(access.repo_id)
    .bind(issue_revision_id)
    .bind(version.0)
    .bind(body)
    .bind(steps_json.clone())
    .execute(&state.pool)
    .await?;
    Ok(json!({
        "id": plan_id.to_string(),
        "version": version.0,
        "state": "draft",
        "issue_revision_id": issue_revision_id.to_string(),
        "body": body,
        "steps": steps_json,
    }))
}

pub async fn plan_accept(
    state: &Arc<AppState>,
    access: &RepoAccess,
    plan_id: Uuid,
) -> Result<Value, AppError> {
    let row = sqlx::query_as::<_, (Uuid,)>(
        "SELECT p.issue_revision_id FROM implementation_plans p
         WHERE p.id = $1 AND p.repo_id = $2 AND p.state = 'draft'",
    )
    .bind(plan_id)
    .bind(access.repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((revision_id,)) = row else {
        return Err(AppError::NotFound);
    };
    let updated = sqlx::query(
        "UPDATE implementation_plans p SET state = 'accepted', accepted_at = CURRENT_TIMESTAMP
         FROM issues i
         WHERE p.issue_revision_id = $1 AND p.id = $2
           AND i.current_revision_id = $1 AND i.repo_id = $3",
    )
    .bind(revision_id)
    .bind(plan_id)
    .bind(access.repo_id)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::Validation(
            "plan must match issue current revision".to_string(),
        ));
    }
    Ok(json!({
        "id": plan_id.to_string(),
        "state": "accepted",
        "issue_revision_id": revision_id.to_string()
    }))
}

pub async fn agent_run_start(
    state: &Arc<AppState>,
    access: &RepoAccess,
    principal_id: Uuid,
    issue_id: Uuid,
    message: &str,
    budget_tokens: Option<u64>,
    adapter: Option<&str>,
) -> Result<Value, AppError> {
    let issue =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM issues WHERE id = $1 AND repo_id = $2")
            .bind(issue_id)
            .bind(access.repo_id)
            .fetch_optional(&state.pool)
            .await?;
    if issue.is_none() {
        return Err(AppError::NotFound);
    }
    let plan_ok = sqlx::query_as::<_, (i64,)>(
        "SELECT COUNT(*) FROM implementation_plans p
         JOIN issues i ON i.id = $1 AND i.repo_id = $2
         WHERE p.state = 'accepted' AND p.issue_revision_id = i.current_revision_id",
    )
    .bind(issue_id)
    .bind(access.repo_id)
    .fetch_one(&state.pool)
    .await?;
    if plan_ok.0 == 0 {
        return Err(AppError::Validation(
            "agent run requires accepted plan on current issue revision".to_string(),
        ));
    }
    let storage_key: (Uuid,) = sqlx::query_as("SELECT storage_key FROM repositories WHERE id = $1")
        .bind(access.repo_id)
        .fetch_one(&state.pool)
        .await?;
    let run_id = Uuid::new_v4();
    let branch = format!("agent/{run_id}");
    let default_branch: (String,) =
        sqlx::query_as("SELECT default_branch FROM repositories WHERE id = $1")
            .bind(access.repo_id)
            .fetch_one(&state.pool)
            .await?;
    security::validate_agent_work_branch(&branch, &default_branch.0)
        .map_err(AppError::Validation)?;
    let remote_agent = crate::runner_api::agent_executor_remote();
    let instance_id = state.instance_id;
    let adapter_kind = crate::agent_adapter::adapter_kind_for_request(adapter);
    let message_owned = message.to_string();
    if remote_agent {
        sqlx::query(
            "INSERT INTO agent_runs (id, org_id, repo_id, issue_id, requested_by, adapter_kind, work_branch, state, budget_tokens_limit, task_message)
             VALUES ($1,$2,$3,$4,$5,$6,$7,'queued',$8,$9)",
        )
        .bind(run_id)
        .bind(access.org_id)
        .bind(access.repo_id)
        .bind(issue_id)
        .bind(principal_id)
        .bind(adapter_kind)
        .bind(&branch)
        .bind(budget_tokens.map(|v| v as i64))
        .bind(&message_owned)
        .execute(&state.pool)
        .await?;
    } else {
        sqlx::query(
            "INSERT INTO agent_runs (id, org_id, repo_id, issue_id, requested_by, adapter_kind, work_branch, state, budget_tokens_limit, task_message, lease_holder, lease_until)
             VALUES ($1,$2,$3,$4,$5,$6,$7,'queued',$8,$9,$10,
               CURRENT_TIMESTAMP + ($11::text || ' seconds')::interval)",
        )
        .bind(run_id)
        .bind(access.org_id)
        .bind(access.repo_id)
        .bind(issue_id)
        .bind(principal_id)
        .bind(adapter_kind)
        .bind(&branch)
        .bind(budget_tokens.map(|v| v as i64))
        .bind(&message_owned)
        .bind(instance_id)
        .bind(crate::lease::lease_secs())
        .execute(&state.pool)
        .await?;
        let supervisor = state.supervisor.clone();
        let pool = state.pool.clone();
        let git_root = state.git_root.clone();
        let reference_bin = state.reference_adapter_bin.clone();
        let deadline = std::time::Duration::from_secs(120);
        let repo_id = access.repo_id;
        let storage = storage_key.0;
        tokio::spawn(async move {
            agent_supervisor::run_agent_job(
                pool,
                supervisor,
                instance_id,
                run_id,
                repo_id,
                storage,
                git_root,
                reference_bin,
                branch,
                message_owned,
                deadline,
                budget_tokens,
            )
            .await;
        });
    }
    let kind = crate::agent_adapter::adapter_kind_for_request(adapter);
    Ok(json!({
        "id": run_id.to_string(),
        "adapter_kind": kind,
        "state": "queued",
        "issue_id": issue_id.to_string()
    }))
}

pub async fn agent_run_cancel(
    state: &Arc<AppState>,
    principal_id: Uuid,
    agent_run_id: Uuid,
    repo_id: Uuid,
) -> Result<Value, AppError> {
    let updated = sqlx::query(
        "UPDATE agent_runs SET state = 'cancel_requested', updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND requested_by = $2 AND repo_id = $3 AND state IN ('queued', 'running')",
    )
    .bind(agent_run_id)
    .bind(principal_id)
    .bind(repo_id)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    let _ = state.supervisor.cancel(agent_run_id).await;
    Ok(json!({
        "id": agent_run_id.to_string(),
        "state": "cancel_requested"
    }))
}

pub async fn pr_review(
    state: &Arc<AppState>,
    access: &RepoAccess,
    principal_id: Uuid,
    pr_id: Uuid,
    decision: &str,
    body: &str,
) -> Result<Value, AppError> {
    let decision = decision.to_lowercase();
    if !matches!(decision.as_str(), "approve" | "request_changes" | "comment") {
        return Err(AppError::Validation("invalid review decision".to_string()));
    }
    let pr = sqlx::query_as::<_, (Uuid, i64, Uuid, String, String)>(
        "SELECT id, number, author_id, state, source_branch FROM pull_requests WHERE id = $1 AND repo_id = $2",
    )
    .bind(pr_id)
    .bind(access.repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((id, number, author_id, pr_state, source_branch)) = pr else {
        return Err(AppError::NotFound);
    };
    if pr_state != "open" {
        return Err(AppError::Validation("pull request is not open".to_string()));
    }
    if decision == "approve" && principal_id == author_id {
        return Err(AppError::Validation(
            "author cannot approve their own pull request".to_string(),
        ));
    }
    let storage_key: (Uuid,) = sqlx::query_as("SELECT storage_key FROM repositories WHERE id = $1")
        .bind(access.repo_id)
        .fetch_one(&state.pool)
        .await?;
    let bare = bare_repo_path(&state.git_root, &storage_key.0);
    let head_oid = pr::git_rev_parse_public(&bare, &format!("refs/heads/{source_branch}"))?;
    let approved_head = if decision == "approve" {
        Some(head_oid.as_str())
    } else {
        None
    };
    let review_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pr_reviews (id, org_id, repo_id, pr_id, reviewer_id, decision, body, approved_head_oid)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(review_id)
    .bind(access.org_id)
    .bind(access.repo_id)
    .bind(id)
    .bind(principal_id)
    .bind(&decision)
    .bind(body)
    .bind(approved_head)
    .execute(&state.pool)
    .await?;
    Ok(json!({
        "review_id": review_id.to_string(),
        "pr_id": id.to_string(),
        "pr_number": number,
        "decision": decision
    }))
}

pub async fn pipeline_run(
    state: &Arc<AppState>,
    access: &RepoAccess,
    principal_id: Uuid,
    workflow_path: &str,
    target_oid: &str,
) -> Result<Value, AppError> {
    security::validate_git_oid(target_oid).map_err(AppError::Validation)?;
    let storage_key: (Uuid,) = sqlx::query_as("SELECT storage_key FROM repositories WHERE id = $1")
        .bind(access.repo_id)
        .fetch_one(&state.pool)
        .await?;
    let bare = bare_repo_path(&state.git_root, &storage_key.0);
    let live_main = pr::git_rev_parse_public(&bare, "refs/heads/main")?;
    if target_oid != live_main {
        return Err(AppError::Validation(
            "target_oid must match current main tip".to_string(),
        ));
    }
    let merged = sqlx::query_as::<_, (Uuid, Option<Uuid>, Option<Uuid>)>(
        "SELECT id, issue_id, agent_run_id FROM pull_requests
         WHERE repo_id = $1 AND state = 'merged' AND merged_oid = $2
         ORDER BY merged_at DESC NULLS LAST LIMIT 1",
    )
    .bind(access.repo_id)
    .bind(target_oid)
    .fetch_optional(&state.pool)
    .await?;
    let Some((merged_pr_id, issue_id, agent_run_id)) = merged else {
        return Err(AppError::Validation(
            "target_oid must be a merged pull request commit".to_string(),
        ));
    };
    let workflow = if workflow_path.trim().is_empty() {
        "product-ci".to_string()
    } else {
        workflow_path.trim().to_string()
    };
    let (_, axum::Json(view)) = crate::pipeline::spawn_pipeline_run(
        state,
        access.org_id,
        access.repo_id,
        storage_key.0,
        target_oid.to_string(),
        workflow,
        "release_build",
        principal_id,
        Some(merged_pr_id),
        issue_id,
        agent_run_id,
    )
    .await?;
    Ok(json!({
        "id": view.id.to_string(),
        "source_oid": view.source_oid,
        "workflow_name": view.workflow_name,
        "state": view.state,
        "run_attempt_id": view.run_attempt_id.to_string(),
        "error_code": view.error_code,
    }))
}

pub async fn deployment_start(
    state: &Arc<AppState>,
    access: &RepoAccess,
    environment_id: Uuid,
    release_id: Uuid,
) -> Result<Value, AppError> {
    crate::deployment::mcp_start_deployment(
        state,
        access.org_id,
        access.repo_id,
        environment_id,
        release_id,
    )
    .await
}

pub async fn deployment_rollback(
    state: &Arc<AppState>,
    access: &RepoAccess,
    environment_id: Uuid,
    target_deployment_id: Uuid,
    attempt_id: Uuid,
    reason: Option<String>,
) -> Result<Value, AppError> {
    let pat = state.mcp_pat_slot.read().await;
    let actor = pat.as_ref().map(|p| p.principal_id);
    drop(pat);
    let (status, axum::Json(view)) = crate::deployment::mcp_rollback_deployment(
        state,
        access.org_id,
        access.repo_id,
        environment_id,
        target_deployment_id,
        attempt_id,
        reason,
        actor,
        None,
    )
    .await?;
    Ok(json!({
        "id": view.id.to_string(),
        "release_id": view.release_id.to_string(),
        "environment_slug": view.environment_slug,
        "state": view.state,
        "adapter_kind": view.adapter_kind,
        "attempt_id": view.attempt_id.map(|u| u.to_string()),
        "health_summary": view.health_summary,
        "http_status": status.as_u16(),
    }))
}

pub fn repo_summary_json(
    bind_display: &str,
    org_slug: &str,
    id: Uuid,
    slug: &str,
    state: &str,
) -> Value {
    json!({
        "id": id.to_string(),
        "slug": slug,
        "state": state,
        "clone_url": git_clone_url(bind_display, org_slug, slug),
    })
}

fn normalize_repo_slug(slug: &str) -> Result<String, AppError> {
    let slug = slug.trim().to_lowercase();
    if slug.is_empty() || !slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(AppError::Validation("invalid repository slug".to_string()));
    }
    Ok(slug)
}

#[derive(Debug, Default)]
pub struct IssueUpdatePatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub state: Option<String>,
}

pub async fn repository_create(
    state: &Arc<AppState>,
    org_id: Uuid,
    org_slug: &str,
    principal_id: Uuid,
    slug: &str,
    description: Option<&str>,
    default_branch: Option<&str>,
) -> Result<Value, AppError> {
    let slug = normalize_repo_slug(slug)?;
    if !crate::grants::is_org_privileged(&state.pool, org_id, principal_id).await? {
        return Err(AppError::Forbidden);
    }
    let dup = sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM repositories WHERE org_id = $1 AND slug_normalized = $2",
    )
    .bind(org_id)
    .bind(&slug)
    .fetch_optional(&state.pool)
    .await?;
    if dup.is_some() {
        return Err(AppError::Validation(
            "repository slug already exists".to_string(),
        ));
    }
    let branch = default_branch
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("main")
        .to_string();
    if branch.contains("..") || branch.starts_with('/') {
        return Err(AppError::Validation("invalid default_branch".to_string()));
    }
    let storage_key = Uuid::new_v4();
    let repo_id = Uuid::new_v4();
    init_bare_repository(&state.git_root, &storage_key)?;
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO repositories (id, org_id, slug, slug_normalized, description, storage_key, state, default_branch)
         VALUES ($1, $2, $3, $4, $5, $6, 'ready', $7)",
    )
    .bind(repo_id)
    .bind(org_id)
    .bind(&slug)
    .bind(&slug)
    .bind(description.unwrap_or_default())
    .bind(storage_key)
    .bind(&branch)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO repository_grants (id, org_id, repo_id, principal_id, role)
         VALUES ($1, $2, $3, $4, 'maintainer')",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(repo_id)
    .bind(principal_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    pr::seed_repo_required_checks(&state.pool, repo_id).await?;
    pipeline::seed_repo_environment(&state.pool, org_id, repo_id).await?;
    let mut data = repo_summary_json(&state.bind_display, org_slug, repo_id, &slug, "ready");
    if let Some(map) = data.as_object_mut() {
        map.insert("org_id".into(), json!(org_id.to_string()));
        map.insert("description".into(), json!(description.unwrap_or_default()));
        map.insert("default_branch".into(), json!(branch));
    }
    Ok(data)
}

pub async fn issue_update(
    state: &Arc<AppState>,
    access: &RepoAccess,
    principal_id: Uuid,
    issue_id: Uuid,
    expected_version: i32,
    patch: IssueUpdatePatch,
) -> Result<Value, AppError> {
    let row = sqlx::query_as::<_, (Uuid, i64, String, String, Option<Uuid>)>(
        "SELECT i.id, i.number, i.title, i.state, i.current_revision_id
         FROM issues i WHERE i.id = $1 AND i.repo_id = $2",
    )
    .bind(issue_id)
    .bind(access.repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((id, number, title, issue_state, current_revision_id)) = row else {
        return Err(AppError::NotFound);
    };
    let revision_id = current_revision_id
        .ok_or_else(|| AppError::Validation("issue has no current revision".to_string()))?;
    let rev = sqlx::query_as::<_, (i32, String, String)>(
        "SELECT revision, title, body FROM issue_revisions WHERE id = $1 AND issue_id = $2",
    )
    .bind(revision_id)
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((revision, rev_title, rev_body)) = rev else {
        return Err(AppError::NotFound);
    };
    if revision != expected_version {
        return Err(AppError::Validation(format!(
            "expected_version {expected_version} does not match current revision {revision}"
        )));
    }
    if patch.title.is_none() && patch.body.is_none() && patch.state.is_none() {
        return Err(AppError::Validation(
            "patch must include at least one field".to_string(),
        ));
    }
    let mut new_title = title;
    let rev_body_snapshot = rev_body.clone();
    let mut new_body = rev_body;
    let mut content_changed = false;
    if let Some(t) = patch.title {
        let t = t.trim().to_string();
        if t.is_empty() {
            return Err(AppError::Validation("title cannot be empty".to_string()));
        }
        if t != rev_title {
            new_title = t;
            content_changed = true;
        }
    }
    if let Some(b) = patch.body {
        if b != rev_body_snapshot {
            new_body = b;
            content_changed = true;
        }
    }
    let new_state = if let Some(s) = patch.state {
        let s = s.trim().to_lowercase();
        if !matches!(s.as_str(), "open" | "closed") {
            return Err(AppError::Validation("invalid issue state".to_string()));
        }
        s
    } else {
        issue_state.clone()
    };
    let mut tx = state.pool.begin().await?;
    let mut out_revision = revision;
    let mut out_revision_id = revision_id;
    if content_changed {
        out_revision = revision + 1;
        out_revision_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO issue_revisions (id, org_id, repo_id, issue_id, revision, title, body, author_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(out_revision_id)
        .bind(access.org_id)
        .bind(access.repo_id)
        .bind(id)
        .bind(out_revision)
        .bind(&new_title)
        .bind(&new_body)
        .bind(principal_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE issues SET title = $2, body = $3, current_revision_id = $4, updated_at = CURRENT_TIMESTAMP
             WHERE id = $1",
        )
        .bind(id)
        .bind(&new_title)
        .bind(&new_body)
        .bind(out_revision_id)
        .execute(&mut *tx)
        .await?;
        copy_criteria_to_revision(
            &state.pool,
            access.org_id,
            access.repo_id,
            revision_id,
            out_revision_id,
        )
        .await?;
    }
    if new_state != issue_state {
        sqlx::query("UPDATE issues SET state = $2, updated_at = CURRENT_TIMESTAMP WHERE id = $1")
            .bind(id)
            .bind(&new_state)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(json!({
        "id": id.to_string(),
        "number": number,
        "title": new_title,
        "state": new_state,
        "body": new_body,
        "current_revision_id": out_revision_id.to_string(),
        "version": out_revision,
    }))
}

pub async fn pr_create(
    state: &Arc<AppState>,
    access: &RepoAccess,
    principal_id: Uuid,
    title: &str,
    source_branch: &str,
    target_branch: Option<&str>,
    expected_head_oid: &str,
    issue_revision_ids: &[Uuid],
    _draft: bool,
) -> Result<Value, AppError> {
    let issue_id = if issue_revision_ids.is_empty() {
        None
    } else {
        let rev_id = issue_revision_ids[0];
        let row = sqlx::query_as::<_, (Uuid,)>(
            "SELECT issue_id FROM issue_revisions WHERE id = $1 AND repo_id = $2",
        )
        .bind(rev_id)
        .bind(access.repo_id)
        .fetch_optional(&state.pool)
        .await?;
        let Some((issue_id,)) = row else {
            return Err(AppError::NotFound);
        };
        for extra in issue_revision_ids.iter().skip(1) {
            let ok = sqlx::query_as::<_, (Uuid,)>(
                "SELECT issue_id FROM issue_revisions WHERE id = $1 AND repo_id = $2",
            )
            .bind(extra)
            .bind(access.repo_id)
            .fetch_optional(&state.pool)
            .await?;
            if ok.is_none() {
                return Err(AppError::NotFound);
            }
        }
        Some(issue_id)
    };
    let storage_key: (Uuid,) = sqlx::query_as("SELECT storage_key FROM repositories WHERE id = $1")
        .bind(access.repo_id)
        .fetch_one(&state.pool)
        .await?;
    pr::create_pull_request_in_repo(
        state,
        access.org_id,
        access.repo_id,
        storage_key.0,
        principal_id,
        pr::CreatePrParams {
            title: title.to_string(),
            source_branch: source_branch.to_string(),
            target_branch: target_branch.map(str::to_string),
            expected_head_oid: expected_head_oid.to_string(),
            issue_id,
        },
    )
    .await
}
