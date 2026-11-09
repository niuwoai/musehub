//! MCP tools backing domain logic (04-mcp-api contract completion).

use std::sync::Arc;

use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::AppError;
use crate::git_http::bare_repo_path;
use crate::mcp::auth::RepoAccess;
use crate::mcp::git_read;
use crate::pipeline;
use crate::release;
use crate::security;
use crate::AppState;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CriterionInput {
    pub description: String,
    #[serde(default = "default_verification")]
    pub verification_method: String,
    #[serde(default = "default_true")]
    pub required: bool,
}

fn default_verification() -> String {
    "manual".to_string()
}
fn default_true() -> bool {
    true
}

pub async fn storage_for_repo(state: &AppState, repo_id: Uuid) -> Result<Uuid, AppError> {
    let row: (Uuid,) = sqlx::query_as("SELECT storage_key FROM repositories WHERE id = $1")
        .bind(repo_id)
        .fetch_one(&state.pool)
        .await?;
    Ok(row.0)
}

pub async fn copy_criteria_to_revision(
    pool: &sqlx::PgPool,
    org_id: Uuid,
    repo_id: Uuid,
    from_revision: Uuid,
    to_revision: Uuid,
) -> Result<(), AppError> {
    let rows = sqlx::query_as::<_, (String, String, bool, i32)>(
        "SELECT description, verification_method, required, ordinal
         FROM issue_acceptance_criteria WHERE issue_revision_id = $1 ORDER BY ordinal",
    )
    .bind(from_revision)
    .fetch_all(pool)
    .await?;
    for (description, method, required, ordinal) in rows {
        sqlx::query(
            "INSERT INTO issue_acceptance_criteria
             (id, org_id, repo_id, issue_revision_id, ordinal, description, verification_method, required)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(Uuid::new_v4())
        .bind(org_id)
        .bind(repo_id)
        .bind(to_revision)
        .bind(ordinal)
        .bind(description)
        .bind(method)
        .bind(required)
        .execute(pool)
        .await?;
    }
    Ok(())
}

pub async fn criteria_for_revision(pool: &sqlx::PgPool, revision_id: Uuid) -> Result<Vec<Value>, AppError> {
    let rows = sqlx::query_as::<_, (i32, String, String, bool)>(
        "SELECT ordinal, description, verification_method, required
         FROM issue_acceptance_criteria WHERE issue_revision_id = $1 ORDER BY ordinal",
    )
    .bind(revision_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(ordinal, description, verification_method, required)| {
            json!({
                "ordinal": ordinal,
                "description": description,
                "verification_method": verification_method,
                "required": required,
            })
        })
        .collect())
}

