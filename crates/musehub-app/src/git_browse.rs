//! REST git browse (GIT-03) at pinned commit.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use serde_json::Value;

use crate::error::AppError;
use crate::git_http::bare_repo_path;
use crate::mcp::git_read::{
    git_commit_compare, git_log_commits, git_read_file_at, git_tree_entries, git_unified_diff,
};
use crate::AppState;

#[derive(Deserialize)]
pub struct CommitListQuery {
    pub r#ref: String,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    30
}

#[derive(Deserialize)]
pub struct TreeQuery {
    pub commit: String,
    #[serde(default)]
    pub path: String,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

#[derive(Deserialize)]
pub struct BlobQuery {
    pub commit: String,
    pub path: String,
    #[serde(default = "default_start")]
    pub start_line: u32,
    #[serde(default = "default_max_lines")]
    pub max_lines: u32,
}

fn default_start() -> u32 {
    1
}
fn default_max_lines() -> u32 {
    200
}

#[derive(Deserialize)]
pub struct DiffQuery {
    pub base: String,
    pub head: String,
    pub path: String,
}

async fn bare_for_repo(
    state: &AppState,
    org: &str,
    repo: &str,
    principal_id: uuid::Uuid,
) -> Result<std::path::PathBuf, AppError> {
    let (_, _, storage_key) =
        super::repo_storage_for_member(&state.pool, org, repo, principal_id).await?;
    Ok(bare_repo_path(&state.git_root, &storage_key))
}

pub async fn list_commits(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo)): Path<(String, String)>,
    Query(q): Query<CommitListQuery>,
) -> Result<Json<Value>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let bare = bare_for_repo(&state, &org, &repo, principal_id).await?;
    let oid = if crate::security::validate_git_oid(&q.r#ref).is_ok() {
        q.r#ref.clone()
    } else {
        crate::pr::git_rev_parse_public(&bare, &format!("refs/heads/{}", q.r#ref))?
    };
    let (items, has_more) = git_log_commits(&bare, &oid, q.limit.clamp(1, 100))?;
    Ok(Json(serde_json::json!({
        "ref": q.r#ref,
        "commit_oid": oid,
        "items": items,
        "has_more": has_more,
    })))
}

pub async fn list_tree(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo)): Path<(String, String)>,
    Query(q): Query<TreeQuery>,
) -> Result<Json<Value>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let bare = bare_for_repo(&state, &org, &repo, principal_id).await?;
    let (items, has_more) =
        git_tree_entries(&bare, &q.commit, &q.path, q.limit.clamp(1, 500))?;
    Ok(Json(serde_json::json!({
        "commit": q.commit,
        "path": q.path,
        "items": items,
        "has_more": has_more,
    })))
}

pub async fn read_blob(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo)): Path<(String, String)>,
    Query(q): Query<BlobQuery>,
) -> Result<Json<Value>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let bare = bare_for_repo(&state, &org, &repo, principal_id).await?;
    let body = git_read_file_at(&bare, &q.commit, &q.path, q.start_line, q.max_lines)?;
    Ok(Json(body))
}

pub async fn compare_commits(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org, repo)): Path<(String, String)>,
    Query(q): Query<DiffQuery>,
) -> Result<Json<Value>, AppError> {
    let principal_id = super::require_session(&state.pool, &headers).await?;
    let bare = bare_for_repo(&state, &org, &repo, principal_id).await?;
    if q.path.is_empty() {
        let (items, summary, has_more) =
            git_commit_compare(&bare, &q.base, &q.head, None, 200)?;
        return Ok(Json(serde_json::json!({
            "base": q.base,
            "head": q.head,
            "summary": summary,
            "files": items,
            "has_more": has_more,
        })));
    }
    let diff = git_unified_diff(&bare, &q.base, &q.head, &q.path, 128 * 1024)?;
    Ok(Json(diff))
}
