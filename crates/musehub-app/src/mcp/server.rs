//! Production MCP tool server (`/mcp`).

use std::borrow::Cow;
use std::fmt;
use std::marker::PhantomData;
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
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::AppError;
use crate::mcp::auth::{self, deny_runner_only_pat, require_scope, resolve_repo};
use crate::mcp::capabilities::capability_payload;
use crate::mcp::domain::{
    agent_run_cancel, agent_run_start, deployment_rollback, deployment_start, issue_create,
    issue_update, pipeline_run, plan_accept, plan_create, pr_create, pr_review, repo_summary_json,
    repository_create, IssueAssigneePatch, IssueUpdatePatch, PlanStepInput,
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
            tool_router: Self::tool_router() + server_contract::contract_tool_router(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct PageInput {
    #[serde(default)]
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
#[serde(deny_unknown_fields)]
struct OrgListInput {
    org_id: String,
    #[serde(default, flatten)]
    page: PageInput,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct IssueListInput {
    #[serde(flatten)]
    repo: RepoInput,
    #[serde(default, flatten)]
    page: PageInput,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    assignee_id: Option<Uuid>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    label: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct IssueGetInput {
    #[serde(flatten)]
    repo: RepoInput,
    issue_id: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PlanGetInput {
    #[serde(flatten)]
    repo: RepoInput,
    plan_id: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PrGetInput {
    #[serde(flatten)]
    repo: RepoInput,
    pr_id: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AgentRunGetInput {
    #[serde(flatten)]
    repo: RepoInput,
    agent_run_id: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReleaseGetInput {
    #[serde(flatten)]
    repo: RepoInput,
    release_id: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DeploymentGetInput {
    #[serde(flatten)]
    repo: RepoInput,
    deployment_id: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DeploymentTraceInput {
    #[serde(flatten)]
    repo: RepoInput,
    deployment_id: String,
}

#[derive(Debug, Clone, Default)]
enum PatchField<T> {
    #[default]
    Missing,
    Null,
    Value(T),
}

struct PatchFieldVisitor<T>(PhantomData<T>);

impl<'de, T> Visitor<'de> for PatchFieldVisitor<T>
where
    T: Deserialize<'de>,
{
    type Value = PatchField<T>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("null or a valid value")
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(PatchField::Null)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(PatchField::Null)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        T::deserialize(deserializer).map(PatchField::Value)
    }
}

impl<'de, T> Deserialize<'de> for PatchField<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_option(PatchFieldVisitor(PhantomData))
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct IssueCreateInput {
    #[serde(flatten)]
    repo: RepoInput,
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    criteria: Vec<crate::mcp::contract_domain::CriterionInput>,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    assignee_id: Option<Uuid>,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct IssueUpdatePatchInput {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    labels: Option<Vec<String>>,
    #[serde(default)]
    resolution_reason: Option<String>,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    assignee_id: PatchField<Uuid>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct IssueUpdateInput {
    #[serde(flatten)]
    repo: RepoInput,
    issue_id: String,
    expected_version: i32,
    #[serde(flatten)]
    patch: IssueUpdatePatchInput,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RepositoryCreateInput {
    org_id: String,
    slug: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    default_branch: Option<String>,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PrCreateInput {
    #[serde(flatten)]
    repo: RepoInput,
    title: String,
    #[serde(default)]
    body: String,
    source_branch: String,
    target_branch: String,
    expected_head_oid: String,
    #[serde(default)]
    issue_revision_ids: Vec<String>,
    #[serde(default)]
    draft: bool,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PrMergeInput {
    #[serde(flatten)]
    repo: RepoInput,
    pr_id: String,
    #[serde(default)]
    expected_version: Option<i32>,
    expected_head_oid: String,
    expected_base_oid: String,
    expected_candidate_oid: String,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PrListInput {
    #[serde(flatten)]
    repo: RepoInput,
    #[serde(default)]
    state: Option<String>,
    #[serde(default, flatten)]
    page: PageInput,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PlanCreateInput {
    #[serde(flatten)]
    repo: RepoInput,
    issue_revision_id: String,
    body: String,
    #[serde(default)]
    steps: Vec<PlanStepInput>,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PlanAcceptInput {
    #[serde(flatten)]
    repo: RepoInput,
    plan_id: String,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
struct AgentRunCancelInput {
    #[serde(flatten)]
    repo: RepoInput,
    agent_run_id: String,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
        name = "musehub_repository_create",
        description = "Create a repository in an organization (org admin)",
        annotations(destructive_hint = false, idempotent_hint = true)
    )]
    async fn musehub_repository_create(
        &self,
        Parameters(input): Parameters<RepositoryCreateInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, WRITE_SCOPE) {
            return Ok(domain_err(e));
        }
        let org_id = parse_uuid("org_id", &input.org_id)?;
        if !crate::pat_auth::pat_org_allowed(pat.org_id, org_id) {
            return Ok(domain_err(AppError::Forbidden));
        }
        let org_slug: (String,) = sqlx::query_as("SELECT slug FROM organizations WHERE id = $1")
            .bind(org_id)
            .fetch_optional(&self.state.pool)
            .await
            .map_err(|e| app_error_to_mcp(e.into()))?
            .ok_or_else(|| app_error_to_mcp(AppError::NotFound))?;
        let canonical = json!({
            "org_id": input.org_id,
            "slug": input.slug,
            "description": input.description,
            "default_branch": input.default_branch,
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(org_id),
            "musehub_repository_create",
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
        match repository_create(
            &self.state,
            org_id,
            &org_slug.0,
            pat.principal_id,
            &input.slug,
            input.description.as_deref(),
            input.default_branch.as_deref(),
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
        name = "musehub_issue_list",
        description = "List issues in a repository",
        annotations(read_only_hint = true)
    )]
    async fn musehub_issue_list(
        &self,
        Parameters(input): Parameters<IssueListInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, READ_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let state = input.state.map(|state| state.trim().to_ascii_lowercase());
        if state.as_deref().is_some_and(|state| {
            !matches!(
                state,
                "open" | "in_progress" | "in_review" | "resolved" | "closed"
            )
        }) {
            return Ok(domain_err(AppError::Validation(
                "invalid issue state".to_string(),
            )));
        }
        let query = input.query.map(|query| query.trim().to_string());
        let query = query.filter(|query| !query.is_empty());
        let label = input.label.map(|label| label.trim().to_string());
        let label = label.filter(|label| !label.is_empty());
        if let Some(label) = &label {
            if let Err(error) = crate::issues::validate_label_filter(label) {
                return Ok(domain_err(error));
            }
        }
        if let Some(assignee_id) = input.assignee_id {
            if let Err(error) = crate::issues::repo_assignee_id_for_id(
                &self.state.pool,
                access.org_id,
                access.repo_id,
                assignee_id,
            )
            .await
            {
                return Ok(domain_err(error));
            }
        }
        let limit = page_limit(input.page.limit);
        let after = input
            .page
            .cursor
            .as_deref()
            .map(|cursor| {
                pagination::decode_issue_cursor(
                    cursor,
                    access.repo_id,
                    pat.principal_id,
                    state.as_deref(),
                    input.assignee_id,
                    query.as_deref(),
                    label.as_deref(),
                    limit,
                )
            })
            .transpose()
            .map_err(app_error_to_mcp)?;
        let rows = sqlx::query_as::<_, (Uuid, i64, String, String, Vec<String>, Option<Uuid>)>(
            "SELECT id, number, title, state, labels, assignee_id FROM issues
             WHERE repo_id = $1
               AND ($2::text IS NULL OR state = $2)
               AND ($3::uuid IS NULL OR assignee_id = $3)
               AND ($4::text IS NULL OR title ILIKE '%' || $4 || '%' OR body ILIKE '%' || $4 || '%')
               AND ($5::text IS NULL OR $5 = ANY(labels))
               AND ($6::bigint IS NULL OR (number, id) > ($6, $7))
             ORDER BY number, id LIMIT $8",
        )
        .bind(access.repo_id)
        .bind(&state)
        .bind(input.assignee_id)
        .bind(&query)
        .bind(&label)
        .bind(after.map(|(number, _)| number))
        .bind(after.map(|(_, id)| id))
        .bind(limit + 1)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| app_error_to_mcp(e.into()))?;
        let has_more = rows.len() as i64 > limit;
        let mut rows = rows;
        if has_more {
            rows.truncate(limit as usize);
        }
        let next_cursor = if has_more {
            rows.last()
                .map(|(id, number, _, _, _, _)| {
                    pagination::encode_issue_cursor(
                        access.repo_id,
                        pat.principal_id,
                        state.as_deref(),
                        input.assignee_id,
                        query.as_deref(),
                        label.as_deref(),
                        limit,
                        *number,
                        *id,
                    )
                })
                .transpose()
                .map_err(app_error_to_mcp)?
        } else {
            None
        };
        let items: Vec<Value> = rows
            .into_iter()
            .map(|(id, number, title, state, labels, assignee_id)| {
                json!({
                    "id": id.to_string(),
                    "number": number,
                    "title": title,
                    "state": state,
                    "labels": labels,
                    "assignee_id": assignee_id.map(|id| id.to_string()),
                })
            })
            .collect();
        Ok(domain_ok_page(
            json!({ "items": items }),
            page_meta(has_more, next_cursor),
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
        let row = sqlx::query_as::<
            _,
            (
                Uuid,
                i64,
                String,
                String,
                Option<Uuid>,
                Option<String>,
                Option<Uuid>,
                Vec<String>,
                Option<String>,
            ),
        >(
            "SELECT i.id, i.number, i.title, i.state, i.current_revision_id, r.body,
                    i.assignee_id, i.labels, i.close_reason
             FROM issues i
             LEFT JOIN issue_revisions r ON r.id = i.current_revision_id
             WHERE i.id = $1 AND i.repo_id = $2",
        )
        .bind(issue_id)
        .bind(access.repo_id)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| app_error_to_mcp(e.into()))?;
        let Some((
            id,
            number,
            title,
            state,
            revision_id,
            body,
            assignee_id,
            labels,
            resolution_reason,
        )) = row
        else {
            return Ok(domain_err(AppError::NotFound));
        };
        Ok(domain_ok(json!({
            "id": id.to_string(),
            "number": number,
            "title": title,
            "state": state,
            "body": body.unwrap_or_default(),
            "labels": labels,
            "resolution_reason": resolution_reason,
            "assignee_id": assignee_id.map(|id| id.to_string()),
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
            "criteria": input.criteria,
            "labels": input.labels,
            "assignee_id": input.assignee_id,
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
            &input.criteria,
            &input.labels,
            input.assignee_id,
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
        name = "musehub_issue_update",
        description = "Update an issue (CAS on revision)",
        annotations(idempotent_hint = true)
    )]
    async fn musehub_issue_update(
        &self,
        Parameters(input): Parameters<IssueUpdateInput>,
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
        let canonical_assignee = match &input.patch.assignee_id {
            PatchField::Missing => json!({"operation": "unchanged"}),
            PatchField::Null => json!({"operation": "clear"}),
            PatchField::Value(id) => json!({"operation": "set", "assignee_id": id}),
        };
        let canonical = json!({
            "org_id": input.repo.org_id,
            "repository_id": input.repo.repository_id,
            "issue_id": input.issue_id,
            "expected_version": input.expected_version,
            "title": input.patch.title,
            "body": input.patch.body,
            "state": input.patch.state,
            "labels": input.patch.labels,
            "resolution_reason": input.patch.resolution_reason,
            "assignee": canonical_assignee,
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(access.org_id),
            "musehub_issue_update",
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
        let assignee_id = match input.patch.assignee_id {
            PatchField::Missing => IssueAssigneePatch::Unchanged,
            PatchField::Null => IssueAssigneePatch::Clear,
            PatchField::Value(id) => IssueAssigneePatch::Set(id),
        };
        let patch = IssueUpdatePatch {
            title: input.patch.title,
            body: input.patch.body,
            state: input.patch.state,
            labels: input.patch.labels,
            resolution_reason: input.patch.resolution_reason,
            assignee_id,
        };
        match issue_update(
            &self.state,
            &access,
            pat.principal_id,
            issue_id,
            input.expected_version,
            patch,
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
        let row = sqlx::query_as::<_, (Uuid, i32, String, Uuid, String, serde_json::Value)>(
            "SELECT id, version, state, issue_revision_id, body, steps FROM implementation_plans
             WHERE id = $1 AND repo_id = $2",
        )
        .bind(plan_id)
        .bind(access.repo_id)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| app_error_to_mcp(e.into()))?;
        let Some((id, version, state, revision_id, body, steps)) = row else {
            return Ok(domain_err(AppError::NotFound));
        };
        Ok(domain_ok(json!({
            "id": id.to_string(),
            "version": version,
            "state": state,
            "issue_revision_id": revision_id.to_string(),
            "body": body,
            "steps": steps,
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
            "steps": input.steps,
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
        match plan_create(
            &self.state,
            &access,
            issue_revision_id,
            &input.body,
            &input.steps,
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
        name = "musehub_pr_list",
        description = "List pull requests in a repository",
        annotations(read_only_hint = true)
    )]
    async fn musehub_pr_list(
        &self,
        Parameters(input): Parameters<PrListInput>,
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
        let state_filter = input.state.as_deref().map(|s| s.trim().to_lowercase());
        let rows = if let Some(st) = state_filter.as_deref() {
            sqlx::query_as::<_, (Uuid, i64, String, String, String, String)>(
                "SELECT id, number, title, source_branch, target_branch, state
                 FROM pull_requests WHERE repo_id = $1 AND state = $2
                 ORDER BY number DESC LIMIT $3",
            )
            .bind(access.repo_id)
            .bind(st)
            .bind(limit)
            .fetch_all(&self.state.pool)
            .await
        } else {
            sqlx::query_as::<_, (Uuid, i64, String, String, String, String)>(
                "SELECT id, number, title, source_branch, target_branch, state
                 FROM pull_requests WHERE repo_id = $1 ORDER BY number DESC LIMIT $2",
            )
            .bind(access.repo_id)
            .bind(limit)
            .fetch_all(&self.state.pool)
            .await
        }
        .map_err(|e| app_error_to_mcp(e.into()))?;
        let has_more = rows.len() as i64 == limit;
        let items: Vec<Value> = rows
            .into_iter()
            .map(|(id, number, title, source_branch, target_branch, state)| {
                json!({
                    "id": id.to_string(),
                    "number": number,
                    "title": title,
                    "source_branch": source_branch,
                    "target_branch": target_branch,
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
        name = "musehub_pr_create",
        description = "Open a pull request for an existing branch",
        annotations(idempotent_hint = true)
    )]
    async fn musehub_pr_create(
        &self,
        Parameters(input): Parameters<PrCreateInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, WRITE_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let mut issue_revision_ids = Vec::new();
        for raw in &input.issue_revision_ids {
            issue_revision_ids.push(parse_uuid("issue_revision_id", raw)?);
        }
        let canonical = json!({
            "org_id": input.repo.org_id,
            "repository_id": input.repo.repository_id,
            "title": input.title,
            "body": input.body,
            "source_branch": input.source_branch,
            "target_branch": input.target_branch,
            "expected_head_oid": input.expected_head_oid,
            "issue_revision_ids": input.issue_revision_ids,
            "draft": input.draft,
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(access.org_id),
            "musehub_pr_create",
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
        let target_branch = if input.target_branch.trim().is_empty() {
            None
        } else {
            Some(input.target_branch.as_str())
        };
        match pr_create(
            &self.state,
            &access,
            pat.principal_id,
            &input.title,
            &input.source_branch,
            target_branch,
            &input.expected_head_oid,
            &issue_revision_ids,
            input.draft,
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
        name = "musehub_pr_merge",
        description = "Merge an open pull request (CAS on base/head)",
        annotations(destructive_hint = true, idempotent_hint = true)
    )]
    async fn musehub_pr_merge(
        &self,
        Parameters(input): Parameters<PrMergeInput>,
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
            "expected_version": input.expected_version,
            "expected_head_oid": input.expected_head_oid,
            "expected_base_oid": input.expected_base_oid,
            "expected_candidate_oid": input.expected_candidate_oid,
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(access.org_id),
            "musehub_pr_merge",
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
        let storage_key: (Uuid,) =
            sqlx::query_as("SELECT storage_key FROM repositories WHERE id = $1")
                .bind(access.repo_id)
                .fetch_one(&self.state.pool)
                .await
                .map_err(|e| app_error_to_mcp(e.into()))?;
        match crate::pr::merge_pull_request_by_id(
            &self.state,
            access.repo_id,
            storage_key.0,
            pat.principal_id,
            pr_id,
            &input.expected_head_oid,
            &input.expected_base_oid,
            &input.expected_candidate_oid,
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
            let belongs: Option<(i32,)> =
                sqlx::query_as("SELECT 1 FROM repositories WHERE id = $1 AND org_id = $2")
                    .bind(rid)
                    .bind(org_id)
                    .fetch_optional(&self.state.pool)
                    .await
                    .map_err(|e| app_error_to_mcp(e.into()))?;
            if belongs.is_none() {
                return Ok(domain_err(AppError::NotFound));
            }
        }
        let limit = page_limit(input.page.limit);
        let items = crate::audit::query_org_audit(
            &self.state.pool,
            org_id,
            repo_id,
            input.resource_type.clone(),
            resource_id,
            limit,
        )
        .await
        .map_err(app_error_to_mcp)?;
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

#[path = "server_contract.rs"]
mod server_contract;

#[cfg(test)]
mod strict_input_tests {
    use super::{IssueGetInput, OrgListInput};
    use crate::mcp::{contract_domain::CriterionInput, domain::PlanStepInput};
    use serde_json::json;

    #[test]
    fn flattened_tool_inputs_reject_unknown_fields() {
        let input = json!({
            "org_id": "org",
            "cursor": "next",
            "limit": 10,
            "unexpected": true
        });
        assert!(serde_json::from_value::<OrgListInput>(input).is_err());

        let input = json!({
            "org_id": "org",
            "repository_id": "repo",
            "issue_id": "issue",
            "unexpected": true
        });
        assert!(serde_json::from_value::<IssueGetInput>(input).is_err());
    }

    #[test]
    fn nested_tool_inputs_reject_unknown_fields() {
        let criterion = json!({
            "description": "works",
            "verification_method": "manual",
            "required": true,
            "unexpected": true
        });
        assert!(serde_json::from_value::<CriterionInput>(criterion).is_err());

        let step = json!({
            "key": "build",
            "title": "Build",
            "description": "Build the project",
            "criterion_ordinals": [1],
            "unexpected": true
        });
        assert!(serde_json::from_value::<PlanStepInput>(step).is_err());
    }

    #[test]
    fn tool_input_schemas_disallow_additional_properties() {
        for schema in [
            rmcp::schemars::schema_for!(IssueGetInput),
            rmcp::schemars::schema_for!(CriterionInput),
            rmcp::schemars::schema_for!(PlanStepInput),
        ] {
            let schema = serde_json::to_value(schema).expect("schema serializes");
            assert_eq!(schema["additionalProperties"], json!(false));
        }
    }
}

#[cfg(test)]
mod assignee_patch_tests {
    use super::{IssueUpdatePatchInput, PatchField};
    use uuid::Uuid;

    #[test]
    fn update_assignee_distinguishes_omitted_null_and_id() {
        let missing: IssueUpdatePatchInput = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(matches!(missing.assignee_id, PatchField::Missing));

        let clear: IssueUpdatePatchInput =
            serde_json::from_value(serde_json::json!({"assignee_id": null})).unwrap();
        assert!(matches!(clear.assignee_id, PatchField::Null));

        let id = Uuid::new_v4();
        let set: IssueUpdatePatchInput =
            serde_json::from_value(serde_json::json!({"assignee_id": id.to_string()})).unwrap();
        assert!(matches!(set.assignee_id, PatchField::Value(value) if value == id));
    }
}
