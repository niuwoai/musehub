//! Production MCP tool server (`/mcp`).

use std::borrow::Cow;
use std::sync::Arc;

use crate::mcp::prompts;
use crate::mcp::resources;
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters, ServerHandler},
    model::{
        CallToolResult, ErrorData, GetPromptRequestParams, GetPromptResponse, Implementation,
        ListPromptsResult, ListResourcesResult, PaginatedRequestParams, ProtocolVersion,
        ReadResourceRequestParams, ReadResourceResponse, ServerCapabilities, ServerConfig,
    },
    schemars::JsonSchema,
    service::RequestContext,
    tool, tool_handler, tool_router, RoleServer,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::AppError;
use crate::mcp::auth::{self, deny_runner_only_pat, require_scope, resolve_repo};
use crate::mcp::capabilities::capability_payload;
use crate::mcp::domain::{
    agent_run_cancel, agent_run_start, deployment_rollback, deployment_start, issue_create,
    pipeline_run, plan_accept, plan_create, pr_review, repo_summary_json,
};
use crate::mcp::envelope::{app_err_envelope, ToolEnvelope, MCP_CONTRACT_VERSION};
use crate::mcp::idempotency::{
    begin_idempotent_write, complete_idempotent_write, fail_idempotent_write, IdempotencyAction,
};
use crate::mcp::pagination;
use crate::AppState;

const READ_SCOPE: &str = "repo:read";
const WRITE_SCOPE: &str = "repo:write";
const AUDIT_READ_SCOPE: &str = "audit:read";

