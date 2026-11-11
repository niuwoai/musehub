//! Pull request create / review / merge gates.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::AppError;
use crate::git_http::bare_repo_path;
use crate::AppState;

pub const DEFAULT_REPO_CHECK: &str = "product-ci";

#[derive(Serialize)]
pub struct PullRequestView {
    pub id: Uuid,
    pub number: i64,
    pub title: String,
    pub source_branch: String,
    pub target_branch: String,
    pub state: String,
    pub head_oid: Option<String>,
    pub base_oid: Option<String>,
    pub issue_id: Option<Uuid>,
}

#[derive(Deserialize)]
pub struct CreatePullRequestRequest {
    pub title: String,
    pub source_branch: String,
    pub target_branch: Option<String>,
    pub issue_id: Option<Uuid>,
    pub agent_run_id: Option<Uuid>,
}

#[derive(Deserialize)]
pub struct ReviewPullRequestRequest {
    pub decision: String,
    pub body: Option<String>,
}

#[derive(Deserialize)]
pub struct SetCheckRequest {
    pub state: String,
    /// Unique attempt id from the CI system reporting this result (required for success/failure).
    pub attempt_id: Option<Uuid>,
}

async fn require_check_reporter(
    pool: &sqlx::PgPool,
    org_id: Uuid,
    principal_id: Uuid,
    author_id: Uuid,
) -> Result<(), AppError> {
    if principal_id == author_id {
        return Err(AppError::Forbidden);
    }
    let row = sqlx::query_as::<_, (String,)>(
        "SELECT role FROM memberships WHERE org_id = $1 AND principal_id = $2 AND status = 'active'",
    )
    .bind(org_id)
    .bind(principal_id)
    .fetch_optional(pool)
    .await?;
    let Some((role,)) = row else {
        return Err(AppError::Forbidden);
    };
    if role == "owner" || role == "admin" {
        return Ok(());
    }
    Err(AppError::Forbidden)
}

pub async fn seed_repo_required_checks(pool: &sqlx::PgPool, repo_id: Uuid) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO repo_required_checks (id, repo_id, check_name)
         VALUES ($1, $2, $3)
         ON CONFLICT (repo_id, check_name) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(repo_id)
    .bind(DEFAULT_REPO_CHECK)
    .execute(pool)
    .await?;
    Ok(())
}

async fn insert_pr_checks(
    pool: &sqlx::PgPool,
    org_id: Uuid,
    repo_id: Uuid,
    pr_id: Uuid,
    head_oid: &str,
) -> Result<(), AppError> {
    let checks = sqlx::query_as::<_, (String,)>(
        "SELECT check_name FROM repo_required_checks WHERE repo_id = $1",
    )
    .bind(repo_id)
    .fetch_all(pool)
    .await?;
    for (check_name,) in checks {
        sqlx::query(
            "INSERT INTO pr_check_runs (id, org_id, repo_id, pr_id, check_name, head_oid, state)
             VALUES ($1, $2, $3, $4, $5, $6, 'pending')
             ON CONFLICT (pr_id, check_name, head_oid) DO NOTHING",
        )
        .bind(Uuid::new_v4())
        .bind(org_id)
        .bind(repo_id)
        .bind(pr_id)
        .bind(check_name)
        .bind(head_oid)
        .execute(pool)
        .await?;
    }
    Ok(())
}

pub(crate) fn git_rev_parse_public(bare: &Path, refspec: &str) -> Result<String, AppError> {
    git_rev_parse(bare, refspec)
}

