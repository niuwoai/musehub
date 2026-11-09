//! MCP Resources (`docs/04-mcp-api.md` §13) — URI is not auth; every read re-checks PAT.

use rmcp::model::{
    ListResourcesResult, ReadResourceResponse, ReadResourceResult, Resource, ResourceContents,
};
use rmcp::ErrorData;
use uuid::Uuid;

use crate::error::AppError;
use crate::mcp::auth::{active_pat, resolve_repo};
use crate::AppState;

const DEPLOYMENT_URI_PREFIX: &str = "musehub://deployments/";

pub fn list_static_resources() -> ListResourcesResult {
    let templates = [
        Resource::new("musehub://deployments/{id}", "Deployment summary")
            .with_mime_type("application/json"),
        Resource::new("musehub://agent-runs/{id}/summary", "Agent run summary")
            .with_mime_type("application/json"),
    ];
    ListResourcesResult::with_all_items(templates.to_vec())
}

pub async fn read_resource(state: &AppState, uri: &str) -> Result<ReadResourceResponse, ErrorData> {
    let pat = active_pat(state)
        .await
        .map_err(|_| ErrorData::invalid_request("unauthorized", None))?;
    if uri.starts_with(DEPLOYMENT_URI_PREFIX) {
        let id_str = uri.strip_prefix(DEPLOYMENT_URI_PREFIX).unwrap_or("");
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
            .map_err(|e| match e {
                AppError::Forbidden | AppError::Unauthorized => {
                    ErrorData::invalid_request("forbidden", None)
                }
                AppError::NotFound => ErrorData::invalid_params("not found", None),
                other => ErrorData::internal_error(other.to_string(), None),
            })?;
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
    Err(ErrorData::invalid_params("unsupported resource uri", None))
}
