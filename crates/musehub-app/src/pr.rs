//! Pull request create / review / merge (minimal product gates).

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::git_http::bare_repo_path;
use crate::AppState;

#[derive(Serialize)]
pub struct PullRequestView {
    pub id: Uuid,
    pub number: i64,
    pub title: String,
    pub source_branch: String,
    pub target_branch: String,
    pub state: String,
    pub head_oid: Option<String>,
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
    if let Some(run_id) = body.agent_run_id {
        let ok = sqlx::query_as::<_, (Uuid,)>(
            "SELECT id FROM agent_runs WHERE id = $1 AND repo_id = $2",
        )
        .bind(run_id)
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
            issue_id, agent_run_id, head_oid, state
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'open')",
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
    .execute(&state.pool)
    .await?;
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
        }),
    ))
}

pub async fn list_pull_requests(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo)): AxumPath<(String, String)>,
) -> Result<Json<Vec<PullRequestView>>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (_, repo_id, _) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let rows = sqlx::query_as::<_, (Uuid, i64, String, String, String, String, Option<String>)>(
        "SELECT id, number, title, source_branch, target_branch, state, head_oid
         FROM pull_requests WHERE repo_id = $1 ORDER BY number",
    )
    .bind(repo_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(
                |(id, number, title, source_branch, target_branch, state, head_oid)| {
                    PullRequestView {
                        id,
                        number,
                        title,
                        source_branch,
                        target_branch,
                        state,
                        head_oid,
                    }
                },
            )
            .collect(),
    ))
}

pub async fn review_pull_request(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, number)): AxumPath<(String, String, i64)>,
    Json(body): Json<ReviewPullRequestRequest>,
) -> Result<StatusCode, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id, _) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let decision = body.decision.to_lowercase();
    if !matches!(decision.as_str(), "approve" | "request_changes" | "comment") {
        return Err(AppError::Validation("invalid review decision".to_string()));
    }
    let pr = sqlx::query_as::<_, (Uuid, Uuid, String)>(
        "SELECT id, author_id, state FROM pull_requests WHERE repo_id = $1 AND number = $2",
    )
    .bind(repo_id)
    .bind(number)
    .fetch_optional(&state.pool)
    .await?;
    let Some((pr_id, author_id, pr_state)) = pr else {
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
    let review_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pr_reviews (id, org_id, repo_id, pr_id, reviewer_id, decision, body)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(review_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(pr_id)
    .bind(principal_id)
    .bind(&decision)
    .bind(body.body.unwrap_or_default())
    .execute(&state.pool)
    .await?;
    Ok(StatusCode::CREATED)
}

pub async fn merge_pull_request(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((org, repo, number)): AxumPath<(String, String, i64)>,
) -> Result<Json<PullRequestView>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let (org_id, repo_id, storage_key) =
        super::repo_storage_for_member(&state.pool, &org, &repo, principal_id).await?;
    let pr = sqlx::query_as::<_, (Uuid, Uuid, String, String, String, Option<String>)>(
        "SELECT id, author_id, state, source_branch, target_branch, head_oid
         FROM pull_requests WHERE repo_id = $1 AND number = $2",
    )
    .bind(repo_id)
    .bind(number)
    .fetch_optional(&state.pool)
    .await?;
    let Some((pr_id, author_id, pr_state, source_branch, target_branch, head_oid)) = pr else {
        return Err(AppError::NotFound);
    };
    if pr_state != "open" {
        return Err(AppError::Validation("pull request is not open".to_string()));
    }
    let approvals: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM pr_reviews
         WHERE pr_id = $1 AND decision = 'approve' AND reviewer_id <> $2",
    )
    .bind(pr_id)
    .bind(author_id)
    .fetch_one(&state.pool)
    .await?;
    if approvals.0 == 0 {
        return Err(AppError::Validation(
            "merge requires approval from a non-author reviewer".to_string(),
        ));
    }
    let head = head_oid.ok_or_else(|| AppError::Validation("missing head_oid".to_string()))?;
    let bare = bare_repo_path(&state.git_root, &storage_key);
    let merge_base = git_rev_parse(&bare, &format!("refs/heads/{target_branch}"))?;
    let merged_oid = merge_onto_branch(&bare, &target_branch, &source_branch)?;
    sqlx::query(
        "UPDATE pull_requests SET state = 'merged', merged_oid = $2, merged_by = $3,
         merged_at = CURRENT_TIMESTAMP, head_oid = $4, updated_at = CURRENT_TIMESTAMP
         WHERE id = $1",
    )
    .bind(pr_id)
    .bind(&merged_oid)
    .bind(principal_id)
    .bind(&head)
    .execute(&state.pool)
    .await?;
    let _ = (org_id, merge_base);
    let title: (String,) = sqlx::query_as("SELECT title FROM pull_requests WHERE id = $1")
        .bind(pr_id)
        .fetch_one(&state.pool)
        .await?;
    Ok(Json(PullRequestView {
        id: pr_id,
        number,
        title: title.0,
        source_branch,
        target_branch,
        state: "merged".to_string(),
        head_oid: Some(merged_oid),
    }))
}

fn merge_onto_branch(bare: &Path, target: &str, source: &str) -> Result<String, AppError> {
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
        .args(["checkout", target])
        .status()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if !checkout.success() {
        return Err(AppError::Other("checkout target failed".to_string()));
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
            source,
            "-m",
            &format!("Merge pull request branch {source}"),
            "--no-gpg-sign",
        ])
        .status()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if !merge.success() {
        return Err(AppError::Other("git merge failed".to_string()));
    }
    let merged = git_rev_parse(work, "HEAD")?;
    let merge_base = git_rev_parse(bare, &format!("refs/heads/{target}"))?;
    let update = Command::new("git")
        .args([
            "--git-dir",
            bare.to_str().expect("bare"),
            "update-ref",
            &format!("refs/heads/{target}"),
            &merged,
            &merge_base,
        ])
        .status()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if !update.success() {
        return Err(AppError::Other(
            "update-ref after merge failed (concurrent change?)".to_string(),
        ));
    }
    Ok(merged)
}