fn git_rev_parse(bare: &Path, refspec: &str) -> Result<String, AppError> {
    let output = Command::new("git")
        .args([
            "--git-dir",
            bare.to_str()
                .ok_or_else(|| AppError::Other("path".to_string()))?,
            "rev-parse",
            refspec,
        ])
        .output()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if !output.status.success() {
        return Err(AppError::Validation(format!("unknown ref {refspec}")));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn commits_ahead(bare: &Path, base: &str, head: &str) -> Result<u64, AppError> {
    let output = Command::new("git")
        .args([
            "--git-dir",
            bare.to_str()
                .ok_or_else(|| AppError::Other("path".to_string()))?,
            "rev-list",
            "--count",
            &format!("{base}..{head}"),
        ])
        .output()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if !output.status.success() {
        return Err(AppError::Validation(
            "could not compare branches for diff".to_string(),
        ));
    }
    let count = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .unwrap_or(0);
    Ok(count)
}

pub async fn create_pull_request(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo)): AxumPath<(String, String)>,
    Json(body): Json<CreatePullRequestRequest>,
) -> Result<(StatusCode, Json<PullRequestView>), AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id, storage_key) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    if body.title.trim().is_empty() {
        return Err(AppError::Validation("title required".to_string()));
    }
    let default_branch: (String,) =
        sqlx::query_as("SELECT default_branch FROM repositories WHERE id = $1")
            .bind(repo_id)
            .fetch_one(&state.pool)
            .await?;
    let target = body
        .target_branch
        .clone()
        .unwrap_or_else(|| default_branch.0.clone());
    if body.source_branch == target {
        return Err(AppError::Validation(
            "source and target branches must differ".to_string(),
        ));
    }
    let bare = bare_repo_path(&state.git_root, &storage_key);
    let head_oid = git_rev_parse(&bare, &format!("refs/heads/{}", body.source_branch))?;
    let base_oid = git_rev_parse(&bare, &format!("refs/heads/{target}"))?;
    let ahead = commits_ahead(&bare, &base_oid, &head_oid)?;
    if ahead == 0 {
        return Err(AppError::Validation(
            "pull request requires a non-empty diff".to_string(),
        ));
    }
    if let Some(issue_id) = body.issue_id {
        let ok =
            sqlx::query_as::<_, (Uuid,)>("SELECT id FROM issues WHERE id = $1 AND repo_id = $2")
                .bind(issue_id)
                .bind(repo_id)
                .fetch_optional(&state.pool)
                .await?;
        if ok.is_none() {
            return Err(AppError::NotFound);
        }
    }
    let number: (i64,) = sqlx::query_as(
        "UPDATE repositories SET next_pr_number = next_pr_number + 1
         WHERE id = $1 RETURNING next_pr_number - 1",
    )
    .bind(repo_id)
    .fetch_one(&state.pool)
    .await?;
    let pr_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pull_requests (
            id, org_id, repo_id, number, author_id, title, source_branch, target_branch,
            issue_id, agent_run_id, head_oid, base_oid, state
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, 'open')",
    )
    .bind(pr_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(number.0)
    .bind(principal_id)
    .bind(body.title.trim())
    .bind(&body.source_branch)
    .bind(&target)
    .bind(body.issue_id)
    .bind(body.agent_run_id)
    .bind(&head_oid)
    .bind(&base_oid)
    .execute(&state.pool)
    .await?;
    insert_pr_checks(&state.pool, org_id, repo_id, pr_id, &head_oid).await?;
    Ok((
        StatusCode::CREATED,
        Json(PullRequestView {
            id: pr_id,
            number: number.0,
            title: body.title.trim().to_string(),
            source_branch: body.source_branch,
            target_branch: target,
            state: "open".to_string(),
            head_oid: Some(head_oid),
            base_oid: Some(base_oid),
            issue_id: body.issue_id,
        }),
    ))
}

#[derive(Debug)]
pub struct CreatePrParams {
    pub title: String,
    pub source_branch: String,
    pub target_branch: Option<String>,
    pub expected_head_oid: String,
    pub issue_id: Option<Uuid>,
}

pub async fn create_pull_request_in_repo(
    state: &Arc<AppState>,
    org_id: Uuid,
    repo_id: Uuid,
    storage_key: Uuid,
    principal_id: Uuid,
    params: CreatePrParams,
) -> Result<Value, AppError> {
    if params.title.trim().is_empty() {
        return Err(AppError::Validation("title required".to_string()));
    }
    crate::security::validate_git_oid(&params.expected_head_oid).map_err(AppError::Validation)?;
    let default_branch: (String,) =
        sqlx::query_as("SELECT default_branch FROM repositories WHERE id = $1")
            .bind(repo_id)
            .fetch_one(&state.pool)
            .await?;
    let target = params
        .target_branch
        .clone()
        .unwrap_or_else(|| default_branch.0.clone());
    if params.source_branch == target {
        return Err(AppError::Validation(
            "source and target branches must differ".to_string(),
        ));
    }
    let bare = bare_repo_path(&state.git_root, &storage_key);
    let head_oid = git_rev_parse(&bare, &format!("refs/heads/{}", params.source_branch))?;
    if head_oid != params.expected_head_oid {
        return Err(AppError::Validation(
            "expected_head_oid does not match source branch tip".to_string(),
        ));
    }
    let base_oid = git_rev_parse(&bare, &format!("refs/heads/{target}"))?;
    let ahead = commits_ahead(&bare, &base_oid, &head_oid)?;
    if ahead == 0 {
        return Err(AppError::Validation(
            "pull request requires a non-empty diff".to_string(),
        ));
    }
    if let Some(issue_id) = params.issue_id {
        let ok =
            sqlx::query_as::<_, (Uuid,)>("SELECT id FROM issues WHERE id = $1 AND repo_id = $2")
                .bind(issue_id)
                .bind(repo_id)
                .fetch_optional(&state.pool)
                .await?;
        if ok.is_none() {
            return Err(AppError::NotFound);
        }
    }
    let number: (i64,) = sqlx::query_as(
        "UPDATE repositories SET next_pr_number = next_pr_number + 1
         WHERE id = $1 RETURNING next_pr_number - 1",
    )
    .bind(repo_id)
    .fetch_one(&state.pool)
    .await?;
    let pr_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pull_requests (
            id, org_id, repo_id, number, author_id, title, source_branch, target_branch,
            issue_id, agent_run_id, head_oid, base_oid, state
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NULL, $10, $11, 'open')",
    )
    .bind(pr_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(number.0)
    .bind(principal_id)
    .bind(params.title.trim())
    .bind(&params.source_branch)
    .bind(&target)
    .bind(params.issue_id)
    .bind(&head_oid)
    .bind(&base_oid)
    .execute(&state.pool)
    .await?;
    insert_pr_checks(&state.pool, org_id, repo_id, pr_id, &head_oid).await?;
    Ok(json!({
        "id": pr_id.to_string(),
        "number": number.0,
        "title": params.title.trim(),
        "source_branch": params.source_branch,
        "target_branch": target,
        "state": "open",
        "head_oid": head_oid,
        "base_oid": base_oid,
    }))
}

