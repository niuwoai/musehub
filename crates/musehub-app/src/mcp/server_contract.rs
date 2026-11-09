//! Additional MCP tools from `docs/04-mcp-api.md` §5–9 (contract completion).

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ErrorData},
    schemars::JsonSchema,
    tool, tool_router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::AppError;
use crate::mcp::contract_domain::{
    self, agent_profile_list, agent_run_messages, agent_run_respond, delivery_get,
    deployment_approve, deployment_list, discussion_comment, discussion_list,
    discussion_resolve, environment_list, git_repo_access, issue_criteria_replace, job_logs,
    job_retry, pipeline_cancel, pipeline_get, pipeline_list, pr_diff, pr_update, release_create_mcp,
    release_list, release_publish_mcp,
};
use crate::mcp::envelope::ToolEnvelope;
use crate::mcp::git_read::{
    git_commit_compare, git_literal_search, git_read_file_at, git_tree_entries,
};
use crate::mcp::idempotency::{
    begin_idempotent_write, complete_idempotent_write, fail_idempotent_write, IdempotencyAction,
};

use super::{
    app_error_to_mcp, domain_err, domain_ok, domain_ok_page, envelope_value, page_limit,
    page_meta, parse_uuid, pat_context, repo_access, ProductionMcpServer, OrgListInput, PageInput,
    RepoInput, require_scope, READ_SCOPE, WRITE_SCOPE,
};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct RepositoryTreeInput {
    #[serde(flatten)]
    repo: RepoInput,
    commit_oid: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default, flatten)]
    page: PageInput,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct RepositoryReadFileInput {
    #[serde(flatten)]
    repo: RepoInput,
    commit_oid: String,
    path: String,
    #[serde(default)]
    start_line: Option<u32>,
    #[serde(default)]
    max_lines: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct RepositorySearchInput {
    #[serde(flatten)]
    repo: RepoInput,
    commit_oid: String,
    query: String,
    #[serde(default)]
    path_prefix: Option<String>,
    #[serde(default, flatten)]
    page: PageInput,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct CommitCompareInput {
    #[serde(flatten)]
    repo: RepoInput,
    base_oid: String,
    head_oid: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default, flatten)]
    page: PageInput,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct ContractCriterionInput {
    description: String,
    #[serde(default = "default_verification")]
    verification_method: String,
    #[serde(default = "default_true")]
    required: bool,
}

fn default_verification() -> String {
    "manual".to_string()
}
fn default_true() -> bool {
    true
}

impl From<ContractCriterionInput> for contract_domain::CriterionInput {
    fn from(c: ContractCriterionInput) -> Self {
        contract_domain::CriterionInput {
            description: c.description,
            verification_method: c.verification_method,
            required: c.required,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct IssueCriteriaReplaceInput {
    #[serde(flatten)]
    repo: RepoInput,
    issue_id: String,
    expected_version: i32,
    criteria: Vec<ContractCriterionInput>,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize, JsonSchema)]
struct DiscussionTarget {
    kind: String,
    id: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct DiscussionListInput {
    #[serde(flatten)]
    repo: RepoInput,
    target: DiscussionTarget,
    #[serde(default, flatten)]
    page: PageInput,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct DiscussionCommentInput {
    #[serde(flatten)]
    repo: RepoInput,
    target: DiscussionTarget,
    body: String,
    #[serde(default)]
    anchor: Option<Value>,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct DiscussionResolveInput {
    #[serde(flatten)]
    repo: RepoInput,
    comment_id: String,
    expected_version: i64,
    resolved: bool,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct DeliveryGetInput {
    #[serde(flatten)]
    repo: RepoInput,
    issue_id: String,
    #[serde(default, flatten)]
    page: PageInput,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct PrUpdateInput {
    #[serde(flatten)]
    repo: RepoInput,
    pr_id: String,
    expected_version: i64,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    state: Option<String>,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct PrDiffInput {
    #[serde(flatten)]
    repo: RepoInput,
    pr_id: String,
    pr_revision_id: i64,
    #[serde(default)]
    path: Option<String>,
    #[serde(default, flatten)]
    page: PageInput,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct AgentRunMessagesInput {
    #[serde(flatten)]
    repo: RepoInput,
    agent_run_id: String,
    #[serde(default, flatten)]
    page: PageInput,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct AgentRunRespondInput {
    #[serde(flatten)]
    repo: RepoInput,
    agent_run_id: String,
    message: String,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct PipelineListInput {
    #[serde(flatten)]
    repo: RepoInput,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    commit_oid: Option<String>,
    #[serde(default, flatten)]
    page: PageInput,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct PipelineGetInput {
    #[serde(flatten)]
    repo: RepoInput,
    pipeline_run_id: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct JobLogsInput {
    #[serde(flatten)]
    repo: RepoInput,
    job_id: String,
    attempt_id: String,
    #[serde(default)]
    cursor: Option<i32>,
    #[serde(default)]
    max_bytes: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct JobRetryInput {
    #[serde(flatten)]
    repo: RepoInput,
    job_id: String,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct PipelineCancelInput {
    #[serde(flatten)]
    repo: RepoInput,
    pipeline_run_id: String,
    reason: String,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct ReleaseListInput {
    #[serde(flatten)]
    repo: RepoInput,
    #[serde(default, flatten)]
    page: PageInput,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct ReleaseCreateInput {
    #[serde(flatten)]
    repo: RepoInput,
    version: String,
    tag_name: String,
    commit_oid: String,
    pipeline_run_id: String,
    #[serde(default)]
    pr_id: Option<String>,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct ReleasePublishInput {
    #[serde(flatten)]
    repo: RepoInput,
    release_id: String,
    expected_commit_oid: String,
    expected_manifest_hash: String,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct EnvironmentListInput {
    #[serde(flatten)]
    repo: RepoInput,
    #[serde(default, flatten)]
    page: PageInput,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct DeploymentApproveInput {
    #[serde(flatten)]
    repo: RepoInput,
    deployment_id: String,
    expected_plan_hash: String,
    decision: String,
    reason: String,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct DeploymentListInput {
    #[serde(flatten)]
    repo: RepoInput,
    environment_id: String,
    #[serde(default, flatten)]
    page: PageInput,
}

fn target_ids(target: &DiscussionTarget) -> Result<(&str, Uuid), AppError> {
    let kind = target.kind.as_str();
    if !matches!(kind, "issue" | "pr") {
        return Err(AppError::Validation("invalid discussion target kind".into()));
    }
    let id = target
        .id
        .parse::<Uuid>()
        .map_err(|_| AppError::Validation("invalid target id".into()))?;
    Ok((kind, id))
}

#[tool_router(router = contract_tool_router)]
impl ProductionMcpServer {
    #[tool(
        name = "musehub_repository_tree",
        description = "List tree entries at a commit path",
        annotations(read_only_hint = true)
    )]
    async fn musehub_repository_tree(
        &self,
        Parameters(input): Parameters<RepositoryTreeInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, READ_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let bare = match git_repo_access(&self.state, &access).await {
            Ok(p) => p,
            Err(e) => return Ok(domain_err(e)),
        };
        let path = input.path.unwrap_or_default();
        let limit = page_limit(input.page.limit);
        match git_tree_entries(&bare, &input.commit_oid, &path, limit) {
            Ok((items, has_more)) => Ok(domain_ok_page(
                json!({ "items": items }),
                page_meta(has_more, None),
            )),
            Err(e) => Ok(domain_err(e)),
        }
    }

    #[tool(
        name = "musehub_repository_read_file",
        description = "Read file contents at a commit",
        annotations(read_only_hint = true)
    )]
    async fn musehub_repository_read_file(
        &self,
        Parameters(input): Parameters<RepositoryReadFileInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, READ_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let bare = match git_repo_access(&self.state, &access).await {
            Ok(p) => p,
            Err(e) => return Ok(domain_err(e)),
        };
        let start = input.start_line.unwrap_or(1);
        let max_lines = input.max_lines.unwrap_or(200).clamp(1, 500);
        match git_read_file_at(&bare, &input.commit_oid, &input.path, start, max_lines) {
            Ok(data) => Ok(domain_ok(data)),
            Err(e) => Ok(domain_err(e)),
        }
    }

    #[tool(
        name = "musehub_repository_search",
        description = "Literal text search at a commit",
        annotations(read_only_hint = true)
    )]
    async fn musehub_repository_search(
        &self,
        Parameters(input): Parameters<RepositorySearchInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, READ_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let bare = match git_repo_access(&self.state, &access).await {
            Ok(p) => p,
            Err(e) => return Ok(domain_err(e)),
        };
        let limit = page_limit(input.page.limit);
        match git_literal_search(
            &bare,
            &input.commit_oid,
            &input.query,
            input.path_prefix.as_deref(),
            limit,
        ) {
            Ok((items, truncated)) => Ok(domain_ok(json!({
                "items": items,
                "truncated": truncated,
            }))),
            Err(e) => Ok(domain_err(e)),
        }
    }

    #[tool(
        name = "musehub_commit_compare",
        description = "Compare two commits",
        annotations(read_only_hint = true)
    )]
    async fn musehub_commit_compare(
        &self,
        Parameters(input): Parameters<CommitCompareInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, READ_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let bare = match git_repo_access(&self.state, &access).await {
            Ok(p) => p,
            Err(e) => return Ok(domain_err(e)),
        };
        let limit = page_limit(input.page.limit);
        match git_commit_compare(
            &bare,
            &input.base_oid,
            &input.head_oid,
            input.path.as_deref(),
            limit,
        ) {
            Ok((items, summary, truncated)) => Ok(domain_ok(json!({
                "items": items,
                "summary": summary,
                "truncated": truncated,
            }))),
            Err(e) => Ok(domain_err(e)),
        }
    }

    #[tool(
        name = "musehub_issue_criteria_replace",
        description = "Replace issue acceptance criteria (new revision)",
        annotations(idempotent_hint = true)
    )]
    async fn musehub_issue_criteria_replace(
        &self,
        Parameters(input): Parameters<IssueCriteriaReplaceInput>,
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
            "expected_version": input.expected_version,
            "criteria_len": input.criteria.len(),
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(access.org_id),
            "musehub_issue_criteria_replace",
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
        let criteria: Vec<contract_domain::CriterionInput> = input
            .criteria
            .into_iter()
            .map(|c| c.into())
            .collect();
        match issue_criteria_replace(
            &self.state,
            &access,
            pat.principal_id,
            issue_id,
            input.expected_version,
            &criteria,
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
        name = "musehub_discussion_list",
        description = "List discussion comments for an issue or PR",
        annotations(read_only_hint = true)
    )]
    async fn musehub_discussion_list(
        &self,
        Parameters(input): Parameters<DiscussionListInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, READ_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let (kind, target_id) = match target_ids(&input.target) {
            Ok(v) => v,
            Err(e) => return Ok(domain_err(e)),
        };
        let limit = page_limit(input.page.limit);
        match discussion_list(&self.state, &access, kind, target_id, limit).await {
            Ok((items, has_more)) => Ok(domain_ok_page(
                json!({ "items": items }),
                page_meta(has_more, None),
            )),
            Err(e) => Ok(domain_err(e)),
        }
    }

    #[tool(
        name = "musehub_discussion_comment",
        description = "Add a discussion comment",
        annotations(idempotent_hint = true)
    )]
    async fn musehub_discussion_comment(
        &self,
        Parameters(input): Parameters<DiscussionCommentInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, WRITE_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let (kind, target_id) = match target_ids(&input.target) {
            Ok(v) => v,
            Err(e) => return Ok(domain_err(e)),
        };
        let canonical = json!({
            "org_id": input.repo.org_id,
            "repository_id": input.repo.repository_id,
            "target": input.target,
            "body": input.body,
            "anchor": input.anchor,
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(access.org_id),
            "musehub_discussion_comment",
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
        match discussion_comment(
            &self.state,
            &access,
            pat.principal_id,
            kind,
            target_id,
            &input.body,
            input.anchor,
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
        name = "musehub_discussion_resolve",
        description = "Resolve or unresolve a PR discussion comment",
        annotations(idempotent_hint = true)
    )]
    async fn musehub_discussion_resolve(
        &self,
        Parameters(input): Parameters<DiscussionResolveInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, WRITE_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let comment_id = parse_uuid("comment_id", &input.comment_id)?;
        let canonical = json!({
            "org_id": input.repo.org_id,
            "repository_id": input.repo.repository_id,
            "comment_id": input.comment_id,
            "expected_version": input.expected_version,
            "resolved": input.resolved,
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(access.org_id),
            "musehub_discussion_resolve",
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
        match discussion_resolve(
            &self.state,
            &access,
            pat.principal_id,
            comment_id,
            input.expected_version,
            input.resolved,
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
        name = "musehub_delivery_get",
        description = "Delivery links and status for an issue",
        annotations(read_only_hint = true)
    )]
    async fn musehub_delivery_get(
        &self,
        Parameters(input): Parameters<DeliveryGetInput>,
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
        let limit = page_limit(input.page.limit);
        match delivery_get(&self.state.pool, &access, issue_id, limit).await {
            Ok((items, status)) => Ok(domain_ok(json!({
                "items": items,
                "current_requirements_status": status,
            }))),
            Err(e) => Ok(domain_err(e)),
        }
    }

    #[tool(
        name = "musehub_pr_update",
        description = "Update pull request metadata",
        annotations(idempotent_hint = true)
    )]
    async fn musehub_pr_update(
        &self,
        Parameters(input): Parameters<PrUpdateInput>,
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
            "title": input.title,
            "state": input.state,
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(access.org_id),
            "musehub_pr_update",
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
        match pr_update(
            &self.state,
            &access,
            pr_id,
            input.expected_version,
            input.title.as_deref(),
            input.state.as_deref(),
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
        name = "musehub_pr_diff",
        description = "File diffs for a pull request revision",
        annotations(read_only_hint = true)
    )]
    async fn musehub_pr_diff(
        &self,
        Parameters(input): Parameters<PrDiffInput>,
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
        let limit = page_limit(input.page.limit);
        match pr_diff(
            &self.state,
            &access,
            pr_id,
            input.pr_revision_id,
            input.path.as_deref(),
            limit,
        )
        .await
        {
            Ok((items, truncated)) => Ok(domain_ok(json!({
                "items": items,
                "truncated": truncated,
            }))),
            Err(e) => Ok(domain_err(e)),
        }
    }

    #[tool(
        name = "musehub_agent_profile_list",
        description = "List agent profiles for an organization",
        annotations(read_only_hint = true)
    )]
    async fn musehub_agent_profile_list(
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
        let limit = page_limit(input.page.limit);
        match agent_profile_list(org_id, limit).await {
            Ok(items) => Ok(domain_ok_page(
                json!({ "items": items }),
                page_meta(false, None),
            )),
            Err(e) => Ok(domain_err(e)),
        }
    }

    #[tool(
        name = "musehub_agent_run_messages",
        description = "List agent run messages",
        annotations(read_only_hint = true)
    )]
    async fn musehub_agent_run_messages(
        &self,
        Parameters(input): Parameters<AgentRunMessagesInput>,
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
        let limit = page_limit(input.page.limit);
        match agent_run_messages(&self.state, &access, run_id, limit).await {
            Ok((items, has_more)) => Ok(domain_ok_page(
                json!({ "items": items }),
                page_meta(has_more, None),
            )),
            Err(e) => Ok(domain_err(e)),
        }
    }

    #[tool(
        name = "musehub_agent_run_respond",
        description = "Respond to an agent run waiting for input",
        annotations(idempotent_hint = true)
    )]
    async fn musehub_agent_run_respond(
        &self,
        Parameters(input): Parameters<AgentRunRespondInput>,
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
            "message": input.message,
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(access.org_id),
            "musehub_agent_run_respond",
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
        match agent_run_respond(&self.state, &access, run_id, &input.message).await {
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
        name = "musehub_pipeline_list",
        description = "List pipeline runs",
        annotations(read_only_hint = true)
    )]
    async fn musehub_pipeline_list(
        &self,
        Parameters(input): Parameters<PipelineListInput>,
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
        match pipeline_list(
            &self.state.pool,
            access.repo_id,
            input.state.as_deref(),
            input.commit_oid.as_deref(),
            limit,
        )
        .await
        {
            Ok((items, has_more)) => Ok(domain_ok_page(
                json!({ "items": items }),
                page_meta(has_more, None),
            )),
            Err(e) => Ok(domain_err(e)),
        }
    }

    #[tool(
        name = "musehub_pipeline_get",
        description = "Get a pipeline run",
        annotations(read_only_hint = true)
    )]
    async fn musehub_pipeline_get(
        &self,
        Parameters(input): Parameters<PipelineGetInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, READ_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let run_id = parse_uuid("pipeline_run_id", &input.pipeline_run_id)?;
        match pipeline_get(&self.state.pool, access.repo_id, run_id).await {
            Ok(data) => Ok(domain_ok(data)),
            Err(e) => Ok(domain_err(e)),
        }
    }

    #[tool(
        name = "musehub_job_logs",
        description = "Fetch pipeline job log chunks",
        annotations(read_only_hint = true)
    )]
    async fn musehub_job_logs(
        &self,
        Parameters(input): Parameters<JobLogsInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, READ_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let job_id = parse_uuid("job_id", &input.job_id)?;
        let attempt_id = parse_uuid("attempt_id", &input.attempt_id)?;
        let max_bytes = input.max_bytes.unwrap_or(65536).clamp(1, 262_144) as usize;
        match job_logs(
            &self.state.pool,
            access.repo_id,
            job_id,
            attempt_id,
            input.cursor,
            max_bytes,
        )
        .await
        {
            Ok(data) => Ok(domain_ok(data)),
            Err(e) => Ok(domain_err(e)),
        }
    }

    #[tool(
        name = "musehub_job_retry",
        description = "Retry a failed pipeline job",
        annotations(idempotent_hint = true)
    )]
    async fn musehub_job_retry(
        &self,
        Parameters(input): Parameters<JobRetryInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, WRITE_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let job_id = parse_uuid("job_id", &input.job_id)?;
        let canonical = json!({
            "org_id": input.repo.org_id,
            "repository_id": input.repo.repository_id,
            "job_id": input.job_id,
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(access.org_id),
            "musehub_job_retry",
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
        match job_retry(&self.state, &access, job_id).await {
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
        name = "musehub_pipeline_cancel",
        description = "Cancel a pipeline run",
        annotations(idempotent_hint = true, destructive_hint = true)
    )]
    async fn musehub_pipeline_cancel(
        &self,
        Parameters(input): Parameters<PipelineCancelInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, WRITE_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let run_id = parse_uuid("pipeline_run_id", &input.pipeline_run_id)?;
        let canonical = json!({
            "org_id": input.repo.org_id,
            "repository_id": input.repo.repository_id,
            "pipeline_run_id": input.pipeline_run_id,
            "reason": input.reason,
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(access.org_id),
            "musehub_pipeline_cancel",
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
        match pipeline_cancel(&self.state.pool, access.repo_id, run_id).await {
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
        name = "musehub_release_list",
        description = "List releases",
        annotations(read_only_hint = true)
    )]
    async fn musehub_release_list(
        &self,
        Parameters(input): Parameters<ReleaseListInput>,
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
        match release_list(&self.state.pool, access.repo_id, limit).await {
            Ok((items, has_more)) => Ok(domain_ok_page(
                json!({ "items": items }),
                page_meta(has_more, None),
            )),
            Err(e) => Ok(domain_err(e)),
        }
    }

    #[tool(
        name = "musehub_release_create",
        description = "Create a draft release",
        annotations(idempotent_hint = true)
    )]
    async fn musehub_release_create(
        &self,
        Parameters(input): Parameters<ReleaseCreateInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, WRITE_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let pipeline_run_id = parse_uuid("pipeline_run_id", &input.pipeline_run_id)?;
        let pr_id = match input.pr_id.as_deref() {
            Some(s) => Some(parse_uuid("pr_id", s)?),
            None => None,
        };
        let canonical = json!({
            "org_id": input.repo.org_id,
            "repository_id": input.repo.repository_id,
            "version": input.version,
            "tag_name": input.tag_name,
            "commit_oid": input.commit_oid,
            "pipeline_run_id": input.pipeline_run_id,
            "pr_id": input.pr_id,
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(access.org_id),
            "musehub_release_create",
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
        match release_create_mcp(
            &self.state,
            &access,
            &input.version,
            &input.tag_name,
            &input.commit_oid,
            pipeline_run_id,
            pr_id,
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
        name = "musehub_release_publish",
        description = "Publish a release",
        annotations(idempotent_hint = true)
    )]
    async fn musehub_release_publish(
        &self,
        Parameters(input): Parameters<ReleasePublishInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, WRITE_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let release_id = parse_uuid("release_id", &input.release_id)?;
        let canonical = json!({
            "org_id": input.repo.org_id,
            "repository_id": input.repo.repository_id,
            "release_id": input.release_id,
            "expected_commit_oid": input.expected_commit_oid,
            "expected_manifest_hash": input.expected_manifest_hash,
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(access.org_id),
            "musehub_release_publish",
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
        match release_publish_mcp(
            &self.state,
            &access,
            release_id,
            &input.expected_commit_oid,
            &input.expected_manifest_hash,
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
        name = "musehub_environment_list",
        description = "List deployment environments",
        annotations(read_only_hint = true)
    )]
    async fn musehub_environment_list(
        &self,
        Parameters(input): Parameters<EnvironmentListInput>,
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
        match environment_list(&self.state.pool, access.repo_id, limit).await {
            Ok((items, has_more)) => Ok(domain_ok_page(
                json!({ "items": items }),
                page_meta(has_more, None),
            )),
            Err(e) => Ok(domain_err(e)),
        }
    }

    #[tool(
        name = "musehub_deployment_approve",
        description = "Approve or reject a deployment",
        annotations(idempotent_hint = true)
    )]
    async fn musehub_deployment_approve(
        &self,
        Parameters(input): Parameters<DeploymentApproveInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, WRITE_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let deployment_id = parse_uuid("deployment_id", &input.deployment_id)?;
        let canonical = json!({
            "org_id": input.repo.org_id,
            "repository_id": input.repo.repository_id,
            "deployment_id": input.deployment_id,
            "expected_plan_hash": input.expected_plan_hash,
            "decision": input.decision,
            "reason": input.reason,
        });
        let canonical_json = serde_json::to_string(&canonical).unwrap_or_default();
        let action = begin_idempotent_write(
            &self.state.pool,
            pat.principal_id,
            Some(access.org_id),
            "musehub_deployment_approve",
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
        match deployment_approve(
            &self.state,
            &access,
            pat.principal_id,
            deployment_id,
            &input.expected_plan_hash,
            &input.decision,
            &input.reason,
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
        name = "musehub_deployment_list",
        description = "List deployments for an environment",
        annotations(read_only_hint = true)
    )]
    async fn musehub_deployment_list(
        &self,
        Parameters(input): Parameters<DeploymentListInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let pat = pat_context(&self.state).await?;
        if let Err(e) = require_scope(&pat, READ_SCOPE) {
            return Ok(domain_err(e));
        }
        let access = match repo_access(&self.state, &pat, &input.repo).await {
            Ok(a) => a,
            Err(e) => return Ok(domain_err(e)),
        };
        let environment_id = parse_uuid("environment_id", &input.environment_id)?;
        let limit = page_limit(input.page.limit);
        match deployment_list(&self.state.pool, access.repo_id, environment_id, limit).await {
            Ok((items, has_more)) => Ok(domain_ok_page(
                json!({ "items": items }),
                page_meta(has_more, None),
            )),
            Err(e) => Ok(domain_err(e)),
        }
    }
}

/// Exposes the contract tool router to the parent `server` module (macro-generated fn is private here).
pub(super) fn contract_tool_router() -> ToolRouter<ProductionMcpServer> {
    ProductionMcpServer::contract_tool_router()
}
