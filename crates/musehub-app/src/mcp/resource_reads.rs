//! V1 MCP resource read implementations (DB + git).

use rmcp::model::{ReadResourceResponse, ReadResourceResult, ResourceContents};
use rmcp::ErrorData;
use urlencoding::decode;
use uuid::Uuid;

use crate::error::AppError;
use crate::git_http::bare_repo_path;
use crate::mcp::auth::{active_pat, resolve_repo};
use crate::AppState;

pub async fn read_issue_revision(
    state: &AppState,
    issue_id: Uuid,
    revision: i32,
    uri: &str,
) -> Result<ReadResourceResponse, ErrorData> {
    let pat = active_pat(state)
        .await
        .map_err(|_| ErrorData::invalid_request("unauthorized", None))?;
    let row = sqlx::query_as::<_, (Uuid, Uuid, String, String, Uuid)>(
        "SELECT org_id, repo_id, title, body, author_id FROM issue_revisions
         WHERE issue_id = $1 AND revision = $2",
    )
    .bind(issue_id)
    .bind(revision)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    let Some((org_id, repo_id, title, body, author_id)) = row else {
        return Err(ErrorData::invalid_params("not found", None));
    };
    resolve_repo(state, &pat, org_id, repo_id)
        .await
        .map_err(map_access_err)?;
    let body_json = serde_json::json!({
        "issue_id": issue_id.to_string(),
        "revision": revision,
        "title": title,
        "body": body,
        "author_id": author_id.to_string(),
    });
    Ok(ReadResourceResult::new(vec![ResourceContents::text(
        body_json.to_string(),
        uri.to_string(),
    )])
    .into())
}

pub async fn read_pr_revision(
    state: &AppState,
    pr_id: Uuid,
    revision: i64,
    uri: &str,
) -> Result<ReadResourceResponse, ErrorData> {
    let pat = active_pat(state)
        .await
        .map_err(|_| ErrorData::invalid_request("unauthorized", None))?;
    let row = sqlx::query_as::<_, (Uuid, Uuid, String, String, String, Option<String>, i64)>(
        "SELECT org_id, repo_id, title, source_branch, target_branch, head_oid, row_version
         FROM pull_requests WHERE id = $1",
    )
    .bind(pr_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    let Some((org_id, repo_id, title, source_branch, target_branch, head_oid, row_version)) = row
    else {
        return Err(ErrorData::invalid_params("not found", None));
    };
    if row_version != revision {
        return Err(ErrorData::invalid_params("revision mismatch", None));
    }
    resolve_repo(state, &pat, org_id, repo_id)
        .await
        .map_err(map_access_err)?;
    let body_json = serde_json::json!({
        "pr_id": pr_id.to_string(),
        "revision": row_version,
        "title": title,
        "source_branch": source_branch,
        "target_branch": target_branch,
        "head_oid": head_oid,
    });
    Ok(ReadResourceResult::new(vec![ResourceContents::text(
        body_json.to_string(),
        uri.to_string(),
    )])
    .into())
}

pub async fn read_repo_commit_file(
    state: &AppState,
    repo_id: Uuid,
    commit_oid: &str,
    encoded_path: &str,
    uri: &str,
) -> Result<ReadResourceResponse, ErrorData> {
    let pat = active_pat(state)
        .await
        .map_err(|_| ErrorData::invalid_request("unauthorized", None))?;
    if commit_oid.len() != 40 || !commit_oid.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ErrorData::invalid_params("invalid commit oid", None));
    }
    let path = decode(encoded_path)
        .map_err(|_| ErrorData::invalid_params("invalid encoded path", None))?
        .into_owned();
    if path.is_empty() || path.contains("..") || path.starts_with('/') {
        return Err(ErrorData::invalid_params("invalid path", None));
    }
    let row = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT org_id, storage_key FROM repositories WHERE id = $1",
    )
    .bind(repo_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    let Some((org_id, storage_key)) = row else {
        return Err(ErrorData::invalid_params("not found", None));
    };
    resolve_repo(state, &pat, org_id, repo_id)
        .await
        .map_err(map_access_err)?;
    let bare = bare_repo_path(&state.git_root, &storage_key);
    let spec = format!("{commit_oid}:{path}");
    let output = std::process::Command::new("git")
        .args([
            "--git-dir",
            bare.to_str().ok_or_else(|| ErrorData::internal_error("git path", None))?,
            "show",
            &spec,
        ])
        .output()
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    if !output.status.success() {
        return Err(ErrorData::invalid_params("file not found at commit", None));
    }
    let bytes = output.stdout;
    const MAX_BYTES: usize = 512_000;
    let truncated = bytes.len() > MAX_BYTES;
    let slice = if truncated { &bytes[..MAX_BYTES] } else { &bytes[..] };
    let content = String::from_utf8_lossy(slice).into_owned();
    let body_json = serde_json::json!({
        "repository_id": repo_id.to_string(),
        "commit_oid": commit_oid,
        "path": path,
        "truncated": truncated,
        "byte_length": bytes.len(),
        "content": content,
    });
    Ok(ReadResourceResult::new(vec![ResourceContents::text(
        body_json.to_string(),
        uri.to_string(),
    )])
    .into())
}

fn map_access_err(e: AppError) -> ErrorData {
    match e {
        AppError::Forbidden | AppError::Unauthorized => {
            ErrorData::invalid_request("forbidden", None)
        }
        AppError::NotFound => ErrorData::invalid_params("not found", None),
        other => ErrorData::internal_error(other.to_string(), None),
    }
}