#[allow(clippy::too_many_arguments)]
pub async fn merge_pull_request_by_id(
    state: &Arc<AppState>,
    repo_id: Uuid,
    storage_key: Uuid,
    principal_id: Uuid,
    pr_id: Uuid,
    expected_head_oid: &str,
    expected_base_oid: &str,
    expected_candidate_oid: &str,
) -> Result<Value, AppError> {
    crate::security::validate_git_oid(expected_head_oid).map_err(AppError::Validation)?;
    crate::security::validate_git_oid(expected_base_oid).map_err(AppError::Validation)?;
    crate::security::validate_git_oid(expected_candidate_oid).map_err(AppError::Validation)?;
    if expected_head_oid != expected_candidate_oid {
        return Err(AppError::Validation(
            "expected_candidate_oid must match expected_head_oid".to_string(),
        ));
    }
    let pr = sqlx::query_as::<
        _,
        (
            Uuid,
            Uuid,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
        ),
    >(
        "SELECT id, author_id, state, source_branch, target_branch, head_oid, base_oid
         FROM pull_requests WHERE id = $1 AND repo_id = $2",
    )
    .bind(pr_id)
    .bind(repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((id, author_id, pr_state, source_branch, target_branch, stored_head, base_oid)) = pr
    else {
        return Err(AppError::NotFound);
    };
    if pr_state != "open" {
        return Err(AppError::Validation("pull request is not open".to_string()));
    }
    let stored_head =
        stored_head.ok_or_else(|| AppError::Validation("missing head_oid".to_string()))?;
    if stored_head != expected_head_oid {
        return Err(AppError::Validation(
            "expected_head_oid does not match pull request record".to_string(),
        ));
    }
    let base_oid = base_oid.ok_or_else(|| AppError::Validation("missing base_oid".to_string()))?;
    if base_oid != expected_base_oid {
        return Err(AppError::Validation(
            "expected_base_oid does not match pull request record".to_string(),
        ));
    }
    let bare = bare_repo_path(&state.git_root, &storage_key);
    let live_head = git_rev_parse(&bare, &format!("refs/heads/{source_branch}"))?;
    if live_head != expected_head_oid {
        return Err(AppError::Validation(
            "source branch advanced; sync pull request before merge".to_string(),
        ));
    }
    let current_target = git_rev_parse(&bare, &format!("refs/heads/{target_branch}"))?;
    if current_target != expected_base_oid {
        return Err(AppError::Validation(
            "target branch advanced since pull request opened (CAS)".to_string(),
        ));
    }
    merge_preconditions(&state.pool, id, author_id, &live_head).await?;
    let merged_oid = merge_with_cas(&bare, &target_branch, expected_base_oid, &live_head)?;
    sqlx::query(
        "UPDATE pull_requests SET state = 'merged', merged_oid = $2, merged_by = $3,
         merged_at = CURRENT_TIMESTAMP, head_oid = $2, updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND state = 'open'",
    )
    .bind(id)
    .bind(&merged_oid)
    .bind(principal_id)
    .execute(&state.pool)
    .await?;
    let number: (i64,) = sqlx::query_as("SELECT number FROM pull_requests WHERE id = $1")
        .bind(id)
        .fetch_one(&state.pool)
        .await?;
    let title: (String,) = sqlx::query_as("SELECT title FROM pull_requests WHERE id = $1")
        .bind(id)
        .fetch_one(&state.pool)
        .await?;
    Ok(json!({
        "id": id.to_string(),
        "number": number.0,
        "title": title.0,
        "state": "merged",
        "source_branch": source_branch,
        "target_branch": target_branch,
        "head_oid": merged_oid,
        "base_oid": expected_base_oid,
        "merge_commit_oid": merged_oid,
    }))
}

pub async fn list_pull_requests(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo)): AxumPath<(String, String)>,
) -> Result<Json<Vec<PullRequestView>>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (_, repo_id, _) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            i64,
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<Uuid>,
        ),
    >(
        "SELECT id, number, title, source_branch, target_branch, state, head_oid, base_oid, issue_id
         FROM pull_requests WHERE repo_id = $1 ORDER BY number",
    )
    .bind(repo_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(
                |(
                    id,
                    number,
                    title,
                    source_branch,
                    target_branch,
                    state,
                    head_oid,
                    base_oid,
                    issue_id,
                )| {
                    PullRequestView {
                        id,
                        number,
                        title,
                        source_branch,
                        target_branch,
                        state,
                        head_oid,
                        base_oid,
                        issue_id,
                    }
                },
            )
            .collect(),
    ))
}

