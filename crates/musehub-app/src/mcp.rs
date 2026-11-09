//! Production MCP (`/mcp`) on the product API — PAT Bearer auth, domain read tools.
//!
//! Distinct from `musehub-mcp-probe` (`m0_mcp_probe`, `production_claim: false`).

use std::borrow::Cow;
use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{header::AUTHORIZATION, StatusCode},
    middleware::{self, Next},
    response::Response,
    Router,
};
use musehub_core::version;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters, ServerHandler},
    model::{
        CallToolResult, ErrorData, Implementation, ProtocolVersion, ServerCapabilities,
        ServerConfig,
    },
    schemars::JsonSchema,
    tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::AppError;
use crate::git_http::git_clone_url;
use crate::pat_auth::{self, PatContext};
use crate::AppState;

pub const PRODUCTION_SCOPE: &str = "m1_product_mcp";
pub const MCP_CONTRACT_VERSION: i32 = 1;
pub const RMCP_SDK_CRATE_VERSION: &str = "3.8.0";
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2026-07-28"];

pub fn product_mcp_enabled() -> bool {
    std::env::var("MUSEHUB_MCP_PRODUCT")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DomainErrorBody {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolEnvelope {
    pub schema_version: i32,
    pub request_id: String,
    pub data: Option<Value>,
    pub page: Option<Value>,
    pub error: Option<DomainErrorBody>,
}

impl ToolEnvelope {
    pub fn success(data: Value) -> Self {
        Self {
            schema_version: MCP_CONTRACT_VERSION,
            request_id: Uuid::new_v4().to_string(),
            data: Some(data),
            page: None,
            error: None,
        }
    }

    pub fn domain_error(
        code: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            schema_version: MCP_CONTRACT_VERSION,
            request_id: Uuid::new_v4().to_string(),
            data: None,
            page: None,
            error: Some(DomainErrorBody {
                code: code.into(),
                message: message.into(),
                retryable,
                details: Some(json!({ "mcp_scope": PRODUCTION_SCOPE })),
            }),
        }
    }
}

#[derive(Clone)]
pub struct ProductionMcpServer {
    state: Arc<AppState>,
    tool_router: ToolRouter<Self>,
}

impl ProductionMcpServer {
    pub fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    pub fn capability_payload() -> Value {
        json!({
            "service_version": version(),
            "contract_version": MCP_CONTRACT_VERSION,
            "mcp_scope": PRODUCTION_SCOPE,
            "rmcp_sdk_crate_version": RMCP_SDK_CRATE_VERSION,
            "supported_protocol_versions": SUPPORTED_PROTOCOL_VERSIONS,
            "tool_groups": ["repository_read"],
            "limits": {
                "idempotency_write_tools_verified": false,
                "oauth_verified": false
            },
            "production_claim": true
        })
    }

    async fn active_pat(&self) -> Result<PatContext, AppError> {
        let guard = self.state.mcp_pat_slot.read().await;
        guard.clone().ok_or_else(|| AppError::Unauthorized)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ProductionMcpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "musehub-mcp-product",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "MuseHub production MCP (read tools). Requires PAT Bearer with repo:read.",
            )
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28])
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RepositoryListInput {
    pub org_id: String,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[tool_router(router = tool_router)]
impl ProductionMcpServer {
    #[tool(
        name = "musehub_capabilities_get",
        description = "Return production MCP capabilities and verified scope marker",
        annotations(read_only_hint = true)
    )]
    async fn musehub_capabilities_get(&self) -> Result<CallToolResult, ErrorData> {
        let _ = self.active_pat().await.map_err(app_error_to_mcp)?;
        Ok(CallToolResult::structured(json!(ToolEnvelope::success(
            Self::capability_payload()
        ))))
    }

    #[tool(
        name = "musehub_repository_list",
        description = "List repositories in an organization the PAT may read",
        annotations(read_only_hint = true)
    )]
    async fn musehub_repository_list(
        &self,
        Parameters(input): Parameters<RepositoryListInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = self.active_pat().await.map_err(app_error_to_mcp)?;
        if !pat_auth::scope_allows(&pat.scopes, "repo:read") {
            let envelope =
                ToolEnvelope::domain_error("FORBIDDEN", "PAT missing repo:read scope", false);
            return Ok(CallToolResult::structured_error(json!(envelope)));
        }
        let org_id = Uuid::parse_str(input.org_id.trim())
            .map_err(|_| ErrorData::invalid_params("org_id must be a UUID", None))?;
        if !pat_auth::pat_org_allowed(pat.org_id, org_id) {
            let envelope = ToolEnvelope::domain_error(
                "FORBIDDEN",
                "PAT not valid for this organization",
                false,
            );
            return Ok(CallToolResult::structured_error(json!(envelope)));
        }
        let slug: Option<(String,)> =
            sqlx::query_as("SELECT slug FROM organizations WHERE id = $1")
                .bind(org_id)
                .fetch_optional(&self.state.pool)
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let Some((org_slug,)) = slug else {
            let envelope = ToolEnvelope::domain_error("NOT_FOUND", "organization not found", false);
            return Ok(CallToolResult::structured_error(json!(envelope)));
        };
        let _principal = pat.principal_id;
        if crate::org_id_for_member(&self.state.pool, &org_slug, pat.principal_id)
            .await
            .is_err()
        {
            let envelope =
                ToolEnvelope::domain_error("FORBIDDEN", "not a member of organization", false);
            return Ok(CallToolResult::structured_error(json!(envelope)));
        }
        let limit = input.limit.unwrap_or(20).clamp(1, 100);
        let rows = sqlx::query_as::<_, (Uuid, String, String)>(
            "SELECT r.id, r.slug, r.state
             FROM repositories r
             WHERE r.org_id = $1
             ORDER BY r.created_at
             LIMIT $2",
        )
        .bind(org_id)
        .bind(limit as i64)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let items: Vec<Value> = rows
            .into_iter()
            .map(|(id, slug, repo_state)| {
                json!({
                    "id": id.to_string(),
                    "slug": slug,
                    "state": repo_state,
                    "clone_url": git_clone_url(&self.state.bind_display, &org_slug, &slug),
                })
            })
            .collect();
        Ok(CallToolResult::structured(json!(ToolEnvelope::success(
            json!({
                "items": items,
                "org_id": org_id.to_string(),
            })
        ))))
    }
}