fn require_audit_read(pat: &crate::pat_auth::PatContext) -> Result<(), AppError> {
    if require_scope(pat, AUDIT_READ_SCOPE).is_ok() || require_scope(pat, READ_SCOPE).is_ok() {
        Ok(())
    } else {
        Err(AppError::Forbidden)
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
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct PageInput {
    #[serde(default)]
    #[allow(dead_code)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct RepoInput {
    org_id: String,
    repository_id: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct OrgListInput {
    org_id: String,
    #[serde(default, flatten)]
    page: PageInput,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct RepoPageInput {
    #[serde(flatten)]
    repo: RepoInput,
    #[serde(default, flatten)]
    page: PageInput,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct IssueGetInput {
    #[serde(flatten)]
    repo: RepoInput,
    issue_id: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct PlanGetInput {
    #[serde(flatten)]
    repo: RepoInput,
    plan_id: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct PrGetInput {
    #[serde(flatten)]
    repo: RepoInput,
    pr_id: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct AgentRunGetInput {
    #[serde(flatten)]
    repo: RepoInput,
    agent_run_id: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct ReleaseGetInput {
    #[serde(flatten)]
    repo: RepoInput,
    release_id: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct DeploymentGetInput {
    #[serde(flatten)]
    repo: RepoInput,
    deployment_id: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct DeploymentTraceInput {
    #[serde(flatten)]
    repo: RepoInput,
    deployment_id: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct IssueCreateInput {
    #[serde(flatten)]
    repo: RepoInput,
    title: String,
    #[serde(default)]
    body: String,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct PlanCreateInput {
    #[serde(flatten)]
    repo: RepoInput,
    issue_revision_id: String,
    body: String,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct PlanAcceptInput {
    #[serde(flatten)]
    repo: RepoInput,
    plan_id: String,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct PrReviewInput {
    #[serde(flatten)]
    repo: RepoInput,
    pr_id: String,
    decision: String,
    #[serde(default)]
    body: String,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct AgentRunStartInput {
    #[serde(flatten)]
    repo: RepoInput,
    issue_id: String,
    message: String,
    #[serde(default)]
    adapter: Option<String>,
    #[serde(default)]
    budget_tokens: Option<u64>,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct AgentRunCancelInput {
    #[serde(flatten)]
    repo: RepoInput,
    agent_run_id: String,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct PipelineRunInput {
    #[serde(flatten)]
    repo: RepoInput,
    workflow_path: String,
    #[serde(default)]
    workflow_source_oid: Option<String>,
    target_oid: String,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct DeploymentStartInput {
    #[serde(flatten)]
    repo: RepoInput,
    environment_id: String,
    release_id: String,
    #[serde(default)]
    expected_environment_version: Option<i64>,
    #[serde(default)]
    config_version: Option<i64>,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct DeploymentRollbackInput {
    #[serde(flatten)]
    repo: RepoInput,
    environment_id: String,
    target_healthy_deployment_id: String,
    #[serde(default)]
    expected_environment_version: Option<i64>,
    #[serde(default)]
    reason: Option<String>,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct AuditListInput {
    org_id: String,
    #[serde(default)]
    repository_id: Option<String>,
    #[serde(default)]
    resource_type: Option<String>,
    #[serde(default)]
    resource_id: Option<String>,
    #[serde(default, flatten)]
    page: PageInput,
}

fn app_error_to_mcp(err: AppError) -> ErrorData {
    match err {
        AppError::Unauthorized => ErrorData::invalid_request("unauthorized", None),
        other => ErrorData::internal_error(other.to_string(), None),
    }
}

fn parse_uuid(field: &str, raw: &str) -> Result<Uuid, ErrorData> {
    raw.parse::<Uuid>().map_err(|_| {
        ErrorData::invalid_params(
            format!("invalid UUID for {field}"),
            Some(json!({ "field": field })),
        )
    })
}

fn page_limit(limit: Option<u32>) -> i64 {
    match limit.unwrap_or(20) {
        0 => 20,
        n if n > 100 => 100,
        n => n as i64,
    }
}

fn page_meta(has_more: bool, next_cursor: Option<String>) -> Value {
    json!({
        "has_more": has_more,
        "next_cursor": next_cursor
    })
}

fn domain_ok(data: Value) -> CallToolResult {
    CallToolResult::structured(json!(ToolEnvelope::success(data)))
}

fn domain_ok_page(data: Value, page: Value) -> CallToolResult {
    CallToolResult::structured(json!(ToolEnvelope::success_page(data, page)))
}

fn domain_err(err: AppError) -> CallToolResult {
    CallToolResult::structured_error(json!(app_err_envelope(err)))
}

async fn pat_context(state: &AppState) -> Result<crate::pat_auth::PatContext, ErrorData> {
    let pat = auth::active_pat(state).await.map_err(app_error_to_mcp)?;
    if let Err(e) = deny_runner_only_pat(&pat) {
        return Err(match e {
            AppError::Unauthorized => app_error_to_mcp(e),
            _ => ErrorData::invalid_request("forbidden", None),
        });
    }
    Ok(pat)
}

fn parse_repo_uuids(input: &RepoInput) -> Result<(Uuid, Uuid), AppError> {
    let org_id = input
        .org_id
        .parse::<Uuid>()
        .map_err(|_| AppError::Validation("invalid UUID for org_id".into()))?;
    let repository_id = input
        .repository_id
        .parse::<Uuid>()
        .map_err(|_| AppError::Validation("invalid UUID for repository_id".into()))?;
    Ok((org_id, repository_id))
}

async fn repo_access(
    state: &AppState,
    pat: &crate::pat_auth::PatContext,
    input: &RepoInput,
) -> Result<auth::RepoAccess, AppError> {
    let (org_id, repository_id) = parse_repo_uuids(input)?;
    resolve_repo(state, pat, org_id, repository_id).await
}

fn envelope_value(envelope: &ToolEnvelope) -> Value {
    serde_json::to_value(envelope).unwrap_or(json!({
        "schema_version": MCP_CONTRACT_VERSION,
        "error": { "code": "INTERNAL", "message": "serialize failed" }
    }))
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ProductionMcpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
            .with_server_info(Implementation::new(
                "musehub-mcp-product",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "MuseHub product MCP (partial contract). Authenticate with PAT Bearer HTTP only; remote OAuth is not supported on this endpoint.",
            )
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28])
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let _ = pat_context(&self.state).await?;
        Ok(resources::list_static_resources())
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        resources::read_resource(&self.state, request.uri.as_str()).await
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        let _ = pat_context(&self.state).await?;
        Ok(prompts::list_prompts())
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        let _ = pat_context(&self.state).await?;
        prompts::get_prompt(request.name.as_str())
    }
}

#[tool_router(router = tool_router)]
impl ProductionMcpServer {
    #[tool(
        name = "musehub_capabilities_get",
        description = "Return product MCP capabilities and implemented tool catalog",
        annotations(read_only_hint = true)
    )]
    async fn musehub_capabilities_get(&self) -> Result<CallToolResult, ErrorData> {
        let _ = pat_context(&self.state).await?;
        Ok(domain_ok(capability_payload()))
    }

    #[tool(
        name = "musehub_repository_list",
        description = "List repositories in an organization",
        annotations(read_only_hint = true)
    )]
    async fn musehub_repository_list(
        &self,
        Parameters(input): Parameters<OrgListInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, READ_SCOPE) {
            return Ok(domain_err(e));
        }
        let org_id = parse_uuid("org_id", &input.org_id)?;
        if !crate::pat_auth::pat_org_allowed(pat.org_id, org_id) {
            return Ok(domain_err(AppError::Forbidden));
        }
        let member: Option<(i32,)> = sqlx::query_as(
            "SELECT 1 FROM memberships WHERE org_id = $1 AND principal_id = $2 AND status = 'active'",
        )
        .bind(org_id)
        .bind(pat.principal_id)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| app_error_to_mcp(e.into()))?;
        if member.is_none() {
            return Ok(domain_err(AppError::Forbidden));
        }
        let org_slug: (String,) = sqlx::query_as("SELECT slug FROM organizations WHERE id = $1")
            .bind(org_id)
            .fetch_optional(&self.state.pool)
            .await
            .map_err(|e| app_error_to_mcp(e.into()))?
            .ok_or_else(|| app_error_to_mcp(AppError::NotFound))?;
        let limit = page_limit(input.page.limit);
        let after = match input.page.cursor.as_deref() {
            Some(c) => Some(pagination::decode_repo_cursor(c).map_err(app_error_to_mcp)?),
            None => None,
        };
        let fetch_limit = limit + 1;
        let rows = if let Some((created_at, repo_id)) = after {
            sqlx::query_as::<_, (Uuid, String, String, time::OffsetDateTime)>(
                "SELECT r.id, r.slug, r.state, r.created_at FROM repositories r
                 WHERE r.org_id = $1
                   AND (r.created_at > $2 OR (r.created_at = $2 AND r.id > $3))
                 ORDER BY r.created_at, r.id LIMIT $4",
            )
            .bind(org_id)
            .bind(created_at)
            .bind(repo_id)
            .bind(fetch_limit)
            .fetch_all(&self.state.pool)
            .await
        } else {
            sqlx::query_as::<_, (Uuid, String, String, time::OffsetDateTime)>(
                "SELECT r.id, r.slug, r.state, r.created_at FROM repositories r
                 WHERE r.org_id = $1 ORDER BY r.created_at, r.id LIMIT $2",
            )
            .bind(org_id)
            .bind(fetch_limit)
            .fetch_all(&self.state.pool)
            .await
        }
        .map_err(|e| app_error_to_mcp(e.into()))?;
        let has_more = rows.len() as i64 > limit;
        let rows: Vec<_> = rows.into_iter().take(limit as usize).collect();
        let next_cursor = if has_more {
            rows.last()
                .map(|(id, _, _, created_at)| pagination::encode_repo_cursor(*created_at, *id))
        } else {
            None
        };
        let items: Vec<Value> = rows
            .into_iter()
            .map(|(id, slug, state, _)| {
                repo_summary_json(&self.state.bind_display, &org_slug.0, id, &slug, &state)
            })
            .collect();
        Ok(domain_ok_page(
            json!({ "items": items }),
            page_meta(has_more, next_cursor),
        ))
    }

    #[tool(
        name = "musehub_repository_get",
        description = "Get repository metadata",
        annotations(read_only_hint = true)
    )]
    async fn musehub_repository_get(
        &self,
        Parameters(input): Parameters<RepoInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, READ_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let row = sqlx::query_as::<_, (String, Option<String>, String)>(
            "SELECT slug, description, state FROM repositories WHERE id = $1",
        )
        .bind(access.repo_id)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| app_error_to_mcp(e.into()))?;
        let Some((slug, description, state)) = row else {
            return Ok(domain_err(AppError::NotFound));
        };
        let mut data = repo_summary_json(
            &self.state.bind_display,
            &access.org_slug,
            access.repo_id,
            &slug,
            &state,
        );
        if let Some(map) = data.as_object_mut() {
            map.insert("description".into(), json!(description.unwrap_or_default()));
            map.insert("org_id".into(), json!(access.org_id.to_string()));
        }
        Ok(domain_ok(data))
    }

    #[tool(
        name = "musehub_issue_list",
        description = "List issues in a repository",
        annotations(read_only_hint = true)
    )]
    async fn musehub_issue_list(
        &self,
        Parameters(input): Parameters<RepoPageInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, READ_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let limit = page_limit(input.page.limit);
        let rows = sqlx::query_as::<_, (Uuid, i64, String, String)>(
            "SELECT id, number, title, state FROM issues WHERE repo_id = $1 ORDER BY number LIMIT $2",
        )
        .bind(access.repo_id)
        .bind(limit)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| app_error_to_mcp(e.into()))?;
        let has_more = rows.len() as i64 == limit;
        let items: Vec<Value> = rows
            .into_iter()
            .map(|(id, number, title, state)| {
                json!({
                    "id": id.to_string(),
                    "number": number,
                    "title": title,
                    "state": state,
                })
            })
            .collect();
        Ok(domain_ok_page(
            json!({ "items": items }),
            page_meta(has_more, None),
        ))
    }

    #[tool(
        name = "musehub_issue_get",
        description = "Get an issue with current revision",
        annotations(read_only_hint = true)
    )]
    async fn musehub_issue_get(
        &self,
        Parameters(input): Parameters<IssueGetInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, READ_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let issue_id = parse_uuid("issue_id", &input.issue_id)?;
        let row = sqlx::query_as::<_, (Uuid, i64, String, String, Option<Uuid>, Option<String>)>(
            "SELECT i.id, i.number, i.title, i.state, i.current_revision_id, r.body
             FROM issues i
             LEFT JOIN issue_revisions r ON r.id = i.current_revision_id
             WHERE i.id = $1 AND i.repo_id = $2",
        )
        .bind(issue_id)
        .bind(access.repo_id)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| app_error_to_mcp(e.into()))?;
        let Some((id, number, title, state, revision_id, body)) = row else {
            return Ok(domain_err(AppError::NotFound));
        };
        Ok(domain_ok(json!({
            "id": id.to_string(),
            "number": number,
            "title": title,
            "state": state,
            "body": body.unwrap_or_default(),
            "current_revision_id": revision_id.map(|u| u.to_string()),
        })))
    }

    #[tool(
        name = "musehub_issue_create",
        description = "Create an issue (idempotent)",
        annotations(destructive_hint = false, idempotent_hint = true)
    )]
    async fn musehub_issue_create(
        &self,
        Parameters(input): Parameters<IssueCreateInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, WRITE_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let canonical = json!({
            "org_id": input.repo.org_id,
            "repository_id": input.repo.repository_id,
            "title": input.title,
            "body": input.body,
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(access.org_id),
            "musehub_issue_create",
            &input.idempotency_key,
            &canonical_json,
        )
        .await
        .map_err(app_error_to_mcp)?;
        if let IdempotencyAction::Replay(stored) = action {
            return Ok(CallToolResult::structured(stored));
        }
        let IdempotencyAction::Proceed { record_id } = action else {
            return Ok(domain_err(AppError::Other("idempotency".into())));
        };
        match issue_create(
            &self.state,
            &access,
            pat.principal_id,
            &input.title,
            &input.body,
        )
        .await
        {
            Ok(data) => {
                let envelope = ToolEnvelope::success(data);
                let stored = envelope_value(&envelope);
                let _ = complete_idempotent_write(&self.state.pool, record_id, &stored, None).await;
                Ok(CallToolResult::structured(stored))
            }
            Err(e) => {
                let _ = fail_idempotent_write(&self.state.pool, record_id).await;
                Ok(domain_err(e))
            }
        }
    }

    #[tool(
        name = "musehub_plan_get",
        description = "Get an implementation plan",
        annotations(read_only_hint = true)
    )]
    async fn musehub_plan_get(
        &self,
        Parameters(input): Parameters<PlanGetInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, READ_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let plan_id = parse_uuid("plan_id", &input.plan_id)?;
        let row = sqlx::query_as::<_, (Uuid, i32, String, Uuid, String)>(
            "SELECT id, version, state, issue_revision_id, body FROM implementation_plans
             WHERE id = $1 AND repo_id = $2",
        )
        .bind(plan_id)
        .bind(access.repo_id)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| app_error_to_mcp(e.into()))?;
        let Some((id, version, state, revision_id, body)) = row else {
            return Ok(domain_err(AppError::NotFound));
        };
        Ok(domain_ok(json!({
            "id": id.to_string(),
            "version": version,
            "state": state,
            "issue_revision_id": revision_id.to_string(),
            "body": body,
        })))
    }

    #[tool(
        name = "musehub_plan_create",
        description = "Create a draft implementation plan",
        annotations(idempotent_hint = true)
    )]
    async fn musehub_plan_create(
        &self,
        Parameters(input): Parameters<PlanCreateInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, WRITE_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let issue_revision_id = parse_uuid("issue_revision_id", &input.issue_revision_id)?;
        let canonical = json!({
            "org_id": input.repo.org_id,
            "repository_id": input.repo.repository_id,
            "issue_revision_id": input.issue_revision_id,
            "body": input.body,
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(access.org_id),
            "musehub_plan_create",
            &input.idempotency_key,
            &canonical_json,
        )
        .await
        .map_err(app_error_to_mcp)?;
        if let IdempotencyAction::Replay(stored) = action {
            return Ok(CallToolResult::structured(stored));
        }
        let IdempotencyAction::Proceed { record_id } = action else {
            return Ok(domain_err(AppError::Other("idempotency".into())));
        };
        match plan_create(&self.state, &access, issue_revision_id, &input.body).await {
            Ok(data) => {
                let stored = envelope_value(&ToolEnvelope::success(data));
                let _ = complete_idempotent_write(&self.state.pool, record_id, &stored, None).await;
                Ok(CallToolResult::structured(stored))
            }
            Err(e) => {
                let _ = fail_idempotent_write(&self.state.pool, record_id).await;
                Ok(domain_err(e))
            }
        }
    }

    #[tool(
        name = "musehub_plan_accept",
        description = "Accept a draft plan bound to the issue current revision",
        annotations(idempotent_hint = true)
    )]
    async fn musehub_plan_accept(
        &self,
        Parameters(input): Parameters<PlanAcceptInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, WRITE_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let plan_id = parse_uuid("plan_id", &input.plan_id)?;
        let canonical = json!({
            "org_id": input.repo.org_id,
            "repository_id": input.repo.repository_id,
            "plan_id": input.plan_id,
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(access.org_id),
            "musehub_plan_accept",
            &input.idempotency_key,
            &canonical_json,
        )
        .await
        .map_err(app_error_to_mcp)?;
        if let IdempotencyAction::Replay(stored) = action {
            return Ok(CallToolResult::structured(stored));
        }
        let IdempotencyAction::Proceed { record_id } = action else {
            return Ok(domain_err(AppError::Other("idempotency".into())));
        };
        match plan_accept(&self.state, &access, plan_id).await {
            Ok(data) => {
                let stored = envelope_value(&ToolEnvelope::success(data));
                let _ = complete_idempotent_write(&self.state.pool, record_id, &stored, None).await;
                Ok(CallToolResult::structured(stored))
            }
            Err(e) => {
                let _ = fail_idempotent_write(&self.state.pool, record_id).await;
                Ok(domain_err(e))
            }
        }
    }

    #[tool(
        name = "musehub_pr_get",
        description = "Get a pull request including head check runs",
        annotations(read_only_hint = true)
    )]
    async fn musehub_pr_get(
        &self,
        Parameters(input): Parameters<PrGetInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, READ_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let pr_id = parse_uuid("pr_id", &input.pr_id)?;
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
            ),
        >(
            "SELECT id, number, title, source_branch, target_branch, state, head_oid, base_oid
             FROM pull_requests WHERE id = $1 AND repo_id = $2",
        )
        .bind(pr_id)
        .bind(access.repo_id)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| app_error_to_mcp(e.into()))?;
        let Some((id, number, title, source_branch, target_branch, state, head_oid, base_oid)) =
            row
        else {
            return Ok(domain_err(AppError::NotFound));
        };
        let head = head_oid.clone().unwrap_or_default();
        let checks = if head.is_empty() {
            Vec::new()
        } else {
            sqlx::query_as::<_, (String, String)>(
                "SELECT check_name, state FROM pr_check_runs WHERE pr_id = $1 AND head_oid = $2 ORDER BY check_name",
            )
            .bind(id)
            .bind(&head)
            .fetch_all(&self.state.pool)
            .await
            .map_err(|e| app_error_to_mcp(e.into()))?
            .into_iter()
            .map(|(check_name, check_state)| {
                json!({ "check_name": check_name, "state": check_state, "head_oid": head })
            })
            .collect::<Vec<_>>()
        };
        Ok(domain_ok(json!({
            "id": id.to_string(),
            "number": number,
            "title": title,
            "source_branch": source_branch,
            "target_branch": target_branch,
            "state": state,
            "head_oid": head_oid,
            "base_oid": base_oid,
            "checks": checks,
        })))
    }

    #[tool(
        name = "musehub_pr_review",
        description = "Submit a pull request review",
        annotations(idempotent_hint = true)
    )]
    async fn musehub_pr_review(
        &self,
        Parameters(input): Parameters<PrReviewInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, WRITE_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let pr_id = parse_uuid("pr_id", &input.pr_id)?;
        let canonical = json!({
            "org_id": input.repo.org_id,
            "repository_id": input.repo.repository_id,
            "pr_id": input.pr_id,
            "decision": input.decision,
            "body": input.body,
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(access.org_id),
            "musehub_pr_review",
            &input.idempotency_key,
            &canonical_json,
        )
        .await
        .map_err(app_error_to_mcp)?;
        if let IdempotencyAction::Replay(stored) = action {
            return Ok(CallToolResult::structured(stored));
        }
        let IdempotencyAction::Proceed { record_id } = action else {
            return Ok(domain_err(AppError::Other("idempotency".into())));
        };
        match pr_review(
            &self.state,
            &access,
            pat.principal_id,
            pr_id,
            &input.decision,
            &input.body,
        )
        .await
        {
            Ok(data) => {
                let stored = envelope_value(&ToolEnvelope::success(data));
                let _ = complete_idempotent_write(&self.state.pool, record_id, &stored, None).await;
                Ok(CallToolResult::structured(stored))
            }
            Err(e) => {
                let _ = fail_idempotent_write(&self.state.pool, record_id).await;
                Ok(domain_err(e))
            }
        }
    }

    #[tool(
        name = "musehub_agent_run_get",
        description = "Get an agent run in this repository",
        annotations(read_only_hint = true)
    )]
    async fn musehub_agent_run_get(
        &self,
        Parameters(input): Parameters<AgentRunGetInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, READ_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let run_id = parse_uuid("agent_run_id", &input.agent_run_id)?;
        let row = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                String,
                Option<String>,
                i64,
                Option<String>,
                Uuid,
            ),
        >(
            "SELECT id, adapter_kind, state, result_commit_oid, usage_tokens, error_code, issue_id
             FROM agent_runs WHERE id = $1 AND repo_id = $2",
        )
        .bind(run_id)
        .bind(access.repo_id)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| app_error_to_mcp(e.into()))?;
        let Some((id, adapter_kind, state, commit, tokens, error_code, issue_id)) = row else {
            return Ok(domain_err(AppError::NotFound));
        };
        Ok(domain_ok(json!({
            "id": id.to_string(),
            "adapter_kind": adapter_kind,
            "state": state,
            "result_commit_oid": commit,
            "usage_tokens": tokens,
            "error_code": error_code,
            "issue_id": issue_id.to_string(),
        })))
    }

    #[tool(
        name = "musehub_agent_run_start",
        description = "Queue an agent run for an issue",
        annotations(idempotent_hint = true)
    )]
    async fn musehub_agent_run_start(
        &self,
        Parameters(input): Parameters<AgentRunStartInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, WRITE_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let issue_id = parse_uuid("issue_id", &input.issue_id)?;
        let canonical = json!({
            "org_id": input.repo.org_id,
            "repository_id": input.repo.repository_id,
            "issue_id": input.issue_id,
            "message": input.message,
            "adapter": input.adapter,
            "budget_tokens": input.budget_tokens,
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(access.org_id),
            "musehub_agent_run_start",
            &input.idempotency_key,
            &canonical_json,
        )
        .await
        .map_err(app_error_to_mcp)?;
        if let IdempotencyAction::Replay(stored) = action {
            return Ok(CallToolResult::structured(stored));
        }
        let IdempotencyAction::Proceed { record_id } = action else {
            return Ok(domain_err(AppError::Other("idempotency".into())));
        };
        match agent_run_start(
            &self.state,
            &access,
            pat.principal_id,
            issue_id,
            &input.message,
            input.budget_tokens,
            input.adapter.as_deref(),
        )
        .await
        {
            Ok(data) => {
                let resource_id = data
                    .get("id")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Uuid>().ok());
                let stored = envelope_value(&ToolEnvelope::success(data));
                let _ =
                    complete_idempotent_write(&self.state.pool, record_id, &stored, resource_id)
                        .await;
                Ok(CallToolResult::structured(stored))
            }
            Err(e) => {
                let _ = fail_idempotent_write(&self.state.pool, record_id).await;
                Ok(domain_err(e))
            }
        }
    }

    #[tool(
        name = "musehub_agent_run_cancel",
        description = "Request cancellation of an agent run",
        annotations(idempotent_hint = true)
    )]
    async fn musehub_agent_run_cancel(
        &self,
        Parameters(input): Parameters<AgentRunCancelInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, WRITE_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let run_id = parse_uuid("agent_run_id", &input.agent_run_id)?;
        let canonical = json!({
            "org_id": input.repo.org_id,
            "repository_id": input.repo.repository_id,
            "agent_run_id": input.agent_run_id,
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(access.org_id),
            "musehub_agent_run_cancel",
            &input.idempotency_key,
            &canonical_json,
        )
        .await
        .map_err(app_error_to_mcp)?;
        if let IdempotencyAction::Replay(stored) = action {
            return Ok(CallToolResult::structured(stored));
        }
        let IdempotencyAction::Proceed { record_id } = action else {
            return Ok(domain_err(AppError::Other("idempotency".into())));
        };
        match agent_run_cancel(&self.state, pat.principal_id, run_id, access.repo_id).await {
            Ok(data) => {
                let stored = envelope_value(&ToolEnvelope::success(data));
                let _ =
                    complete_idempotent_write(&self.state.pool, record_id, &stored, Some(run_id))
                        .await;
                Ok(CallToolResult::structured(stored))
            }
            Err(e) => {
                let _ = fail_idempotent_write(&self.state.pool, record_id).await;
                Ok(domain_err(e))
            }
        }
    }

    #[tool(
        name = "musehub_pipeline_run",
        description = "Start a release pipeline run on the current main tip",
        annotations(idempotent_hint = true, destructive_hint = true)
    )]
    async fn musehub_pipeline_run(
        &self,
        Parameters(input): Parameters<PipelineRunInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, WRITE_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let canonical = json!({
            "org_id": input.repo.org_id,
            "repository_id": input.repo.repository_id,
            "workflow_path": input.workflow_path,
            "target_oid": input.target_oid,
            "workflow_source_oid": input.workflow_source_oid,
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(access.org_id),
            "musehub_pipeline_run",
            &input.idempotency_key,
            &canonical_json,
        )
        .await
        .map_err(app_error_to_mcp)?;
        if let IdempotencyAction::Replay(stored) = action {
            return Ok(CallToolResult::structured(stored));
        }
        let IdempotencyAction::Proceed { record_id } = action else {
            return Ok(domain_err(AppError::Other("idempotency".into())));
        };
        let _src = input
            .workflow_source_oid
            .as_deref()
            .unwrap_or(&input.target_oid);
        match pipeline_run(
            &self.state,
            &access,
            pat.principal_id,
            &input.workflow_path,
            &input.target_oid,
        )
        .await
        {
            Ok(data) => {
                let stored = envelope_value(&ToolEnvelope::success(data));
                let _ = complete_idempotent_write(&self.state.pool, record_id, &stored, None).await;
                Ok(CallToolResult::structured(stored))
            }
            Err(e) => {
                let _ = fail_idempotent_write(&self.state.pool, record_id).await;
                Ok(domain_err(e))
            }
        }
    }

    #[tool(
        name = "musehub_deployment_start",
        description = "Start a deployment for a published release",
        annotations(idempotent_hint = true, destructive_hint = true)
    )]
    async fn musehub_deployment_start(
        &self,
        Parameters(input): Parameters<DeploymentStartInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, WRITE_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let environment_id = parse_uuid("environment_id", &input.environment_id)?;
        let release_id = parse_uuid("release_id", &input.release_id)?;
        let canonical = json!({
            "org_id": input.repo.org_id,
            "repository_id": input.repo.repository_id,
            "environment_id": input.environment_id,
            "release_id": input.release_id,
            "expected_environment_version": input.expected_environment_version,
            "config_version": input.config_version,
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(access.org_id),
            "musehub_deployment_start",
            &input.idempotency_key,
            &canonical_json,
        )
        .await
        .map_err(app_error_to_mcp)?;
        if let IdempotencyAction::Replay(stored) = action {
            return Ok(CallToolResult::structured(stored));
        }
        let IdempotencyAction::Proceed { record_id } = action else {
            return Ok(domain_err(AppError::Other("idempotency".into())));
        };
        match deployment_start(&self.state, &access, environment_id, release_id).await {
            Ok(data) => {
                let stored = envelope_value(&ToolEnvelope::success(data));
                let _ = complete_idempotent_write(&self.state.pool, record_id, &stored, None).await;
                Ok(CallToolResult::structured(stored))
            }
            Err(e) => {
                let _ = fail_idempotent_write(&self.state.pool, record_id).await;
                Ok(domain_err(e))
            }
        }
    }

    #[tool(
        name = "musehub_deployment_rollback",
        description = "Rollback an environment to a previous healthy deployment",
        annotations(idempotent_hint = true, destructive_hint = true)
    )]
    async fn musehub_deployment_rollback(
        &self,
        Parameters(input): Parameters<DeploymentRollbackInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, WRITE_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let environment_id = parse_uuid("environment_id", &input.environment_id)?;
        let target_id = parse_uuid(
            "target_healthy_deployment_id",
            &input.target_healthy_deployment_id,
        )?;
        let canonical = json!({
            "org_id": input.repo.org_id,
            "repository_id": input.repo.repository_id,
            "environment_id": input.environment_id,
            "target_healthy_deployment_id": input.target_healthy_deployment_id,
            "expected_environment_version": input.expected_environment_version,
            "reason": input.reason,
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(access.org_id),
            "musehub_deployment_rollback",
            &input.idempotency_key,
            &canonical_json,
        )
        .await
        .map_err(app_error_to_mcp)?;
        if let IdempotencyAction::Replay(stored) = action {
            return Ok(CallToolResult::structured(stored));
        }
        let IdempotencyAction::Proceed { record_id } = action else {
            return Ok(domain_err(AppError::Other("idempotency".into())));
        };
        let attempt_id = Uuid::new_v4();
        match deployment_rollback(
            &self.state,
            &access,
            environment_id,
            target_id,
            attempt_id,
            input.reason.clone(),
        )
        .await
        {
            Ok(data) => {
                let stored = envelope_value(&ToolEnvelope::success(data));
                let _ = complete_idempotent_write(&self.state.pool, record_id, &stored, None).await;
                Ok(CallToolResult::structured(stored))
            }
            Err(e) => {
                let _ = fail_idempotent_write(&self.state.pool, record_id).await;
                Ok(domain_err(e))
            }
        }
    }

    #[tool(
        name = "musehub_audit_list",
        description = "List audit events for an organization",
        annotations(read_only_hint = true)
    )]
    async fn musehub_audit_list(
        &self,
        Parameters(input): Parameters<AuditListInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_audit_read(&pat) {
            return Ok(domain_err(e));
        }
        let org_id = parse_uuid("org_id", &input.org_id)?;
        if !crate::pat_auth::pat_org_allowed(pat.org_id, org_id) {
            return Ok(domain_err(AppError::Forbidden));
        }
        let member: Option<(i32,)> = sqlx::query_as(
            "SELECT 1 FROM memberships WHERE org_id = $1 AND principal_id = $2 AND status = 'active'",
        )
        .bind(org_id)
        .bind(pat.principal_id)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| app_error_to_mcp(e.into()))?;
        if member.is_none() {
            return Ok(domain_err(AppError::Forbidden));
        }
        let repo_id = match input.repository_id.as_deref() {
            Some(raw) => Some(parse_uuid("repository_id", raw)?),
            None => None,
        };
        let resource_id = match input.resource_id.as_deref() {
            Some(raw) => Some(parse_uuid("resource_id", raw)?),
            None => None,
        };
        if let Some(rid) = repo_id {
            let belongs: Option<(i32,)> = sqlx::query_as(
                "SELECT 1 FROM repositories WHERE id = $1 AND org_id = $2",
            )
            .bind(rid)
            .bind(org_id)
            .fetch_optional(&self.state.pool)
            .await
            .map_err(|e| app_error_to_mcp(e.into()))?;
            if belongs.is_none() {
                return Ok(domain_err(AppError::NotFound));
            }
        }
        let limit = page_limit(input.page.limit) as i64;
        let items = crate::audit::query_org_audit(
            &self.state.pool,
            org_id,
            repo_id,
            input.resource_type.clone(),
            resource_id,
            limit,
        )
        .await
        .map_err(|e| app_error_to_mcp(e.into()))?;
        let rows: Vec<Value> = items
            .into_iter()
            .map(|e| {
                json!({
                    "id": e.id.to_string(),
                    "org_id": e.org_id.map(|u| u.to_string()),
                    "repo_id": e.repo_id.map(|u| u.to_string()),
                    "actor_id": e.actor_id.to_string(),
                    "action": e.action,
                    "resource_type": e.resource_type,
                    "resource_id": e.resource_id.to_string(),
                    "decision": e.decision,
                    "metadata": e.metadata,
                    "occurred_at": e.occurred_at,
                })
            })
            .collect();
        Ok(domain_ok(json!({ "items": rows })))
    }

    #[tool(
        name = "musehub_release_get",
        description = "Get a release record",
        annotations(read_only_hint = true)
    )]
    async fn musehub_release_get(
        &self,
        Parameters(input): Parameters<ReleaseGetInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, READ_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let release_id = parse_uuid("release_id", &input.release_id)?;
        let row = sqlx::query_as::<_, (Uuid, String, String, String, Uuid, String, Option<String>, Option<String>)>(
            "SELECT id, version, tag_name, commit_oid, pipeline_run_id, state, manifest_sha256, oci_image_ref
             FROM releases WHERE id = $1 AND repo_id = $2",
        )
        .bind(release_id)
        .bind(access.repo_id)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| app_error_to_mcp(e.into()))?;
        let Some((
            id,
            version,
            tag_name,
            commit_oid,
            pipeline_run_id,
            state,
            manifest_sha256,
            oci_image_ref,
        )) = row
        else {
            return Ok(domain_err(AppError::NotFound));
        };
        Ok(domain_ok(json!({
            "id": id.to_string(),
            "version": version,
            "tag_name": tag_name,
            "commit_oid": commit_oid,
            "pipeline_run_id": pipeline_run_id.to_string(),
            "state": state,
            "manifest_sha256": manifest_sha256,
            "oci_image_ref": oci_image_ref,
        })))
    }

    #[tool(
        name = "musehub_deployment_get",
        description = "Get a deployment record",
        annotations(read_only_hint = true)
    )]
    async fn musehub_deployment_get(
        &self,
        Parameters(input): Parameters<DeploymentGetInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, READ_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let deployment_id = parse_uuid("deployment_id", &input.deployment_id)?;
        let row = sqlx::query_as::<_, (Uuid, Uuid, String, String, Option<Value>, String)>(
            "SELECT d.id, d.release_id, d.state, d.adapter_kind, d.health_summary, e.slug
             FROM deployments d
             JOIN environments e ON e.id = d.environment_id
             WHERE d.id = $1 AND d.repo_id = $2",
        )
        .bind(deployment_id)
        .bind(access.repo_id)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| app_error_to_mcp(e.into()))?;
        let Some((id, release_id, state, adapter_kind, health_summary, environment_slug)) = row
        else {
            return Ok(domain_err(AppError::NotFound));
        };
        Ok(domain_ok(json!({
            "id": id.to_string(),
            "release_id": release_id.to_string(),
            "state": state,
            "adapter_kind": adapter_kind,
            "health_summary": health_summary,
            "environment_slug": environment_slug,
        })))
    }

    #[tool(
        name = "musehub_deployment_trace",
        description = "Reverse trace fields for a deployment",
        annotations(read_only_hint = true)
    )]
    async fn musehub_deployment_trace(
        &self,
        Parameters(input): Parameters<DeploymentTraceInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, READ_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let deployment_id = parse_uuid("deployment_id", &input.deployment_id)?;
        let release_row = sqlx::query_as::<_, (Uuid,)>(
            "SELECT release_id FROM deployments WHERE id = $1 AND repo_id = $2",
        )
        .bind(deployment_id)
        .bind(access.repo_id)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| app_error_to_mcp(e.into()))?;
        let Some((release_id,)) = release_row else {
            return Ok(domain_err(AppError::NotFound));
        };
        let release = sqlx::query_as::<_, (String, Uuid, Option<String>)>(
            "SELECT commit_oid, pipeline_run_id, oci_image_ref FROM releases WHERE id = $1",
        )
        .bind(release_id)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| app_error_to_mcp(e.into()))?;
        let Some((commit_oid, pipeline_run_id, release_oci)) = release else {
            return Ok(domain_err(AppError::NotFound));
        };
        let dep = sqlx::query_as::<_, (Option<serde_json::Value>,)>(
            "SELECT health_summary FROM deployments WHERE id = $1",
        )
        .bind(deployment_id)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| app_error_to_mcp(e.into()))?;
        let health_summary = dep.and_then(|(h,)| h);
        let applied_oci_digest = health_summary.as_ref().and_then(|s| {
            s.get("applied_oci_digest")
                .and_then(|v| v.as_str())
                .map(String::from)
        });
        let verified_source_oid = health_summary.as_ref().and_then(|s| {
            s.get("verified_source_oid")
                .and_then(|v| v.as_str())
                .map(String::from)
        });
        let mut gaps = Vec::new();
        if release_oci.is_none() {
            gaps.push("release_oci_image_ref_missing".to_string());
        }
        if applied_oci_digest.is_none() {
            gaps.push("applied_oci_digest_missing".to_string());
        }
        if verified_source_oid.as_deref() != Some(commit_oid.as_str()) {
            gaps.push("verified_source_oid_mismatch".to_string());
        }
        Ok(domain_ok(json!({
            "deployment_id": deployment_id.to_string(),
            "release_id": release_id.to_string(),
            "pipeline_run_id": pipeline_run_id.to_string(),
            "commit_oid": commit_oid,
            "release_oci_image_ref": release_oci,
            "applied_oci_digest": applied_oci_digest,
            "verified_source_oid": verified_source_oid,
            "gaps": gaps,
        })))
    }
}