#[derive(Deserialize)]
pub struct SyncPullRequestBody {
    pub expected_row_version: i64,
}

pub async fn sync_pull_request(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, number)): AxumPath<(String, String, i64)>,
    Json(body): Json<SyncPullRequestBody>,
) -> Result<Json<PullRequestView>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id, storage_key) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let pr = sqlx::query_as::<_, (Uuid, String, String, String, Option<String>, i64)>(
        "SELECT id, source_branch, target_branch, state, head_oid, row_version
         FROM pull_requests WHERE repo_id = $1 AND number = $2",
    )
    .bind(repo_id)
    .bind(number)
    .fetch_optional(&state.pool)
    .await?;
    let Some((pr_id, source_branch, target_branch, pr_state, _old_head, _row_version)) = pr else {
        return Err(AppError::NotFound);
    };
    if pr_state != "open" {
        return Err(AppError::Validation("pull request is not open".to_string()));
    }
    let mut tx = state.pool.begin().await?;
    let locked = sqlx::query_as::<_, (i64,)>(
        "SELECT row_version FROM pull_requests WHERE id = $1 FOR UPDATE",
    )
    .bind(pr_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((locked_row_version,)) = locked else {
        return Err(AppError::NotFound);
    };
    if locked_row_version != body.expected_row_version {
        return Err(AppError::Validation(
            "stale pull request revision".to_string(),
        ));
    }
    let bare = bare_repo_path(&state.git_root, &storage_key);
    let head_oid = git_rev_parse(&bare, &format!("refs/heads/{source_branch}"))?;
    let base_oid = git_rev_parse(&bare, &format!("refs/heads/{target_branch}"))?;
    let new_row: Option<(i64,)> = sqlx::query_as(
        "UPDATE pull_requests SET head_oid = $2, base_oid = $3, row_version = row_version + 1,
         updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND row_version = $4
         RETURNING row_version",
    )
    .bind(pr_id)
    .bind(&head_oid)
    .bind(&base_oid)
    .bind(body.expected_row_version)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((new_row_version,)) = new_row else {
        return Err(AppError::Validation(
            "stale pull request revision".to_string(),
        ));
    };
    crate::discussion::mark_pr_comments_outdated(&mut *tx, pr_id, new_row_version).await?;
    tx.commit().await?;
    insert_pr_checks(&state.pool, org_id, repo_id, pr_id, &head_oid).await?;
    let meta = sqlx::query_as::<_, (String, Option<Uuid>)>(
        "SELECT title, issue_id FROM pull_requests WHERE id = $1",
    )
    .bind(pr_id)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(PullRequestView {
        id: pr_id,
        number,
        title: meta.0,
        source_branch,
        target_branch,
        state: "open".to_string(),
        head_oid: Some(head_oid),
        base_oid: Some(base_oid),
        issue_id: meta.1,
    }))
}

pub async fn review_pull_request(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, number)): AxumPath<(String, String, i64)>,
    Json(body): Json<ReviewPullRequestRequest>,
) -> Result<StatusCode, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id, storage_key) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let decision = body.decision.to_lowercase();
    if !matches!(decision.as_str(), "approve" | "request_changes" | "comment") {
        return Err(AppError::Validation("invalid review decision".to_string()));
    }
    let pr = sqlx::query_as::<_, (Uuid, Uuid, String, String)>(
        "SELECT id, author_id, state, source_branch FROM pull_requests WHERE repo_id = $1 AND number = $2",
    )
    .bind(repo_id)
    .bind(number)
    .fetch_optional(&state.pool)
    .await?;
    let Some((pr_id, author_id, pr_state, source_branch)) = pr else {
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
    let bare = bare_repo_path(&state.git_root, &storage_key);
    let head_oid = git_rev_parse(&bare, &format!("refs/heads/{source_branch}"))?;
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
    .bind(org_id)
    .bind(repo_id)
    .bind(pr_id)
    .bind(principal_id)
    .bind(&decision)
    .bind(body.body.unwrap_or_default())
    .bind(approved_head)
    .execute(&state.pool)
    .await?;
    Ok(StatusCode::CREATED)
}