fn app_error_to_mcp(error: AppError) -> ErrorData {
    match error {
        AppError::Unauthorized => ErrorData::invalid_request("unauthorized", None),
        AppError::Validation(msg) => ErrorData::invalid_params(msg, None),
        other => ErrorData::internal_error(other.to_string(), None),
    }
}

pub fn parse_bearer_token(authorization_header: Option<&str>) -> Option<&str> {
    authorization_header?
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
}

pub fn product_mcp_router(state: Arc<AppState>, cancel: CancellationToken) -> Router {
    let state_for_service = state.clone();
    let service = StreamableHttpService::new(
        move || Ok(ProductionMcpServer::new(state_for_service.clone())),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true)
            .with_cancellation_token(cancel),
    );

    Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, require_pat_for_mcp))
}

async fn require_pat_for_mcp(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if !request.uri().path().starts_with("/mcp") {
        return Ok(next.run(request).await);
    }
    let header = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let Some(token) = parse_bearer_token(header) else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let pat = pat_auth::authenticate_pat_context(&state.pool, token)
        .await
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    {
        let mut guard = state.mcp_pat_slot.write().await;
        *guard = Some(pat);
    }
    let response = next.run(request).await;
    let mut guard = state.mcp_pat_slot.write().await;
    *guard = None;
    Ok(response)
}

pub fn merge_product_mcp(api: Router, state: Arc<AppState>) -> Router {
    if !product_mcp_enabled() {
        return api;
    }
    let cancel = CancellationToken::new();
    api.merge(product_mcp_router(state, cancel))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_states_production() {
        let value = ProductionMcpServer::capability_payload();
        assert_eq!(value["mcp_scope"], PRODUCTION_SCOPE);
        assert_eq!(value["production_claim"], true);
    }

    #[test]
    fn parse_bearer_requires_prefix() {
        assert_eq!(parse_bearer_token(Some("Bearer mhp_x")), Some("mhp_x"));
        assert_eq!(parse_bearer_token(Some("Basic x")), None);
    }
}