pub async fn issue_criteria_replace(
    state: &Arc<AppState>,
    access: &RepoAccess,
    principal_id: Uuid,
    issue_id: Uuid,
    expected_version: i32,
    criteria: &[CriterionInput],
) -> Result<Value, AppError> {
    if criteria.is_empty() || criteria.len() > 50 {
        return Err(AppError::Validation(
            "criteria count must be 1..=50".to_string(),
        ));
    }
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
    let revision_id = current_revision_id.ok_or_else(|| {
        AppError::Validation("issue has no current revision".to_string())
    })?;
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
    let new_revision = revision + 1;
    let new_revision_id = Uuid::new_v4();
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO issue_revisions (id, org_id, repo_id, issue_id, revision, title, body, author_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(new_revision_id)
    .bind(access.org_id)
    .bind(access.repo_id)
    .bind(id)
    .bind(new_revision)
    .bind(&rev_title)
    .bind(&rev_body)
    .bind(principal_id)
    .execute(&mut *tx)
    .await?;
    for (idx, c) in criteria.iter().enumerate() {
        let desc = c.description.trim();
        if desc.is_empty() {
            return Err(AppError::Validation("criterion description required".to_string()));
        }
        sqlx::query(
            "INSERT INTO issue_acceptance_criteria
             (id, org_id, repo_id, issue_revision_id, ordinal, description, verification_method, required)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(Uuid::new_v4())
        .bind(access.org_id)
        .bind(access.repo_id)
        .bind(new_revision_id)
        .bind((idx + 1) as i32)
        .bind(desc)
        .bind(c.verification_method.trim())
        .bind(c.required)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "UPDATE issues SET current_revision_id = $2, updated_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(id)
    .bind(new_revision_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let crit = criteria_for_revision(&state.pool, new_revision_id).await?;
    Ok(json!({
        "id": id.to_string(),
        "number": number,
        "title": title,
        "state": issue_state,
        "current_revision_id": new_revision_id.to_string(),
        "version": new_revision,
        "criteria": crit,
    }))
}

pub async fn discussion_list(
    state: &Arc<AppState>,
    access: &RepoAccess,
    target_kind: &str,
    target_id: Uuid,
    limit: i64,
) -> Result<(Vec<Value>, bool), AppError> {
    if !matches!(target_kind, "issue" | "pr") {
        return Err(AppError::Validation("invalid discussion target kind".to_string()));
    }
    if target_kind == "issue" {
        let ok = sqlx::query_as::<_, (Uuid,)>(
            "SELECT id FROM issues WHERE id = $1 AND repo_id = $2",
        )
        .bind(target_id)
        .bind(access.repo_id)
        .fetch_optional(&state.pool)
        .await?;
        if ok.is_none() {
            return Err(AppError::NotFound);
        }
    } else {
        let ok = sqlx::query_as::<_, (Uuid,)>(
            "SELECT id FROM pull_requests WHERE id = $1 AND repo_id = $2",
        )
        .bind(target_id)
        .bind(access.repo_id)
        .fetch_optional(&state.pool)
        .await?;
        if ok.is_none() {
            return Err(AppError::NotFound);
        }
    }
    let fetch = limit + 1;
    let rows = sqlx::query_as::<_, (Uuid, Uuid, String, bool, bool, time::OffsetDateTime)>(
        "SELECT id, author_id, body, resolved, outdated, created_at
         FROM discussion_comments
         WHERE repo_id = $1 AND target_kind = $2 AND target_id = $3
         ORDER BY created_at LIMIT $4",
    )
    .bind(access.repo_id)
    .bind(target_kind)
    .bind(target_id)
    .bind(fetch)
    .fetch_all(&state.pool)
    .await?;
    let has_more = rows.len() as i64 > limit;
    let items: Vec<Value> = rows
        .into_iter()
        .take(limit as usize)
        .map(|(id, author_id, body, resolved, outdated, at)| {
            json!({
                "id": id.to_string(),
                "author_id": author_id.to_string(),
                "body": body,
                "resolved": resolved,
                "outdated": outdated,
                "occurred_at": at.format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_else(|_| at.to_string()),
            })
        })
        .collect();
    Ok((items, has_more))
}

pub async fn discussion_comment(
    state: &Arc<AppState>,
    access: &RepoAccess,
    principal_id: Uuid,
    target_kind: &str,
    target_id: Uuid,
    body: &str,
    anchor: Option<Value>,
) -> Result<Value, AppError> {
    if body.trim().is_empty() || body.len() > 16 * 1024 {
        return Err(AppError::Validation("invalid comment body".to_string()));
    }
    if anchor.is_some() && target_kind != "pr" {
        return Err(AppError::Validation("anchor only allowed on PR discussions".to_string()));
    }
    discussion_list(state, access, target_kind, target_id, 0).await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO discussion_comments (id, org_id, repo_id, target_kind, target_id, author_id, body, anchor)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(id)
    .bind(access.org_id)
    .bind(access.repo_id)
    .bind(target_kind)
    .bind(target_id)
    .bind(principal_id)
    .bind(body)
    .bind(anchor.as_ref())
    .execute(&state.pool)
    .await?;
    Ok(json!({
        "id": id.to_string(),
        "target": { "kind": target_kind, "id": target_id.to_string() },
        "author_id": principal_id.to_string(),
        "body": body,
        "anchor": anchor,
        "resolved": false,
        "outdated": false,
    }))
}

pub async fn discussion_resolve(
    state: &Arc<AppState>,
    access: &RepoAccess,
    principal_id: Uuid,
    comment_id: Uuid,
    expected_version: i64,
    resolved: bool,
) -> Result<Value, AppError> {
    let row = sqlx::query_as::<_, (Uuid, String, Uuid, i64)>(
        "SELECT id, target_kind, target_id, extract(epoch from updated_at)::bigint
         FROM discussion_comments WHERE id = $1 AND repo_id = $2",
    )
    .bind(comment_id)
    .bind(access.repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((id, kind, _target, row_version)) = row else {
        return Err(AppError::NotFound);
    };
    if kind != "pr" {
        return Err(AppError::Validation(
            "resolve only supported for PR discussion".to_string(),
        ));
    }
    if row_version != expected_version {
        return Err(AppError::Validation("VERSION_CONFLICT".to_string()));
    }
    let grant = crate::grants::principal_has_repo_grant(&state.pool, access.repo_id, principal_id)
        .await?
        || crate::grants::is_org_privileged(&state.pool, access.org_id, principal_id).await?;
    if !grant {
        return Err(AppError::Forbidden);
    }
    sqlx::query(
        "UPDATE discussion_comments SET resolved = $2, updated_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(id)
    .bind(resolved)
    .execute(&state.pool)
    .await?;
    Ok(json!({
        "id": id.to_string(),
        "resolved": resolved,
    }))
}

pub async fn pr_update(
    state: &Arc<AppState>,
    access: &RepoAccess,
    pr_id: Uuid,
    expected_version: i64,
    title: Option<&str>,
    state_patch: Option<&str>,
) -> Result<Value, AppError> {
    if title.is_none() && state_patch.is_none() {
        return Err(AppError::Validation("patch must include fields".to_string()));
    }
    let row = sqlx::query_as::<_, (i64, i64, String, String, String, Option<String>, Option<String>, String)>(
        "SELECT row_version, number, title, source_branch, target_branch, head_oid, base_oid, state
         FROM pull_requests WHERE id = $1 AND repo_id = $2",
    )
    .bind(pr_id)
    .bind(access.repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((row_version, number, cur_title, source_branch, target_branch, head_oid, base_oid, pr_state)) =
        row
    else {
        return Err(AppError::NotFound);
    };
    if row_version != expected_version {
        return Err(AppError::Validation(
            "expected_version does not match pull request row_version".to_string(),
        ));
    }
    if pr_state == "merged" {
        return Err(AppError::Validation("cannot update merged pull request".to_string()));
    }
    let mut new_title = cur_title;
    if let Some(t) = title {
        let t = t.trim();
        if t.is_empty() {
            return Err(AppError::Validation("title cannot be empty".to_string()));
        }
        new_title = t.to_string();
    }
    let mut new_state = pr_state.clone();
    if let Some(s) = state_patch {
        let s = s.to_lowercase();
        if s == "merged" {
            return Err(AppError::Validation(
                "use musehub_pr_merge to merge".to_string(),
            ));
        }
        if !matches!(s.as_str(), "open" | "closed") {
            return Err(AppError::Validation("invalid pr state".to_string()));
        }
        new_state = s;
    }
    sqlx::query(
        "UPDATE pull_requests SET title = $2, state = $3, row_version = row_version + 1,
         updated_at = CURRENT_TIMESTAMP WHERE id = $1 AND row_version = $4",
    )
    .bind(pr_id)
    .bind(&new_title)
    .bind(&new_state)
    .bind(expected_version)
    .execute(&state.pool)
    .await?;
    Ok(json!({
        "id": pr_id.to_string(),
        "number": number,
        "title": new_title,
        "state": new_state,
        "source_branch": source_branch,
        "target_branch": target_branch,
        "head_oid": head_oid,
        "base_oid": base_oid,
        "version": expected_version + 1,
    }))
}

pub async fn pr_diff(
    state: &Arc<AppState>,
    access: &RepoAccess,
    pr_id: Uuid,
    pr_revision_id: i64,
    path: Option<&str>,
    limit: i64,
) -> Result<(Vec<Value>, bool), AppError> {
    let row = sqlx::query_as::<_, (i64, Option<String>, Option<String>)>(
        "SELECT row_version, head_oid, base_oid FROM pull_requests WHERE id = $1 AND repo_id = $2",
    )
    .bind(pr_id)
    .bind(access.repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((row_version, head, base)) = row else {
        return Err(AppError::NotFound);
    };
    if row_version != pr_revision_id {
        return Err(AppError::Validation("pr_revision_id mismatch".to_string()));
    }
    let head = head.ok_or_else(|| AppError::Validation("missing head".into()))?;
    let base = base.ok_or_else(|| AppError::Validation("missing base".into()))?;
    let storage = storage_for_repo(state, access.repo_id).await?;
    let bare = bare_repo_path(&state.git_root, &storage);
    git_read::git_pr_file_diff(&bare, &base, &head, path, limit)
}

pub async fn git_repo_access(
    state: &Arc<AppState>,
    access: &RepoAccess,
) -> Result<std::path::PathBuf, AppError> {
    let storage = storage_for_repo(state, access.repo_id).await?;
    Ok(bare_repo_path(&state.git_root, &storage))
}

pub async fn agent_profile_list(_org_id: Uuid, limit: i64) -> Result<Vec<Value>, AppError> {
    Ok(vec![
        json!({
            "id": "reference",
            "name": "reference",
            "adapter_kind": "reference",
            "enabled": true,
            "available_to_caller": true,
        }),
        json!({
            "id": "codex",
            "name": "codex",
            "adapter_kind": "codex",
            "enabled": std::env::var("MUSEHUB_AGENT_CODEX_BIN").is_ok(),
            "available_to_caller": true,
        }),
    ]
    .into_iter()
    .take(limit as usize)
    .collect())
}

pub async fn agent_run_messages(
    state: &Arc<AppState>,
    access: &RepoAccess,
    agent_run_id: Uuid,
    limit: i64,
) -> Result<(Vec<Value>, bool), AppError> {
    let run = sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM agent_runs WHERE id = $1 AND repo_id = $2",
    )
    .bind(agent_run_id)
    .bind(access.repo_id)
    .fetch_optional(&state.pool)
    .await?;
    if run.is_none() {
        return Err(AppError::NotFound);
    }
    let fetch = limit + 1;
    let rows = sqlx::query_as::<_, (i32, String, String, time::OffsetDateTime)>(
        "SELECT sequence, kind, body, created_at FROM agent_run_messages
         WHERE agent_run_id = $1 ORDER BY sequence LIMIT $2",
    )
    .bind(agent_run_id)
    .bind(fetch)
    .fetch_all(&state.pool)
    .await?;
    if rows.is_empty() {
        let task: Option<(String,)> =
            sqlx::query_as("SELECT task_message FROM agent_runs WHERE id = $1")
                .bind(agent_run_id)
                .fetch_optional(&state.pool)
                .await?;
        if let Some((msg,)) = task {
            if !msg.is_empty() {
                return Ok((
                    vec![json!({
                        "sequence": 1,
                        "kind": "task",
                        "body": msg,
                        "timestamp": time::OffsetDateTime::now_utc()
                            .format(&time::format_description::well_known::Rfc3339)
                            .unwrap_or_default(),
                    })],
                    false,
                ));
            }
        }
    }
    let has_more = rows.len() as i64 > limit;
    let items = rows
        .into_iter()
        .take(limit as usize)
        .map(|(seq, kind, body, at)| {
            json!({
                "sequence": seq,
                "kind": kind,
                "body": body,
                "timestamp": at.format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_else(|_| at.to_string()),
            })
        })
        .collect();
    Ok((items, has_more))
}

pub async fn agent_run_respond(
    state: &Arc<AppState>,
    access: &RepoAccess,
    agent_run_id: Uuid,
    message: &str,
) -> Result<Value, AppError> {
    if message.trim().is_empty() {
        return Err(AppError::Validation("message required".to_string()));
    }
    let row = sqlx::query_as::<_, (String,)>(
        "SELECT state FROM agent_runs WHERE id = $1 AND repo_id = $2",
    )
    .bind(agent_run_id)
    .bind(access.repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((state_name,)) = row else {
        return Err(AppError::NotFound);
    };
    if state_name != "waiting_input" {
        return Err(AppError::Validation(
            "agent run is not waiting for input".to_string(),
        ));
    }
    let seq: (i32,) = sqlx::query_as(
        "SELECT COALESCE(MAX(sequence), 0) + 1 FROM agent_run_messages WHERE agent_run_id = $1",
    )
    .bind(agent_run_id)
    .fetch_one(&state.pool)
    .await?;
    sqlx::query(
        "INSERT INTO agent_run_messages (id, org_id, repo_id, agent_run_id, sequence, kind, body)
         VALUES ($1,$2,$3,$4,$5,'user_input',$6)",
    )
    .bind(Uuid::new_v4())
    .bind(access.org_id)
    .bind(access.repo_id)
    .bind(agent_run_id)
    .bind(seq.0)
    .bind(message)
    .execute(&state.pool)
    .await?;
    sqlx::query(
        "UPDATE agent_runs SET state = 'queued', updated_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(agent_run_id)
    .execute(&state.pool)
    .await?;
    Ok(json!({
        "id": agent_run_id.to_string(),
        "state": "queued",
        "message_sequence": seq.0,
    }))
}

pub async fn pipeline_list(
    pool: &sqlx::PgPool,
    repo_id: Uuid,
    state_filter: Option<&str>,
    commit_oid: Option<&str>,
    limit: i64,
) -> Result<(Vec<Value>, bool), AppError> {
    let fetch = limit + 1;
    let rows = if let Some(st) = state_filter {
        if let Some(oid) = commit_oid {
            sqlx::query_as::<_, (Uuid, String, String, String)>(
                "SELECT id, source_oid, workflow_name, state FROM pipeline_runs
                 WHERE repo_id = $1 AND state = $2 AND source_oid = $3
                 ORDER BY created_at DESC LIMIT $4",
            )
            .bind(repo_id)
            .bind(st)
            .bind(oid)
            .bind(fetch)
            .fetch_all(pool)
            .await?
        } else {
            sqlx::query_as::<_, (Uuid, String, String, String)>(
                "SELECT id, source_oid, workflow_name, state FROM pipeline_runs
                 WHERE repo_id = $1 AND state = $2 ORDER BY created_at DESC LIMIT $3",
            )
            .bind(repo_id)
            .bind(st)
            .bind(fetch)
            .fetch_all(pool)
            .await?
        }
    } else {
        sqlx::query_as::<_, (Uuid, String, String, String)>(
            "SELECT id, source_oid, workflow_name, state FROM pipeline_runs
             WHERE repo_id = $1 ORDER BY created_at DESC LIMIT $2",
        )
        .bind(repo_id)
        .bind(fetch)
        .fetch_all(pool)
        .await?
    };
    let has_more = rows.len() as i64 > limit;
    let items = rows
        .into_iter()
        .take(limit as usize)
        .map(|(id, source_oid, workflow_name, state)| {
            json!({
                "id": id.to_string(),
                "source_oid": source_oid,
                "workflow_name": workflow_name,
                "state": state,
            })
        })
        .collect();
    Ok((items, has_more))
}

pub async fn pipeline_get(
    pool: &sqlx::PgPool,
    repo_id: Uuid,
    pipeline_run_id: Uuid,
) -> Result<Value, AppError> {
    let row = sqlx::query_as::<_, (String, String, String, Uuid, Option<String>)>(
        "SELECT source_oid, workflow_name, state, run_attempt_id, error_code
         FROM pipeline_runs WHERE id = $1 AND repo_id = $2",
    )
    .bind(pipeline_run_id)
    .bind(repo_id)
    .fetch_optional(pool)
    .await?;
    let Some((source_oid, workflow_name, state, attempt_id, error_code)) = row else {
        return Err(AppError::NotFound);
    };
    let jobs = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT id, job_key, state FROM pipeline_jobs WHERE pipeline_run_id = $1",
    )
    .bind(pipeline_run_id)
    .fetch_all(pool)
    .await?;
    let job_items: Vec<Value> = jobs
        .into_iter()
        .map(|(id, key, st)| json!({ "id": id.to_string(), "job_key": key, "state": st }))
        .collect();
  let checks = sqlx::query_as::<_, (String, String)>(
        "SELECT check_name, state FROM pr_check_runs WHERE head_oid = $1 LIMIT 20",
    )
    .bind(&source_oid)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    Ok(json!({
        "id": pipeline_run_id.to_string(),
        "source_oid": source_oid,
        "workflow_name": workflow_name,
        "state": state,
        "run_attempt_id": attempt_id.to_string(),
        "error_code": error_code,
        "jobs": job_items,
        "checks": checks.into_iter().map(|(n, s)| json!({ "check_name": n, "state": s })).collect::<Vec<_>>(),
    }))
}

pub async fn job_logs(
    pool: &sqlx::PgPool,
    repo_id: Uuid,
    job_id: Uuid,
    attempt_id: Uuid,
    cursor: Option<i32>,
    max_bytes: usize,
) -> Result<Value, AppError> {
    let job = sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM pipeline_jobs WHERE id = $1 AND repo_id = $2",
    )
    .bind(job_id)
    .bind(repo_id)
    .fetch_optional(pool)
    .await?;
    if job.is_none() {
        return Err(AppError::NotFound);
    }
    let start = cursor.unwrap_or(0);
    let rows = sqlx::query_as::<_, (i32, String, bool)>(
        "SELECT chunk_index, text, complete FROM pipeline_job_logs
         WHERE job_id = $1 AND attempt_id = $2 AND chunk_index >= $3
         ORDER BY chunk_index LIMIT 10",
    )
    .bind(job_id)
    .bind(attempt_id)
    .bind(start)
    .fetch_all(pool)
    .await?;
    let mut text = String::new();
    let mut complete = true;
    let mut next_cursor = None;
    for (idx, chunk, done) in &rows {
        if text.len() + chunk.len() > max_bytes {
            next_cursor = Some(*idx);
            complete = false;
            break;
        }
        text.push_str(chunk);
        complete = *done;
        next_cursor = Some(idx + 1);
    }
    if rows.is_empty() {
        text = "no log chunks recorded for this job attempt".to_string();
        complete = true;
    }
    Ok(json!({
        "text": text,
        "next_cursor": next_cursor,
        "complete": complete,
        "truncated": next_cursor.is_some(),
    }))
}

pub async fn job_retry(
    state: &Arc<AppState>,
    access: &RepoAccess,
    job_id: Uuid,
) -> Result<Value, AppError> {
    let row = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT pipeline_run_id, job_key, state FROM pipeline_jobs WHERE id = $1 AND repo_id = $2",
    )
    .bind(job_id)
    .bind(access.repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((run_id, job_key, job_state)) = row else {
        return Err(AppError::NotFound);
    };
    if job_key == "deploy" {
        return Err(AppError::Validation("deploy job cannot use generic retry".to_string()));
    }
    if job_state != "failed" {
        return Err(AppError::Validation("only failed jobs may be retried".to_string()));
    }
    sqlx::query(
        "UPDATE pipeline_jobs SET state = 'queued', updated_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(job_id)
    .execute(&state.pool)
    .await?;
    sqlx::query(
        "UPDATE pipeline_runs SET state = 'queued', error_code = NULL, updated_at = CURRENT_TIMESTAMP
         WHERE id = $1",
    )
    .bind(run_id)
    .execute(&state.pool)
    .await?;
    if !crate::runner_api::pipeline_executor_remote() {
        let storage = storage_for_repo(state, access.repo_id).await?;
        let pool = state.pool.clone();
        let git_root = state.git_root.clone();
        let artifact_root = state.artifact_root.clone();
        let pipeline_bin = state.pipeline_adapter_bin.clone();
        let supervisor = state.pipeline_supervisor.clone();
        let instance_id = state.instance_id;
        let org_id = access.org_id;
        let repo_id = access.repo_id;
        let source: (String,) =
            sqlx::query_as("SELECT source_oid FROM pipeline_runs WHERE id = $1")
                .bind(run_id)
                .fetch_one(&pool)
                .await?;
        let run_kind: (String,) =
            sqlx::query_as("SELECT run_kind FROM pipeline_runs WHERE id = $1")
                .bind(run_id)
                .fetch_one(&pool)
                .await?;
        tokio::spawn(async move {
            pipeline::execute_pipeline_job(
                pool,
                git_root,
                artifact_root,
                pipeline_bin,
                supervisor,
                instance_id,
                storage,
                run_id,
                job_id,
                org_id,
                repo_id,
                source.0,
                run_kind.0,
            )
            .await;
        });
    }
    Ok(json!({
        "id": job_id.to_string(),
        "state": "queued",
        "pipeline_run_id": run_id.to_string(),
    }))
}

pub async fn pipeline_cancel(
    pool: &sqlx::PgPool,
    repo_id: Uuid,
    pipeline_run_id: Uuid,
) -> Result<Value, AppError> {
    let updated = sqlx::query(
        "UPDATE pipeline_runs SET cancel_requested = TRUE, state = 'cancel_requested',
         updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND repo_id = $2 AND state IN ('queued','running')",
    )
    .bind(pipeline_run_id)
    .bind(repo_id)
    .execute(pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::Validation(
            "pipeline not cancelable".to_string(),
        ));
    }
    sqlx::query(
        "UPDATE pipeline_jobs SET state = 'cancel_requested', updated_at = CURRENT_TIMESTAMP
         WHERE pipeline_run_id = $1 AND state IN ('queued','running')",
    )
    .bind(pipeline_run_id)
    .execute(pool)
    .await?;
    Ok(json!({
        "id": pipeline_run_id.to_string(),
        "state": "cancel_requested",
    }))
}

pub async fn release_list(pool: &sqlx::PgPool, repo_id: Uuid, limit: i64) -> Result<(Vec<Value>, bool), AppError> {
    let fetch = limit + 1;
    let rows = sqlx::query_as::<_, (Uuid, String, String, String, String)>(
        "SELECT id, version, tag_name, commit_oid, state FROM releases
         WHERE repo_id = $1 ORDER BY created_at DESC LIMIT $2",
    )
    .bind(repo_id)
    .bind(fetch)
    .fetch_all(pool)
    .await?;
    let has_more = rows.len() as i64 > limit;
    let items = rows
        .into_iter()
        .take(limit as usize)
        .map(|(id, version, tag, commit, state)| {
            json!({
                "id": id.to_string(),
                "version_name": version,
                "tag_name": tag,
                "commit_oid": commit,
                "state": state,
            })
        })
        .collect();
    Ok((items, has_more))
}

pub async fn release_create_mcp(
    state: &Arc<AppState>,
    access: &RepoAccess,
    version: &str,
    tag_name: &str,
    commit_oid: &str,
    pipeline_run_id: Uuid,
    pr_id: Option<Uuid>,
) -> Result<Value, AppError> {
    security::validate_git_oid(commit_oid).map_err(AppError::Validation)?;
    let req = release::CreateReleaseRequest {
        version: version.to_string(),
        tag_name: tag_name.to_string(),
        commit_oid: commit_oid.to_string(),
        pipeline_run_id,
        pr_id,
    };
    let view = release::create_release_core(state, access.org_id, access.repo_id, req).await?;
    Ok(json!({
        "id": view.id.to_string(),
        "version_name": view.version,
        "tag_name": view.tag_name,
        "commit_oid": view.commit_oid,
        "pipeline_run_id": view.pipeline_run_id.to_string(),
        "state": view.state,
        "manifest_hash": view.manifest_sha256,
    }))
}

pub async fn release_publish_mcp(
    state: &Arc<AppState>,
    access: &RepoAccess,
    release_id: Uuid,
    expected_commit_oid: &str,
    expected_manifest_hash: &str,
) -> Result<Value, AppError> {
    let row = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT commit_oid, manifest_sha256 FROM releases WHERE id = $1 AND repo_id = $2",
    )
    .bind(release_id)
    .bind(access.repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((commit, manifest)) = row else {
        return Err(AppError::NotFound);
    };
    if commit != expected_commit_oid {
        return Err(AppError::Validation("expected_commit_oid mismatch".to_string()));
    }
    if manifest.as_deref() != Some(expected_manifest_hash) {
        return Err(AppError::Validation("expected_manifest_hash mismatch".to_string()));
    }
    let view = release::publish_release_core(
        state,
        access.repo_id,
        release_id,
        release::PublishReleaseRequest::default(),
    )
    .await?;
    Ok(json!({
        "id": release_id.to_string(),
        "state": view.state,
        "commit_oid": view.commit_oid,
        "manifest_hash": view.manifest_sha256,
    }))
}

pub async fn environment_list(pool: &sqlx::PgPool, repo_id: Uuid, limit: i64) -> Result<(Vec<Value>, bool), AppError> {
    let fetch = limit + 1;
    let rows = sqlx::query_as::<_, (Uuid, String, String, i64)>(
        "SELECT id, slug, name, row_version FROM environments WHERE repo_id = $1 ORDER BY slug LIMIT $2",
    )
    .bind(repo_id)
    .bind(fetch)
    .fetch_all(pool)
    .await?;
    let has_more = rows.len() as i64 > limit;
    let items = rows
        .into_iter()
        .take(limit as usize)
        .map(|(id, slug, name, version)| {
            json!({
                "id": id.to_string(),
                "slug": slug,
                "name": name,
                "version": version,
            })
        })
        .collect();
    Ok((items, has_more))
}

pub async fn deployment_list(
    pool: &sqlx::PgPool,
    repo_id: Uuid,
    environment_id: Uuid,
    limit: i64,
) -> Result<(Vec<Value>, bool), AppError> {
    let env = sqlx::query_as::<_, (i32,)>(
        "SELECT 1 FROM environments WHERE id = $1 AND repo_id = $2",
    )
    .bind(environment_id)
    .bind(repo_id)
    .fetch_optional(pool)
    .await?;
    if env.is_none() {
        return Err(AppError::NotFound);
    }
    let fetch = limit + 1;
    let rows = sqlx::query_as::<_, (Uuid, String, Uuid)>(
        "SELECT id, state, release_id FROM deployments
         WHERE repo_id = $1 AND environment_id = $2
         ORDER BY created_at DESC LIMIT $3",
    )
    .bind(repo_id)
    .bind(environment_id)
    .bind(fetch)
    .fetch_all(pool)
    .await?;
    let has_more = rows.len() as i64 > limit;
    let items = rows
        .into_iter()
        .take(limit as usize)
        .map(|(id, state, release_id)| {
            json!({
                "id": id.to_string(),
                "state": state,
                "release_id": release_id.to_string(),
            })
        })
        .collect();
    Ok((items, has_more))
}

pub async fn deployment_approve(
    state: &Arc<AppState>,
    access: &RepoAccess,
    principal_id: Uuid,
    deployment_id: Uuid,
    expected_plan_hash: &str,
    decision: &str,
    reason: &str,
) -> Result<Value, AppError> {
    crate::deployment::apply_deployment_approval(
        state,
        access.org_id,
        access.repo_id,
        principal_id,
        deployment_id,
        expected_plan_hash,
        decision,
        reason,
    )
    .await
}

pub async fn delivery_get(
    pool: &sqlx::PgPool,
    access: &RepoAccess,
    issue_id: Uuid,
    limit: i64,
) -> Result<(Vec<Value>, Value), AppError> {
    let issue = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, state FROM issues WHERE id = $1 AND repo_id = $2",
    )
    .bind(issue_id)
    .bind(access.repo_id)
    .fetch_optional(pool)
    .await?;
    let Some((_, issue_state)) = issue else {
        return Err(AppError::NotFound);
    };
    let mut links = Vec::new();
    let prs = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, state FROM pull_requests WHERE issue_id = $1 AND repo_id = $2 LIMIT $3",
    )
    .bind(issue_id)
    .bind(access.repo_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    for (id, st) in prs {
        links.push(json!({
            "relation": "pull_request",
            "resource_type": "pull_request",
            "resource_id": id.to_string(),
            "state": st,
        }));
    }
    let runs = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, state FROM agent_runs WHERE issue_id = $1 AND repo_id = $2 LIMIT $3",
    )
    .bind(issue_id)
    .bind(access.repo_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    for (id, st) in runs {
        links.push(json!({
            "relation": "agent_run",
            "resource_type": "agent_run",
            "resource_id": id.to_string(),
            "state": st,
        }));
    }
    let status = json!({
        "issue_state": issue_state,
        "links_count": links.len(),
    });
    Ok((links, status))
}

pub(crate) async fn append_pipeline_job_log(
    pool: &sqlx::PgPool,
    org_id: Uuid,
    repo_id: Uuid,
    job_id: Uuid,
    attempt_id: Uuid,
    text: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO pipeline_job_logs (id, org_id, repo_id, job_id, attempt_id, chunk_index, text, complete)
         VALUES ($1,$2,$3,$4,$5,0,$6,true)
         ON CONFLICT (job_id, attempt_id, chunk_index) DO UPDATE SET text = EXCLUDED.text",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(repo_id)
    .bind(job_id)
    .bind(attempt_id)
    .bind(text)
    .execute(pool)
    .await?;
    Ok(())
}