pub async fn set_pull_request_check(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, number, check_name)): AxumPath<(String, String, i64, String)>,
    Json(body): Json<SetCheckRequest>,
) -> Result<StatusCode, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let org_id = super::org_id_for_member(&state.pool, &org, principal_id).await?;
    let row = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT r.id, r.storage_key FROM repositories r
         WHERE r.org_id = $1 AND r.slug_normalized = $2",
    )
    .bind(org_id)
    .bind(repo.to_lowercase())
    .fetch_optional(&state.pool)
    .await?;
    let Some((repo_id, storage_key)) = row else {
        return Err(AppError::NotFound);
    };
    let state_name = body.state.to_lowercase();
    if !matches!(state_name.as_str(), "pending" | "success" | "failure") {
        return Err(AppError::Validation("invalid check state".to_string()));
    }
    if matches!(state_name.as_str(), "success" | "failure") && body.attempt_id.is_none() {
        return Err(AppError::Validation(
            "attempt_id required for success or failure".to_string(),
        ));
    }
    let pr = sqlx::query_as::<_, (Uuid, Uuid, String, String)>(
        "SELECT id, author_id, source_branch, state FROM pull_requests WHERE repo_id = $1 AND number = $2",
    )
    .bind(repo_id)
    .bind(number)
    .fetch_optional(&state.pool)
    .await?;
    let Some((pr_id, author_id, source_branch, pr_state)) = pr else {
        return Err(AppError::NotFound);
    };
    if pr_state != "open" {
        return Err(AppError::Validation("pull request is not open".to_string()));
    }
    require_check_reporter(&state.pool, org_id, principal_id, author_id).await?;
    let bare = bare_repo_path(&state.git_root, &storage_key);
    let head_oid = git_rev_parse(&bare, &format!("refs/heads/{source_branch}"))?;
    let attempt_id = body.attempt_id.unwrap_or_else(Uuid::new_v4);
    let updated = sqlx::query(
        "UPDATE pr_check_runs SET state = $4, reported_by = $5, attempt_id = $6,
         expires_at = CURRENT_TIMESTAMP + INTERVAL '7 days', updated_at = CURRENT_TIMESTAMP
         WHERE pr_id = $1 AND check_name = $2 AND head_oid = $3",
    )
    .bind(pr_id)
    .bind(&check_name)
    .bind(&head_oid)
    .bind(&state_name)
    .bind(principal_id)
    .bind(attempt_id)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::Validation(
            "check run missing for current head (sync PR first)".to_string(),
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct RunnerCheckReport {
    pub pr_number: i64,
    pub check_name: String,
    pub head_oid: String,
    pub state: String,
    pub attempt_id: Uuid,
    pub pipeline_run_id: Uuid,
}

fn bearer_token(headers: &axum::http::HeaderMap) -> Result<String, AppError> {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let Some(token) = auth.strip_prefix("Bearer ") else {
        return Err(AppError::Unauthorized);
    };
    Ok(token.trim().to_string())
}

pub async fn runner_report_check(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo)): AxumPath<(String, String)>,
    Json(body): Json<RunnerCheckReport>,
) -> Result<StatusCode, AppError> {
    let token = bearer_token(&headers)?;
    let pat_ctx = super::pat_auth::authenticate_pat_context(&state.pool, &token).await?;
    if !super::pat_auth::scope_allows(&pat_ctx.scopes, "checks:write") {
        return Err(AppError::Forbidden);
    }
    if !super::pat_policy::is_runner_only_pat(&pat_ctx.scopes) {
        return Err(AppError::Forbidden);
    }
    let org_id = super::org_id_for_member(&state.pool, &org, pat_ctx.principal_id).await?;
    if super::pat_policy::principal_is_privileged_org_member(
        &state.pool,
        org_id,
        pat_ctx.principal_id,
    )
    .await?
    {
        return Err(AppError::Forbidden);
    }
    if !super::pat_auth::pat_org_allowed(pat_ctx.org_id, org_id) {
        return Err(AppError::Forbidden);
    }
    let row = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT r.id, r.storage_key FROM repositories r
         WHERE r.org_id = $1 AND r.slug_normalized = $2",
    )
    .bind(org_id)
    .bind(repo.to_lowercase())
    .fetch_optional(&state.pool)
    .await?;
    let Some((repo_id, storage_key)) = row else {
        return Err(AppError::NotFound);
    };
    let pipe = sqlx::query_as::<_, (String, Uuid, String, Uuid, String)>(
        "SELECT state, run_attempt_id, source_oid, org_id, run_kind FROM pipeline_runs
         WHERE id = $1 AND repo_id = $2",
    )
    .bind(body.pipeline_run_id)
    .bind(repo_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((pipe_state, run_attempt, pipe_source, pipe_org, run_kind)) = pipe else {
        return Err(AppError::NotFound);
    };
    if run_kind != "pr_validation" {
        return Err(AppError::Validation(
            "check reports must reference a pr_validation pipeline run (not release_build)"
                .to_string(),
        ));
    }
    if pipe_org != org_id {
        return Err(AppError::Forbidden);
    }
    if pipe_source != body.head_oid {
        return Err(AppError::Validation(
            "head_oid must match pipeline source_oid".to_string(),
        ));
    }
    if pipe_state != "succeeded" {
        return Err(AppError::Validation(
            "pipeline must succeed before check report".to_string(),
        ));
    }
    if run_attempt != body.attempt_id {
        return Err(AppError::Validation(
            "attempt_id must match pipeline run_attempt_id".to_string(),
        ));
    }
    let state_name = body.state.to_lowercase();
    if !matches!(state_name.as_str(), "success" | "failure") {
        return Err(AppError::Validation("invalid check state".to_string()));
    }
    let pr = sqlx::query_as::<_, (Uuid, Uuid, String, String)>(
        "SELECT id, author_id, source_branch, state FROM pull_requests WHERE repo_id = $1 AND number = $2",
    )
    .bind(repo_id)
    .bind(body.pr_number)
    .fetch_optional(&state.pool)
    .await?;
    let Some((pr_id, author_id, source_branch, pr_state)) = pr else {
        return Err(AppError::NotFound);
    };
    if pr_state != "open" {
        return Err(AppError::Validation("pull request is not open".to_string()));
    }
    if pat_ctx.principal_id == author_id {
        return Err(AppError::Forbidden);
    }
    let bare = bare_repo_path(&state.git_root, &storage_key);
    let live_head = git_rev_parse(&bare, &format!("refs/heads/{source_branch}"))?;
    if live_head != body.head_oid {
        return Err(AppError::Validation(
            "head_oid does not match current source branch".to_string(),
        ));
    }
    let used: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM runner_check_receipts WHERE attempt_id = $1")
            .bind(body.attempt_id)
            .fetch_one(&state.pool)
            .await?;
    if used.0 > 0 {
        return Err(AppError::Validation(
            "attempt_id already consumed".to_string(),
        ));
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO runner_check_receipts (id, org_id, repo_id, pipeline_run_id, attempt_id,
         pr_id, check_name, head_oid, reported_by)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(repo_id)
    .bind(body.pipeline_run_id)
    .bind(body.attempt_id)
    .bind(pr_id)
    .bind(&body.check_name)
    .bind(&body.head_oid)
    .bind(pat_ctx.principal_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        if let sqlx::Error::Database(db) = &e {
            if db.constraint().is_some() {
                return AppError::Validation("attempt_id already consumed".to_string());
            }
        }
        AppError::Database(e)
    })?;
    let updated = sqlx::query(
        "UPDATE pr_check_runs SET state = $4, reported_by = $5, attempt_id = $6,
         expires_at = CURRENT_TIMESTAMP + INTERVAL '7 days', updated_at = CURRENT_TIMESTAMP
         WHERE pr_id = $1 AND check_name = $2 AND head_oid = $3",
    )
    .bind(pr_id)
    .bind(&body.check_name)
    .bind(&body.head_oid)
    .bind(&state_name)
    .bind(pat_ctx.principal_id)
    .bind(body.attempt_id)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        tx.rollback().await.ok();
        return Err(AppError::Validation(
            "check run missing for current head (sync PR first)".to_string(),
        ));
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn merge_preconditions(
    pool: &sqlx::PgPool,
    pr_id: Uuid,
    author_id: Uuid,
    head_oid: &str,
) -> Result<(), AppError> {
    let approvals: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM pr_reviews
         WHERE pr_id = $1 AND decision = 'approve' AND reviewer_id <> $2
           AND approved_head_oid = $3",
    )
    .bind(pr_id)
    .bind(author_id)
    .bind(head_oid)
    .fetch_one(pool)
    .await?;
    if approvals.0 == 0 {
        return Err(AppError::Validation(
            "merge requires current approval from a non-author reviewer".to_string(),
        ));
    }
    let required: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM repo_required_checks r
         JOIN pull_requests p ON p.id = $1
         WHERE r.repo_id = p.repo_id",
    )
    .bind(pr_id)
    .fetch_one(pool)
    .await?;
    if required.0 > 0 {
        let passing: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM repo_required_checks r
             JOIN pull_requests p ON p.id = $1
             JOIN pr_check_runs c ON c.pr_id = p.id AND c.check_name = r.check_name
             WHERE c.head_oid = $2 AND c.state = 'success'
               AND c.reported_by IS NOT NULL AND c.reported_by <> p.author_id
               AND (c.expires_at IS NULL OR c.expires_at > CURRENT_TIMESTAMP)",
        )
        .bind(pr_id)
        .bind(head_oid)
        .fetch_one(pool)
        .await?;
        if passing.0 < required.0 {
            return Err(AppError::Validation(
                "merge blocked: required checks missing or not passing".to_string(),
            ));
        }
        let failing: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM pr_check_runs
             WHERE pr_id = $1 AND head_oid = $2 AND state = 'failure'",
        )
        .bind(pr_id)
        .bind(head_oid)
        .fetch_one(pool)
        .await?;
        if failing.0 > 0 {
            return Err(AppError::Validation(
                "merge blocked: failing check on current head".to_string(),
            ));
        }
    }
    Ok(())
}

