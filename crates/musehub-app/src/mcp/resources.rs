//! MCP Resources (`docs/04-mcp-api.md` §13) — URI is not auth; every read re-checks PAT + repo scope.

use rmcp::model::{
    ListResourcesResult, ReadResourceResponse, ReadResourceResult, Resource, ResourceContents,
};
use rmcp::ErrorData;
use uuid::Uuid;

use crate::error::AppError;
use crate::mcp::auth::{active_pat, resolve_repo};
use crate::mcp::capabilities::V1_RESOURCE_URI_TEMPLATES;
use crate::AppState;

const DEPLOYMENT_URI_PREFIX: &str = "musehub://deployments/";
const AGENT_RUN_URI_PREFIX: &str = "musehub://agent-runs/";
const ISSUE_REV_PREFIX: &str = "musehub://issues/";
const PR_REV_PREFIX: &str = "musehub://pull-requests/";
const REPO_FILE_PREFIX: &str = "musehub://repositories/";

pub fn list_static_resources() -> ListResourcesResult {
    let templates = V1_RESOURCE_URI_TEMPLATES
        .iter()
        .map(|uri| {
            let name = uri
                .trim_start_matches("musehub://")
                .split('/')
                .next()
                .unwrap_or("resource");
            Resource::new(*uri, format!("MuseHub {name} (V1 template)"))
                .with_mime_type("application/json")
        })
        .collect();
    ListResourcesResult::with_all_items(templates)
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

pub async fn read_resource(state: &AppState, uri: &str) -> Result<ReadResourceResponse, ErrorData> {
    let pat = active_pat(state)
        .await
        .map_err(|_| ErrorData::invalid_request("unauthorized", None))?;
    if let Some(id_str) = uri.strip_prefix(DEPLOYMENT_URI_PREFIX) {
        let deployment_id = Uuid::parse_str(id_str)
            .map_err(|_| ErrorData::invalid_params("invalid deployment resource uri", None))?;
        let row = sqlx::query_as::<_, (Uuid, Uuid, String, Option<serde_json::Value>)>(
            "SELECT org_id, repo_id, state, health_summary FROM deployments WHERE id = $1",
        )
        .bind(deployment_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let Some((org_id, repo_id, dep_state, health)) = row else {
            return Err(ErrorData::invalid_params("not found", None));
        };
        resolve_repo(state, &pat, org_id, repo_id)
            .await
            .map_err(map_access_err)?;
        let body = serde_json::json!({
            "id": deployment_id.to_string(),
            "state": dep_state,
            "health_summary": health,
        });
        return Ok(ReadResourceResult::new(vec![ResourceContents::text(
            body.to_string(),
            uri.to_string(),
        )])
        .into());
    }
    if let Some(rest) = uri.strip_prefix(AGENT_RUN_URI_PREFIX) {
        let (id_part, _) = rest
            .split_once('/')
            .ok_or_else(|| ErrorData::invalid_params("invalid agent-run resource uri", None))?;
        if !uri.ends_with("/summary") {
            return Err(ErrorData::invalid_params("unsupported agent-run uri", None));
        }
        let run_id = Uuid::parse_str(id_part)
            .map_err(|_| ErrorData::invalid_params("invalid agent-run resource uri", None))?;
        let row = sqlx::query_as::<_, (Uuid, Uuid, String, Option<String>, Option<i64>)>(
            "SELECT org_id, repo_id, state, work_branch, budget_tokens_limit
             FROM agent_runs WHERE id = $1",
        )
        .bind(run_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let Some((org_id, repo_id, state_name, work_branch, budget)) = row else {
            return Err(ErrorData::invalid_params("not found", None));
        };
        resolve_repo(state, &pat, org_id, repo_id)
            .await
            .map_err(map_access_err)?;
        let body = serde_json::json!({
            "id": run_id.to_string(),
            "state": state_name,
            "work_branch": work_branch,
            "budget_tokens_limit": budget,
        });
        return Ok(ReadResourceResult::new(vec![ResourceContents::text(
            body.to_string(),
            uri.to_string(),
        )])
        .into());
    }
    if let Some(rest) = uri.strip_prefix(ISSUE_REV_PREFIX) {
        if let Some((issue_str, rev_str)) = rest.split_once("/revisions/") {
            let issue_id = Uuid::parse_str(issue_str)
                .map_err(|_| ErrorData::invalid_params("invalid issue resource uri", None))?;
            let revision: i32 = rev_str
                .parse()
                .map_err(|_| ErrorData::invalid_params("invalid issue revision", None))?;
            return super::resource_reads::read_issue_revision(state, issue_id, revision, uri).await;
        }
    }
    if let Some(rest) = uri.strip_prefix(PR_REV_PREFIX) {
        if let Some((pr_str, rev_str)) = rest.split_once("/revisions/") {
            let pr_id = Uuid::parse_str(pr_str)
                .map_err(|_| ErrorData::invalid_params("invalid pr resource uri", None))?;
            let revision: i64 = rev_str
                .parse()
                .map_err(|_| ErrorData::invalid_params("invalid pr revision", None))?;
            return super::resource_reads::read_pr_revision(state, pr_id, revision, uri).await;
        }
    }
    if let Some(rest) = uri.strip_prefix(REPO_FILE_PREFIX) {
        if let Some(after_repo) = rest.split_once("/commits/") {
            let (repo_str, tail) = after_repo;
            let repo_id = Uuid::parse_str(repo_str)
                .map_err(|_| ErrorData::invalid_params("invalid repository id", None))?;
            if let Some((oid, enc_path)) = tail.split_once("/files/") {
                return super::resource_reads::read_repo_commit_file(
                    state, repo_id, oid, enc_path, uri,
                )
                .await;
            }
        }
    }
    Err(ErrorData::invalid_params("unsupported resource uri", None))
}
