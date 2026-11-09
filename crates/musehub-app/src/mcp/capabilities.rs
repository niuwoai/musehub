use musehub_core::version;
use serde_json::{json, Value};

use super::envelope::MCP_CONTRACT_VERSION;
use super::{PRODUCTION_SCOPE, RMCP_SDK_CRATE_VERSION, SUPPORTED_PROTOCOL_VERSIONS};

/// Tools implemented on the product `/mcp` server (subset of `docs/04-mcp-api.md`).
pub const IMPLEMENTED_TOOLS: &[&str] = &[
    "musehub_capabilities_get",
    "musehub_repository_list",
    "musehub_repository_get",
    "musehub_issue_list",
    "musehub_issue_get",
    "musehub_issue_create",
    "musehub_plan_get",
    "musehub_plan_create",
    "musehub_plan_accept",
    "musehub_pr_get",
    "musehub_pr_review",
    "musehub_agent_run_get",
    "musehub_agent_run_start",
    "musehub_agent_run_cancel",
    "musehub_pipeline_run",
    "musehub_release_get",
    "musehub_deployment_start",
    "musehub_deployment_rollback",
    "musehub_deployment_get",
    "musehub_deployment_trace",
    "musehub_audit_list",
];

/// Resource URI templates from `docs/04-mcp-api.md` §13 (V1 catalog; reads enforce PAT + repo scope).
/// Resource URI reads actually implemented on `/mcp` (PAT + scope enforced).
pub const V1_RESOURCE_READS_IMPLEMENTED: &[&str] = &[
    "musehub://deployments/{deployment_id}",
    "musehub://agent-runs/{agent_run_id}/summary",
];

pub const V1_RESOURCE_URI_TEMPLATES: &[&str] = &[
    "musehub://repositories/{repository_id}/commits/{oid}/files/{encoded-path}",
    "musehub://issues/{issue_id}/revisions/{n}",
    "musehub://pull-requests/{pr_id}/revisions/{n}",
    "musehub://agent-runs/{agent_run_id}/summary",
    "musehub://deployments/{deployment_id}",
];

pub const WRITE_IDEMPOTENCY_TOOLS: &[&str] = &[
    "musehub_issue_create",
    "musehub_plan_create",
    "musehub_plan_accept",
    "musehub_pr_review",
    "musehub_agent_run_start",
    "musehub_agent_run_cancel",
    "musehub_pipeline_run",
    "musehub_deployment_start",
    "musehub_deployment_rollback",
];

pub fn capability_payload() -> Value {
    json!({
        "service_version": version(),
        "contract_version": MCP_CONTRACT_VERSION,
        "mcp_scope": PRODUCTION_SCOPE,
        "rmcp_sdk_crate_version": RMCP_SDK_CRATE_VERSION,
        "supported_protocol_versions": SUPPORTED_PROTOCOL_VERSIONS,
        "maturity": "partial_implementation",
        "implemented_tools": IMPLEMENTED_TOOLS,
        "resource_uri_templates_v1": V1_RESOURCE_URI_TEMPLATES,
        "resource_reads_implemented_v1": V1_RESOURCE_READS_IMPLEMENTED,
        "write_idempotency_tools": WRITE_IDEMPOTENCY_TOOLS,
        "authentication": {
            "http_bearer_pat": true,
            "remote_oauth_v1": false
        },
        "limits": {
            "full_contract_tools": false,
            "idempotency_write_tools_verified": true,
            "oauth_verified": false
        }
    })
}