pub async fn merge_pull_request(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, number)): AxumPath<(String, String, i64)>,
) -> Result<Json<PullRequestView>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (_, repo_id, storage_key) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let pr = sqlx::query_as::<
        _,
        (
            Uuid,
            Uuid,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
        ),
    >(
        "SELECT id, author_id, state, source_branch, target_branch, head_oid, base_oid
         FROM pull_requests WHERE repo_id = $1 AND number = $2",
    )
    .bind(repo_id)
    .bind(number)
    .fetch_optional(&state.pool)
    .await?;
    let Some((pr_id, author_id, pr_state, source_branch, target_branch, stored_head, base_oid)) =
        pr
    else {
        return Err(AppError::NotFound);
    };
    if pr_state != "open" {
        return Err(AppError::Validation("pull request is not open".to_string()));
    }
    let bare = bare_repo_path(&state.git_root, &storage_key);
    let live_head = git_rev_parse(&bare, &format!("refs/heads/{source_branch}"))?;
    let stored_head =
        stored_head.ok_or_else(|| AppError::Validation("missing head_oid".to_string()))?;
    if live_head != stored_head {
        return Err(AppError::Validation(
            "source branch advanced; sync pull request before merge".to_string(),
        ));
    }
    let base_oid = base_oid.ok_or_else(|| AppError::Validation("missing base_oid".to_string()))?;
    let current_target = git_rev_parse(&bare, &format!("refs/heads/{target_branch}"))?;
    if current_target != base_oid {
        return Err(AppError::Validation(
            "target branch advanced since pull request opened (CAS)".to_string(),
        ));
    }
    merge_preconditions(&state.pool, pr_id, author_id, &live_head).await?;
    let merged_oid = merge_with_cas(&bare, &target_branch, &base_oid, &live_head)?;
    sqlx::query(
        "UPDATE pull_requests SET state = 'merged', merged_oid = $2, merged_by = $3,
         merged_at = CURRENT_TIMESTAMP, head_oid = $2, updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND state = 'open'",
    )
    .bind(pr_id)
    .bind(&merged_oid)
    .bind(principal_id)
    .execute(&state.pool)
    .await?;
    let meta = sqlx::query_as::<_, (String, Option<Uuid>)>(
        "SELECT title, issue_id FROM pull_requests WHERE id = $1",
    )
    .bind(pr_id)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(PullRequestView {
        id: pr_id,
        number,
        title: meta.0,
        source_branch,
        target_branch,
        state: "merged".to_string(),
        head_oid: Some(merged_oid.clone()),
        base_oid: Some(base_oid),
        issue_id: meta.1,
    }))
}

