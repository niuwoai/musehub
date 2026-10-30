# MuseHub MCP 接口契约

> 契约草案 v1｜2026-10-30。工具名称、字段和错误码是实现基线；尚未生成 SDK、实现 server 或完成互操作验收。

## 1. 接入与兼容范围

- 服务端入口 `/mcp`，使用官方 Rust SDK `rmcp`；工具名采用 `musehub_<domain>_<verb>`，避免标点对部分客户端的兼容问题。
- V1 支持 Streamable HTTP 的预配置 Bearer token 客户端；提供 `musehub mcp stdio` 本地代理，凭据从环境或操作系统凭据存储读取，不进入命令行参数和日志。
- 远程 OAuth 自动发现和登录不是 V1 承诺；要求这套交互的客户端需等待后续 OAuth 集成，不能宣称所有 MCP 客户端即插即用。
- 协议版本由 SDK 协商，M0 固定 SDK 与实测客户端版本；候选测试集包含 `2025-11-25` 与调研时最新规范版本，只有通过的版本才写入兼容声明。
- 长任务使用普通工具返回 run/job/deployment ID，再调用 get/logs；不依赖客户端支持实验或新版本 Tasks 扩展。

官方 SDK 提供 Rust MCP 实现；HTTP 与 stdio 的鉴权方式应分别处理。[Rust SDK](https://github.com/modelcontextprotocol/rust-sdk)；[MCP 授权规范](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization)

## 2. 通用参数和类型

| 类型/约定 | 定义 |
|---|---|
| ID | UUID 字符串；API 只接受不可歧义 ID，资源名通过 list/search 解析 |
| Repo | `org_id: ID, repository_id: ID`；两者必须匹配 |
| Page | `cursor?: string, limit?: integer`；默认 20、范围 1–100；游标 opaque、绑定查询与主体 |
| Write | `idempotency_key: string`；长度 8–128；所有有副作用工具必需 |
| Edit | Write + `expected_version: integer >= 1`；对象版本 CAS |
| OID | 40 个十六进制字符；V1 sha1；不接受模糊短 SHA 作为写操作依据 |
| Digest | `sha256:` + 64 个十六进制字符 |
| Ref | 合法 Git ref，由服务验证；不能操作平台内部 refs |
| Text | title ≤ 256 字符；body ≤ 64 KiB；comment ≤ 16 KiB；UTF-8 |
| Time | RFC3339 UTC；服务端时间为权威 |
| Patch | 显式允许字段；未提供=不变，nullable 字段传 null=清空；未知字段拒绝 |

所有 inputSchema 为 JSON Schema object，`additionalProperties:false`；每个嵌套对象同样明确字段。输出定义版本化 JSON Schema，`schema_version:1`。后续只做向后兼容新增可选字段；删字段/改变语义用新工具版本或兼容期。

read 工具无需 idempotency_key。写工具按 actor+tool+key 去重，同 key 不同规范化参数返回 `IDEMPOTENCY_CONFLICT`。去重不绕过权限：重放前再次授权，已失权者不能读取旧返回体。

## 3. 统一返回格式

MCP `CallToolResult.structuredContent`：

```json
{
  "schema_version": 1,
  "request_id": "opaque-request-id",
  "data": {
    "agent_run_id": "019a0000-0000-7000-8000-000000000001",
    "state": "queued",
    "version": 1
  },
  "page": null,
  "error": null
}
```

`data` 类型按具体工具定义；列表为 `{items:[...]}`，`page` 为 `{next_cursor:string|null,has_more:boolean}`。单对象 page=null。URL 使用实例配置的绝对链接，不能让客户端拼接内部文件路径。

领域错误返回 `isError:true`，structuredContent 中 data=null、error 包含 code/message/retryable/details；`content` 放简短文本摘要方便旧客户端。认证失败、JSON-RPC 参数错误与协议方法错误按传输/协议规范处理，不伪装成业务成功。

```json
{
  "schema_version": 1,
  "request_id": "opaque-request-id",
  "data": null,
  "page": null,
  "error": {
    "code": "STALE_HEAD",
    "message": "PR 的代码已变化，请刷新后重新检查。",
    "retryable": false,
    "details": {"refresh_tool": "musehub_pr_get"}
  }
}
```

错误 message 由服务端 i18n 生成；客户端应依赖 code，不解析自然语言。工具注解 readOnlyHint/destructiveHint/idempotentHint 仅是客户端提示，不承担授权。

## 4. 权限 scope

`repository:read/write`、`issue:read/write`、`pr:read/write/review/merge`、`agent:read/run`、`pipeline:read/run`、`release:read/write/publish`、`deployment:read/write/approve`、`audit:read`。

工具执行同时检查：token scope、主体资源授权、发起者/Agent 委托交集、当前策略、配额、expected version。`deployment:approve` 只授予环境审批者；Agent 不能因持有工具名就得到批准权。

## 5. 工具目录：仓库与上下文

下面每行的参数是完整业务参数，`Repo/Page/Write/Edit` 按第 2 节展开；未标 `?` 的字段必需。Resource 返回字段见第 10 节。

| 工具 | 输入 | data 输出 | scope / 说明 |
|---|---|---|---|
| musehub_capabilities_get | 无 | service_version、contract_version、supported_protocol_versions、tool_groups、limits | 已登录可读；只公布实际支持能力 |
| musehub_repository_list | org_id、Page、query?:string≤256 | items: RepositorySummary[] | repository:read；只返回有权仓库 |
| musehub_repository_get | Repo | Repository | repository:read |
| musehub_repository_create | org_id、Write、slug、description?:Text、default_branch?:Ref | Repository，state 可为 creating | repository:write + org admin；默认私有 |
| musehub_repository_tree | Repo、commit_oid:OID、path?:string、Page | items: TreeEntry[] | repository:read；不跟随 symlink |
| musehub_repository_read_file | Repo、commit_oid:OID、path:string、start_line?:int≥1、max_lines?:int1..500 | path、oid、text?、binary、truncated、next_line? | repository:read；最大 256 KiB |
| musehub_repository_search | Repo、commit_oid:OID、query:string1..256、path_prefix?:string、Page | items: SearchHit[]、truncated | repository:read；V1 字面文本搜索，无任意正则执行 |
| musehub_commit_compare | Repo、base_oid:OID、head_oid:OID、path?:string、Page | items: FileDiff[]、summary、truncated | repository:read；大 diff 分文件读取 |

## 6. 工具目录：Issue 与计划

| 工具 | 输入 | data 输出 | scope / 说明 |
|---|---|---|---|
| musehub_issue_list | Repo、Page、state?:IssueState、assignee_id?:ID、query?:string | items: IssueSummary[] | issue:read |
| musehub_issue_get | Repo、issue_id:ID | Issue、current_revision、criteria、delivery_summary | issue:read；讨论另分页 |
| musehub_issue_create | Repo、Write、title:Text、body:Text、criteria:CriterionInput[]、assignee_id?:ID | Issue | issue:write；criteria 1–50 项才允许 Agent 实现 |
| musehub_issue_update | Repo、issue_id、Edit、patch:{title?,body?,assignee_id?,state?,resolution_reason?} | Issue、新 revision（若内容变更） | issue:write；编辑内容复制旧验收条目到新 revision |
| musehub_issue_criteria_replace | Repo、issue_id、Edit、criteria:CriterionInput[] | Issue、新 revision、criteria | issue:write；不改写旧快照 |
| musehub_discussion_list | Repo、target:DiscussionTarget、Page | items: Comment[] | issue:read 或 pr:read |
| musehub_discussion_comment | Repo、Write、target:DiscussionTarget、body:Text、anchor?:DiffAnchor | Comment | issue:write 或 pr:write；anchor 仅 PR |
| musehub_discussion_resolve | Repo、comment_id:ID、Edit、resolved:boolean | Comment | pr:write；仅 PR 讨论，需作者或维护者权限；不得伪造 review 批准 |
| musehub_plan_get | Repo、plan_id:ID | Plan | issue:read |
| musehub_plan_create | Repo、Write、issue_revision_id:ID、body:Text、steps:PlanStep[] | Plan（draft） | issue:write |
| musehub_plan_accept | Repo、plan_id:ID、Edit、expected_issue_revision_id:ID | Plan（accepted） | issue:write；人工发起者；陈旧需求需显式新建计划 |
| musehub_delivery_get | Repo、issue_id:ID、Page | items: DeliveryLink[]、current_requirements_status | issue:read + 逐关联资源授权；无权项不泄露名称 |

IssueState=`open/in_progress/in_review/resolved/closed`。CriterionInput=`{description,verification_method,required:boolean}`；PlanStep=`{key,title,description,criterion_ordinals:integer[]}`。DiscussionTarget 是 `{kind:issue,id}` 或 `{kind:pr,id}` 的 oneOf，不能同时指定两种对象。

## 7. 工具目录：PR 与审查

| 工具 | 输入 | data 输出 | scope / 说明 |
|---|---|---|---|
| musehub_pr_list | Repo、Page、state?:PRState | items: PRSummary[] | pr:read |
| musehub_pr_get | Repo、pr_id:ID | PR、current_revision、checks、review_summary、mergeability | pr:read |
| musehub_pr_create | Repo、Write、title、body、source_branch:Ref、target_branch:Ref、expected_head_oid:OID、issue_revision_ids:ID[]、draft:boolean | PR | pr:write；必须已经存在远端分支 |
| musehub_pr_update | Repo、pr_id、Edit、patch:{title?,body?,draft?,state?:open/closed} | PR | pr:write；不接受 state=merged 绕过 merge 工具 |
| musehub_pr_diff | Repo、pr_id、pr_revision_id、path?:string、Page | items: FileDiff[]、truncated | pr:read；精确 revision |
| musehub_pr_review | Repo、pr_id、Write、expected_head_oid:OID、pr_revision_id:ID、decision:approve/request_changes/comment、body:Text | Review、review_summary | pr:review；独立审查者才计入批准 |
| musehub_pr_merge | Repo、pr_id、Edit、expected_head_oid:OID、expected_base_oid:OID、expected_candidate_oid:OID | PR、merge_commit_oid、git_operation_id | pr:merge + maintainer；V1 仅 merge commit |

DiffAnchor=`{pr_revision_id,path,side:left/right,line:int≥1}`；评论位置必须属于该 revision，代码变化后显示 outdated。PRState=`draft/open/merged/closed`。

合并候选不存在或检查未完成时返回 `CHECKS_PENDING`，附当前候选/流水线 ID；不能让客户端传 `force:true` 绕过。合并超时可返回 operation_id 与 pending 状态，由 get 对账确认。

## 8. 工具目录：Agent 与流水线

| 工具 | 输入 | data 输出 | scope / 说明 |
|---|---|---|---|
| musehub_agent_profile_list | org_id、Page | items: AgentProfileSummary[] | agent:read；包括能力与支持的预算限制 |
| musehub_agent_run_start | Repo、Write、issue_revision_id、plan_id、agent_profile_id、expected_base_oid:OID、pr_id?:ID、budget:Budget | AgentRun，通常 queued | agent:run + profile 使用授权 |
| musehub_agent_run_get | Repo、agent_run_id | AgentRun、progress、result?、usage | agent:read；明确任务结果与 PR 检查不同 |
| musehub_agent_run_messages | Repo、agent_run_id、Page | items: AgentMessage[] | agent:read；脱敏 |
| musehub_agent_run_respond | Repo、agent_run_id、Edit、message:Text | AgentRun、新 attempt/job 引用 | agent:run；仅 waiting_input；同需求版本内补充信息 |
| musehub_agent_run_cancel | Repo、agent_run_id、Write、reason:Text | AgentRun，可能 cancel_requested | agent:run；撤销任务凭据并终止工作空间 |
| musehub_pipeline_list | Repo、Page、state?:PipelineState、commit_oid?:OID | items: PipelineSummary[] | pipeline:read |
| musehub_pipeline_run | Repo、Write、workflow_path:string、workflow_source_oid:OID、target_oid:OID | PipelineRun | pipeline:run；可信配置来源与目标代码分开 |
| musehub_pipeline_get | Repo、pipeline_run_id | PipelineRun、jobs、checks | pipeline:read |
| musehub_job_logs | Repo、job_id、attempt_id、cursor?:string、max_bytes?:int1..262144 | text、next_cursor、complete、truncated | pipeline:read 或 agent:read；对象 owner 决定授权 |
| musehub_job_retry | Repo、job_id、Write | Job、新 attempt 编号或 queued | pipeline:run 或 agent:run；仅失败且策略允许；部署 job 禁止此通用重试 |
| musehub_pipeline_cancel | Repo、pipeline_run_id、Write、reason:Text | PipelineRun | pipeline:run；批量取消所属非终态 job |

Budget=`{time_limit_seconds:int60..7200,max_steps:int1..500,token_limit?:int≥1,cost_limit_microusd?:int≥1}`。上限可由组织收紧；不支持的硬预算返回 `UNSUPPORTED_BUDGET`，不静默忽略。

AgentRun 和 PipelineRun 状态详见产品需求和数据库。重复 start 使用同 key 返回同 run。第一次运行失败后要重新执行需新的 key，保留旧 run/attempt 证据。

PipelineState=`queued/running/succeeded/failed/cancel_requested/canceled`。agent_run_start 必须验证 plan 已接受、plan.issue_revision_id 与参数一致、该 revision 是本次明确选择的需求快照，且 expected_base_oid 仍为目标分支当前值；pr_id 存在时还需匹配该 PR 当前 head 和工作分支，禁止在其他 PR 的分支上继续写入。

## 9. 工具目录：Release 与部署

| 工具 | 输入 | data 输出 | scope / 说明 |
|---|---|---|---|
| musehub_release_list | Repo、Page | items: ReleaseSummary[] | release:read |
| musehub_release_get | Repo、release_id | Release、assets、linked_prs | release:read |
| musehub_release_create | Repo、Write、version:string、tag_name:Ref、commit_oid:OID、pipeline_run_id:ID、title、notes、artifact_ids:ID[] | Release（draft） | release:write；验证来源一致 |
| musehub_release_publish | Repo、release_id、Edit、expected_commit_oid:OID、expected_manifest_hash:string | Release、operation_id、state | release:publish；tag 固定、资产 ready、检查通过 |
| musehub_environment_list | Repo、Page | items: EnvironmentSummary[] | deployment:read |
| musehub_deployment_start | Repo、Write、environment_id、release_id、expected_environment_version:int、config_version:int | Deployment、plan_hash、approval_required | deployment:write；受保护环境先 awaiting_approval |
| musehub_deployment_approve | Repo、deployment_id、Edit、expected_plan_hash:string、decision:approve/reject、reason:Text | Deployment、Approval | deployment:approve；独立审批者，策略/计划必须匹配 |
| musehub_deployment_get | Repo、deployment_id | Deployment、desired、observed、health、approval_summary | deployment:read |
| musehub_deployment_list | Repo、environment_id、Page | items: DeploymentSummary[] | deployment:read |
| musehub_deployment_rollback | Repo、Write、environment_id、target_healthy_deployment_id、expected_environment_version:int、reason:Text | 新 Deployment、approval_required | deployment:write；执行同一审批与健康检查路径 |
| musehub_audit_list | org_id、repository_id?:ID、resource_type?:string、resource_id?:ID、Page | items: AuditSummary[] | audit:read；不返回密钥和原始源码 |

首版无 `secret.get`、任意 shell、任意 SQL、硬删除仓库或绕过审查工具。环境/Runner/secret 管理由受限 Web/管理 API 完成；本 MCP 面向研发交付。release_create 即计算草稿 manifest_hash，release_publish 校验该 hash 后冻结同一 manifest；任何草稿内容改变都递增行版本并重新计算 hash。

## 10. 输出资源字段

所有业务资源输出含 `id`、`created_at`、`updated_at`、`url`；可变实体附 `version`。Summary 是以下字段的明确子集，不包含正文、大 diff、原始配置。

| 资源 | 除公共字段外必须提供 |
|---|---|
| Repository | org_id、slug、description、default_branch、state、clone_urls、object_format、permissions |
| TreeEntry | path、kind、oid、size?；无内容 |
| SearchHit | path、line、snippet、commit_oid |
| FileDiff | old_path、new_path、change_type、binary、additions、deletions、hunks?、truncated |
| Issue | repository_id、number、state、author_id、assignee_id?、current_revision_id、title、body |
| Plan | issue_revision_id、state、body、steps、content_hash、accepted_by?、accepted_at? |
| Comment | target、author_id、body、anchor?、outdated、resolved |
| PR | repository_id、number、state、source_branch、target_branch、head_oid、base_oid、current_revision_id、merged_commit_oid? |
| Review | pr_id、pr_revision_id、reviewer_id、decision、body |
| DeliveryLink | issue_revision_id、relation、resource_type、resource_id、state、url、observed_at |
| AgentProfileSummary | name、adapter_kind、config_version、supported_budgets、enabled、available_to_caller |
| AgentRun | issue_revision_id、plan_id、profile_id、base_oid、work_branch、state、budget、usage、current_attempt_id?、result_commit_oid?、pr_id?、error? |
| AgentMessage | sequence、kind、body、timestamp |
| PipelineRun | workflow_source_oid、source_oid、state、trigger_kind、jobs_summary、trust_class |
| Job | kind、state、attempts、current_attempt_id?、failure_reason?、retry_allowed |
| Release | version_name、tag_name、commit_oid、pipeline_run_id、state、manifest_hash、title、notes |
| ReleaseAsset | name、digest、byte_length、media_type、download_url?、image_reference? |
| EnvironmentSummary | name、kind、version、desired_release_id?、observed_deployment_id?、observed_at? |
| Deployment | environment_id、release_id、state、plan_hash、operation_id、desired_digest、observed_digest?、observed_at?、previous_healthy_id?、rollback_of_id? |
| Approval | deployment_id、plan_hash、policy_version、decision、approver_id、expires_at |
| AuditSummary | actor_id、initiator_id?、action、resource_type、resource_id、decision、request_id、occurred_at |

避免 version 字段歧义：Release 的产品版本返回 `version_name`，公共 `version` 是行版本。敏感下载 URL 短时有效，授权至少与原资源一致；不能放在长期共享日志中。

## 11. 核心输入 schema 示例

以下示例描述合并所需全部字段；复用定义应由代码生成并与契约快照测试比较。

```json
{
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "org_id": {"type":"string","format":"uuid"},
    "repository_id": {"type":"string","format":"uuid"},
    "pr_id": {"type":"string","format":"uuid"},
    "idempotency_key": {"type":"string","minLength":8,"maxLength":128},
    "expected_version": {"type":"integer","minimum":1},
    "expected_head_oid": {"type":"string","pattern":"^[0-9a-f]{40}$"},
    "expected_base_oid": {"type":"string","pattern":"^[0-9a-f]{40}$"},
    "expected_candidate_oid": {"type":"string","pattern":"^[0-9a-f]{40}$"}
  },
  "required": ["org_id","repository_id","pr_id","idempotency_key","expected_version","expected_head_oid","expected_base_oid","expected_candidate_oid"]
}
```

## 12. 业务错误目录

| code | retryable | 含义与客户端动作 |
|---|---|---|
| NOT_FOUND | false | 资源不存在或对当前主体不可见，不泄露跨租户信息 |
| PERMISSION_DENIED | false | 当前操作没有权限；不自动换高权限凭据 |
| VALIDATION_FAILED | false | 参数、schema、ref 或业务约束不满足 |
| VERSION_CONFLICT | false | 刷新对象，重新确认意图后提交新请求 |
| STALE_HEAD / STALE_BASE | false | 代码已变化，重新检查当前 PR revision |
| CHECKS_PENDING / CHECKS_FAILED | false | 等待或修复检查；重复 merge 不会绕过 |
| APPROVAL_REQUIRED / APPROVAL_EXPIRED | false | 等待环境批准或更新计划后重新审批 |
| IDEMPOTENCY_CONFLICT | false | 同 key 不同参数；修正客户端，不覆盖原记录 |
| BUDGET_EXCEEDED / UNSUPPORTED_BUDGET | false | 限额到达或适配器不能保证该预算 |
| RUNNER_UNAVAILABLE | true | 无符合条件 Runner；附 retry_after_seconds |
| RATE_LIMITED | true | 附 retry_after_seconds；保持原幂等 key |
| OPERATION_IN_PROGRESS | true | 已接受操作未完成；返回 resource/operation ID，查询而非重新创建 |
| OUTCOME_UNKNOWN | false | 外部副作用结果不明；等待对账，禁止盲目重试 |
| UPSTREAM_UNAVAILABLE | true | 上游临时故障；只有已确定无副作用或同 key 幂等时才重试 |

HTTP 429/503 的重试提示与业务结果一致。实现不得把任意异常一律转换为 retryable=true。

## 13. Resources 与 Prompts

Resource URI：`musehub://repositories/{id}/commits/{oid}/files/{encoded-path}`、`musehub://issues/{id}/revisions/{n}`、`musehub://pull-requests/{id}/revisions/{n}`、`musehub://agent-runs/{id}/summary`、`musehub://deployments/{id}`。URI 不是授权凭据；每次读取做权限检查，路径编码防穿越。

Prompts：`plan_issue`、`review_pull_request`、`diagnose_pipeline_failure`、`draft_release_notes`。Prompt 只组装上下文，不能赋予新权限或自动发布。用户 Issue、仓库 README、评论和日志作为不可信内容处理，不可改变平台允许工具、凭据和审批规则。

## 14. 端到端调用与契约测试

正常链：repository_get → issue_create → plan_create → plan_accept → agent_run_start → agent_run_get/messages → pr_get/review → pr_merge → pipeline_get → release_create/publish → deployment_start/approve/get → delivery_get。

人工开发时由 Git CLI 写代码，musehub_pr_create 接续流程；无需模型参与每一步。Agent run 的创建 PR 是应用服务的幂等完成动作，外部 Agent 也可显式调用 pr_create，但不得产生重复 PR。

必测：工具发现/调用、输入与输出 schema、取消、分页越权、过期 token、相同 key 重试、不同 body 冲突、跨仓库 ID、权限中途撤销、旧 head 合并、部署未知结果、超长日志、提示注入、stdio/HTTP 两种接入。客户端至少覆盖一个本地 stdio 客户端与一个允许配置 HTTP Bearer token 的客户端，记录确切版本。
