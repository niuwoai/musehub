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
    "musehub_release_get",
    "musehub_deployment_get",
    "musehub_deployment_trace",
];

pub const WRITE_IDEMPOTENCY_TOOLS: &[&str] = &[
    "musehub_issue_create",
    "musehub_plan_create",
    "musehub_plan_accept",
    "musehub_pr_review",
    "musehub_agent_run_start",
    "musehub_agent_run_cancel",
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