fn merge_with_cas(
    bare: &Path,
    target_branch: &str,
    expected_target_oid: &str,
    source_oid: &str,
) -> Result<String, AppError> {
    let temp = tempfile::tempdir().map_err(|error| AppError::Other(error.to_string()))?;
    let work = temp.path();
    let clone = Command::new("git")
        .args([
            "clone",
            bare.to_str().expect("bare"),
            work.to_str().expect("work"),
        ])
        .status()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if !clone.success() {
        return Err(AppError::Other("git clone for merge failed".to_string()));
    }
    let checkout = Command::new("git")
        .current_dir(work)
        .args(["checkout", expected_target_oid])
        .status()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if !checkout.success() {
        return Err(AppError::Other("checkout target oid failed".to_string()));
    }
    let merge = Command::new("git")
        .current_dir(work)
        .args([
            "-c",
            "user.email=git@musehub.local",
            "-c",
            "user.name=musehub",
            "merge",
            "--no-ff",
            source_oid,
            "-m",
            &format!("Merge {source_oid} into {target_branch}"),
            "--no-gpg-sign",
        ])
        .status()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if !merge.success() {
        return Err(AppError::Validation(
            "merge conflict or merge failed".to_string(),
        ));
    }
    let now = git_rev_parse(bare, &format!("refs/heads/{target_branch}"))?;
    if now != expected_target_oid {
        return Err(AppError::Validation(format!(
            "target CAS precheck failed (expected {expected_target_oid}, found {now})"
        )));
    }
    let head_out = Command::new("git")
        .current_dir(work)
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if !head_out.status.success() {
        return Err(AppError::Other(
            "rev-parse HEAD after merge failed".to_string(),
        ));
    }
    let merged_oid = String::from_utf8_lossy(&head_out.stdout).trim().to_string();
    if !bare.join("hooks/pre-receive").is_file() {
        return Err(AppError::Other(
            "pre-receive hook missing on bare repository".to_string(),
        ));
    }
    let staging_ref = format!("refs/musehub/merge-staging/{}", Uuid::new_v4().simple());
    let fetch = Command::new("git")
        .args([
            "--git-dir",
            bare.to_str().expect("bare"),
            "fetch",
            "-q",
            work.to_str().expect("work"),
            &format!("+HEAD:{staging_ref}"),
        ])
        .output()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if !fetch.status.success() {
        return Err(AppError::Other(format!(
            "merge object fetch failed: {}",
            String::from_utf8_lossy(&fetch.stderr)
        )));
    }
    let cas = Command::new("git")
        .args([
            "--git-dir",
            bare.to_str().expect("bare"),
            "update-ref",
            &format!("refs/heads/{target_branch}"),
            &merged_oid,
            expected_target_oid,
        ])
        .output()
        .map_err(|error| AppError::Other(error.to_string()))?;
    let _ = Command::new("git").args([
        "--git-dir",
        bare.to_str().expect("bare"),
        "update-ref",
        "-d",
        &staging_ref,
    ]);
    if !cas.status.success() {
        return Err(AppError::Validation(format!(
            "merge CAS update-ref failed: {}",
            String::from_utf8_lossy(&cas.stderr)
        )));
    }
    let new_tip = git_rev_parse(bare, &format!("refs/heads/{target_branch}"))?;
    if new_tip != merged_oid {
        return Err(AppError::Validation(
            "merge CAS result mismatch after update-ref".to_string(),
        ));
    }
    let parent_output = Command::new("git")
        .args([
            "--git-dir",
            bare.to_str().expect("bare"),
            "rev-parse",
            &format!("refs/heads/{target_branch}^1"),
        ])
        .output()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if parent_output.status.success() {
        let parent = String::from_utf8_lossy(&parent_output.stdout)
            .trim()
            .to_string();
        if parent != expected_target_oid {
            return Err(AppError::Validation(
                "merge result failed CAS parent check on target".to_string(),
            ));
        }
    }
    Ok(new_tip)
}

#[derive(Serialize)]
pub struct PullRequestDetail {
    #[serde(flatten)]
    pub view: PullRequestView,
    pub row_version: i64,
    pub author_handle: String,
    pub base_oid: String,
    pub head_oid: String,
}

pub async fn get_pull_request(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, number)): AxumPath<(String, String, i64)>,
) -> Result<Json<PullRequestDetail>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let repo_id = super::repo_id_for_member(&state.pool, &org, &repo, principal_id).await?;
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            i64,
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<Uuid>,
            i64,
            Uuid,
        ),
    >(
        "SELECT pr.id, pr.number, pr.title, pr.source_branch, pr.target_branch, pr.state,
                pr.head_oid, pr.base_oid, pr.issue_id, pr.row_version, pr.author_id
         FROM pull_requests pr WHERE pr.repo_id = $1 AND pr.number = $2",
    )
    .bind(repo_id)
    .bind(number)
    .fetch_optional(&state.pool)
    .await?;
    let Some((
        id,
        num,
        title,
        source_branch,
        target_branch,
        pr_state,
        head_oid,
        base_oid,
        issue_id,
        row_version,
        author_id,
    )) = row
    else {
        return Err(AppError::NotFound);
    };
    let author: (String,) = sqlx::query_as("SELECT handle FROM principals WHERE id = $1")
        .bind(author_id)
        .fetch_one(&state.pool)
        .await?;
    let head = head_oid.unwrap_or_default();
    let base = base_oid.unwrap_or_default();
    Ok(Json(PullRequestDetail {
        view: PullRequestView {
            id,
            number: num,
            title,
            source_branch,
            target_branch,
            state: pr_state,
            head_oid: Some(head.clone()),
            base_oid: Some(base.clone()),
            issue_id,
        },
        row_version,
        author_handle: author.0,
        base_oid: base,
        head_oid: head,
    }))
}

pub async fn get_pull_request_diff(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, number)): AxumPath<(String, String, i64)>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (_, _, storage_key) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let repo_id = super::repo_id_for_member(&state.pool, &org, &repo, principal_id).await?;
    let row = sqlx::query_as::<_, (Option<String>, Option<String>)>(
        "SELECT base_oid, head_oid FROM pull_requests WHERE repo_id = $1 AND number = $2",
    )
    .bind(repo_id)
    .bind(number)
    .fetch_optional(&state.pool)
    .await?;
    let Some((base_oid, head_oid)) = row else {
        return Err(AppError::NotFound);
    };
    let base = base_oid.ok_or(AppError::Validation("missing base_oid".into()))?;
    let head = head_oid.ok_or(AppError::Validation("missing head_oid".into()))?;
    let bare = bare_repo_path(&state.git_root, &storage_key);
    let path = params.get("path").map(|s| s.as_str());
    if let Some(p) = path {
        let diff = crate::mcp::git_read::git_unified_diff(&bare, &base, &head, p, 128 * 1024)?;
        return Ok(Json(diff));
    }
    let (items, summary, has_more) =
        crate::mcp::git_read::git_commit_compare(&bare, &base, &head, None, 200)?;
    Ok(Json(json!({
        "base_oid": base,
        "head_oid": head,
        "summary": summary,
        "files": items,
        "has_more": has_more,
    })))
}
