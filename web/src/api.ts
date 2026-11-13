const API_BASE = ''

async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${API_BASE}${path}`, {
    credentials: 'include',
    headers: { 'Content-Type': 'application/json', ...(init?.headers ?? {}) },
    ...init,
  })
  if (!response.ok) {
    const body = await response.json().catch(() => ({}))
    throw new Error((body as { message?: string }).message ?? response.statusText)
  }
  if (response.status === 204 || response.status === 205) {
    return undefined as T
  }
  const text = await response.text()
  if (!text) {
    return undefined as T
  }
  return JSON.parse(text) as T
}

export type Principal = { id: string; handle: string }
export type Repository = { id: string; slug: string; state: string; clone_url: string }
export type Issue = {
  id: string
  number: number
  title: string
  state: string
  labels?: string[]
  assignee_handle?: string | null
}
export type IssueListPage = {
  items: Issue[]
  next_cursor: string | null
  has_more: boolean
}
export type AcceptanceCriterion = {
  ordinal: number
  description: string
  verification_method: string
  required: boolean
}

export type IssuePlanSummary = {
  id: string
  version: number
  state: string
  body: string
}

export type IssueDetail = Issue & {
  body: string
  assignee_id?: string | null
  author_handle: string
  close_reason?: string | null
  current_revision_id?: string | null
  revision?: number | null
  criteria?: AcceptanceCriterion[]
  plan?: IssuePlanSummary | null
}

export type IssueDeliveryTrace = {
  issue_id: string
  issue_number: number
  pull_requests: { id: string; number: number; state: string; merged_oid?: string | null }[]
  pipeline_runs: { id: string; state: string; source_oid: string }[]
  deployments: { id: string; environment_slug: string; state: string }[]
  gaps: string[]
}

export type RepoEnvironment = {
  id: string
  slug: string
  name: string
  requires_approval: boolean
  row_version: number
}
export type DiscussionComment = {
  id: string
  body: string
  author_handle: string
  anchor?: unknown
  outdated: boolean
  resolved: boolean
  created_at: string
}
export type SshKey = { id: string; name: string; fingerprint: string; created_at: string }
export type PullRequestDetail = PullRequest & {
  row_version: number
  author_handle: string
  base_oid?: string
  head_oid?: string
}
export type AgentRun = {
  id: string
  adapter_kind: string
  state: string
  result_commit_oid?: string | null
  usage_tokens: number
  budget_unknown: boolean
}
export type PullRequest = {
  id: string
  number: number
  title: string
  state: string
  source_branch: string
  target_branch: string
  head_oid?: string | null
  issue_id?: string | null
  issue_number?: number | null
}

export type PrCiFailedJob = {
  id: string
  pipeline_run_id: string
  job_key: string
  state: string
  run_attempt_id: string
  log_api_path: string
  log_has_chunks: boolean
  log_excerpt?: string | null
}

export type PrCiFailure = {
  pr_id: string
  pr_number: number
  head_oid: string
  issue_id?: string | null
  checks_failed: { check_name: string; state: string; log_location_note: string }[]
  pipeline_run_id?: string | null
  pipeline_run_attempt_id?: string | null
  pipeline_state?: string | null
  pipeline_error_code?: string | null
  failed_jobs: PrCiFailedJob[]
  fix_rounds_used: number
  fix_rounds_max: number
  agent_fix_allowed: boolean
  agent_fix_denial_reason?: string | null
}

export type PrAgentFixRun = {
  id: string
  adapter_kind: string
  state: string
  pr_id: string
  fix_round: number
  head_oid: string
  usage_tokens: number
  budget_unknown: boolean
}

export type PipelineRun = {
  id: string
  source_oid: string
  workflow_name: string
  state: string
  run_attempt_id: string
  error_code?: string | null
}

export type PipelineJob = {
  id: string
  pipeline_run_id: string
  job_key: string
  state: string
  retry_allowed: boolean
}

export type DeploymentTrace = {
  deployment_id: string
  release_id: string
  pipeline_run_id: string
  commit_oid: string
  pr_id?: string | null
  pr_number?: number | null
  merged_oid?: string | null
  issue_id?: string | null
  issue_number?: number | null
  issue_revision_id?: string | null
  plan_accepted?: boolean | null
  agent_run_id?: string | null
  deployment_adapter: string
  adapter_is_mock: boolean
  release_oci_image_ref?: string | null
  applied_oci_digest?: string | null
  verified_source_oid?: string | null
  artifacts: { name: string; sha256: string; byte_length: number }[]
  gaps: string[]
}

export type Release = {
  id: string
  version: string
  tag_name: string
  commit_oid: string
  pipeline_run_id: string
  state: string
}

export type DeploymentHealthSummary = {
  status?: string
  error_code?: string
  detail?: string
  target?: string
  rollback?: boolean
  [key: string]: unknown
}

export type Deployment = {
  id: string
  environment_slug: string
  release_id: string
  state: string
  adapter_kind: string
  plan_hash?: string | null
  approval_required: boolean
  health_summary?: DeploymentHealthSummary | null
}

export type DeploymentSummary = {
  id: string
  environment_slug: string
  release_id: string
  state: string
  plan_hash?: string | null
  approval_required: boolean
  updated_at: string
}

export type DeploymentListResponse = {
  items: DeploymentSummary[]
  has_more: boolean
}

export type DeploymentApprovalResult = {
  deployment_id: string
  decision: string
  state: string
  plan_hash?: string
}

export async function login(handle: string, password: string) {
  return api<{ ok: true }>('/api/v1/auth/login', {
    method: 'POST',
    body: JSON.stringify({ handle, password }),
  })
}

export async function logout() {
  return api<void>('/api/v1/auth/logout', { method: 'POST' })
}

export async function me() {
  return api<Principal>('/api/v1/me')
}

export async function changePassword(currentPassword: string, newPassword: string) {
  return api<void>('/api/v1/auth/change-password', {
    method: 'POST',
    body: JSON.stringify({
      current_password: currentPassword,
      new_password: newPassword,
    }),
  })
}

export async function listRepos(org: string) {
  return api<Repository[]>(`/api/v1/orgs/${org}/repos`)
}

export async function createRepo(org: string, slug: string) {
  return api<Repository>(`/api/v1/orgs/${org}/repos`, {
    method: 'POST',
    body: JSON.stringify({ slug }),
  })
}

export async function listIssuesPage(
  org: string,
  repo: string,
  params?: { state?: string; label?: string; assignee?: string; cursor?: string; limit?: number },
) {
  const qs = new URLSearchParams()
  if (params?.state) qs.set('state', params.state)
  if (params?.label) qs.set('label', params.label)
  if (params?.assignee) qs.set('assignee', params.assignee)
  if (params?.cursor) qs.set('cursor', params.cursor)
  if (params?.limit != null) qs.set('limit', String(params.limit))
  const suffix = qs.toString() ? `?${qs.toString()}` : ''
  return api<IssueListPage>(`/api/v1/orgs/${org}/repos/${repo}/issues${suffix}`)
}

export async function listIssues(
  org: string,
  repo: string,
  params?: { state?: string; label?: string; assignee?: string; cursor?: string },
) {
  const page = await listIssuesPage(org, repo, params)
  return page.items
}

export async function getIssue(org: string, repo: string, issueNumber: number) {
  return api<IssueDetail>(`/api/v1/orgs/${org}/repos/${repo}/issues/${issueNumber}`)
}

export type IssueAssigneeOption = { handle: string }

export async function listIssueAssignees(org: string, repo: string) {
  return api<IssueAssigneeOption[]>(`/api/v1/orgs/${org}/repos/${repo}/issue-assignees`)
}

export async function patchIssue(
  org: string,
  repo: string,
  issueNumber: number,
  body: Record<string, unknown>,
) {
  return api<IssueDetail>(`/api/v1/orgs/${org}/repos/${repo}/issues/${issueNumber}`, {
    method: 'PATCH',
    body: JSON.stringify(body),
  })
}

export async function listIssueComments(org: string, repo: string, issueNumber: number) {
  return api<DiscussionComment[]>(
    `/api/v1/orgs/${org}/repos/${repo}/issues/${issueNumber}/comments`,
  )
}

export async function postIssueComment(org: string, repo: string, issueNumber: number, body: string) {
  return api<DiscussionComment>(
    `/api/v1/orgs/${org}/repos/${repo}/issues/${issueNumber}/comments`,
    { method: 'POST', body: JSON.stringify({ body }) },
  )
}

export async function createIssue(org: string, repo: string, title: string) {
  return api<Issue>(`/api/v1/orgs/${org}/repos/${repo}/issues`, {
    method: 'POST',
    body: JSON.stringify({ title }),
  })
}

export async function createPlan(org: string, repo: string, issueNumber: number, body: string) {
  return api('/api/v1/orgs/' + org + '/repos/' + repo + '/issues/' + issueNumber + '/plans', {
    method: 'POST',
    body: JSON.stringify({ body }),
  })
}

export async function acceptPlan(org: string, repo: string, issueNumber: number) {
  return api<void>(
    `/api/v1/orgs/${org}/repos/${repo}/issues/${issueNumber}/plans`,
    { method: 'PATCH' },
  )
}

export async function startAgentRun(
  org: string,
  repo: string,
  issueId: string,
  message: string,
) {
  return api<AgentRun>(`/api/v1/orgs/${org}/repos/${repo}/agent-runs`, {
    method: 'POST',
    body: JSON.stringify({ issue_id: issueId, message }),
  })
}

export async function getAgentRun(runId: string) {
  return api<AgentRun>(`/api/v1/agent-runs/${runId}`)
}

export async function cancelAgentRun(runId: string) {
  return api<void>(`/api/v1/agent-runs/${runId}/cancel`, { method: 'POST' })
}

export async function respondAgentRun(runId: string, message: string) {
  return api<{ id: string; state: string; message_sequence: number }>(
    `/api/v1/agent-runs/${runId}/respond`,
    { method: 'POST', body: JSON.stringify({ message }) },
  )
}

export async function replaceIssueCriteria(
  org: string,
  repo: string,
  issueNumber: number,
  expectedVersion: number,
  criteria: { description: string; verification_method?: string; required?: boolean }[],
) {
  return api<IssueDetail>(
    `/api/v1/orgs/${org}/repos/${repo}/issues/${issueNumber}/criteria`,
    {
      method: 'PUT',
      body: JSON.stringify({ expected_version: expectedVersion, criteria }),
    },
  )
}

export async function getIssueDeliveryTrace(org: string, repo: string, issueNumber: number) {
  return api<IssueDeliveryTrace>(
    `/api/v1/orgs/${org}/repos/${repo}/issues/${issueNumber}/delivery-trace`,
  )
}

export async function listRepoEnvironments(org: string, repo: string) {
  return api<RepoEnvironment[]>(`/api/v1/orgs/${org}/repos/${repo}/environments`)
}

export async function listPullRequests(org: string, repo: string) {
  return api<PullRequest[]>(`/api/v1/orgs/${org}/repos/${repo}/pull-requests`)
}

export async function getPullRequest(org: string, repo: string, number: number) {
  return api<PullRequestDetail>(
    `/api/v1/orgs/${org}/repos/${repo}/pull-requests/${number}`,
  )
}

export async function getPullRequestDiff(org: string, repo: string, number: number, path?: string) {
  const qs = path ? `?path=${encodeURIComponent(path)}` : ''
  return api<Record<string, unknown>>(
    `/api/v1/orgs/${org}/repos/${repo}/pull-requests/${number}/diff${qs}`,
  )
}

export async function submitPrReview(
  org: string,
  repo: string,
  number: number,
  decision: 'approve' | 'request_changes' | 'comment',
  body?: string,
) {
  return api<void>(`/api/v1/orgs/${org}/repos/${repo}/pull-requests/${number}/reviews`, {
    method: 'POST',
    body: JSON.stringify({ decision, body }),
  })
}

export async function listPrComments(org: string, repo: string, number: number) {
  return api<DiscussionComment[]>(
    `/api/v1/orgs/${org}/repos/${repo}/pull-requests/${number}/comments`,
  )
}

export async function postPrComment(
  org: string,
  repo: string,
  number: number,
  body: string,
  anchor?: Record<string, unknown>,
) {
  return api<DiscussionComment>(
    `/api/v1/orgs/${org}/repos/${repo}/pull-requests/${number}/comments`,
    { method: 'POST', body: JSON.stringify({ body, anchor }) },
  )
}

export async function syncPullRequest(org: string, repo: string, number: number, expectedRowVersion: number) {
  return api<PullRequest>(`/api/v1/orgs/${org}/repos/${repo}/pull-requests/${number}/sync`, {
    method: 'PATCH',
    body: JSON.stringify({ expected_row_version: expectedRowVersion }),
  })
}

export async function listGitCommits(org: string, repo: string, ref: string) {
  return api<{ items: { oid: string; summary: string; author: string }[] }>(
    `/api/v1/orgs/${org}/repos/${repo}/git/commits?ref=${encodeURIComponent(ref)}`,
  )
}

export async function listGitTree(org: string, repo: string, commit: string, path: string) {
  return api<{ items: { path: string; kind: string; oid: string; size?: number }[] }>(
    `/api/v1/orgs/${org}/repos/${repo}/git/tree?commit=${encodeURIComponent(commit)}&path=${encodeURIComponent(path)}`,
  )
}

export async function readGitBlob(
  org: string,
  repo: string,
  commit: string,
  path: string,
  startLine = 1,
) {
  return api<{ binary: boolean; text?: string | null; truncated?: boolean }>(
    `/api/v1/orgs/${org}/repos/${repo}/git/blob?commit=${encodeURIComponent(commit)}&path=${encodeURIComponent(path)}&start_line=${startLine}`,
  )
}

export async function listSshKeys() {
  return api<SshKey[]>('/api/v1/ssh-keys')
}

export async function createSshKey(name: string, publicKey: string) {
  return api<SshKey>('/api/v1/ssh-keys', {
    method: 'POST',
    body: JSON.stringify({ name, public_key: publicKey }),
  })
}

export async function revokeSshKey(keyId: string) {
  return api<void>(`/api/v1/ssh-keys/${keyId}`, { method: 'DELETE' })
}

export async function getPrCiFailure(org: string, repo: string, prNumber: number) {
  return api<PrCiFailure>(
    `/api/v1/orgs/${org}/repos/${repo}/pull-requests/${prNumber}/ci-failure`,
  )
}

export async function getPipelineJobLogs(
  org: string,
  repo: string,
  jobId: string,
  attemptId: string,
) {
  return api<{ text: string; complete: boolean; truncated: boolean; next_cursor?: number | null }>(
    `/api/v1/orgs/${org}/repos/${repo}/pipeline-jobs/${jobId}/logs?attempt_id=${encodeURIComponent(attemptId)}`,
  )
}

export async function startPrAgentFix(
  org: string,
  repo: string,
  prNumber: number,
  body?: { budget_tokens?: number },
) {
  return api<PrAgentFixRun>(
    `/api/v1/orgs/${org}/repos/${repo}/pull-requests/${prNumber}/agent-fix`,
    {
      method: 'POST',
      body: JSON.stringify(body ?? {}),
    },
  )
}

export async function createPullRequest(
  org: string,
  repo: string,
  title: string,
  sourceBranch: string,
  opts?: { issue_id?: string; agent_run_id?: string },
) {
  return api<PullRequest>(`/api/v1/orgs/${org}/repos/${repo}/pull-requests`, {
    method: 'POST',
    body: JSON.stringify({
      title,
      source_branch: sourceBranch,
      issue_id: opts?.issue_id,
      agent_run_id: opts?.agent_run_id,
    }),
  })
}

export async function startPipeline(org: string, repo: string, sourceOid: string) {
  return api<PipelineRun>(`/api/v1/orgs/${org}/repos/${repo}/pipeline-runs`, {
    method: 'POST',
    body: JSON.stringify({ source_oid: sourceOid, workflow_name: 'product-ci' }),
  })
}

export async function getPipeline(org: string, repo: string, runId: string) {
  return api<PipelineRun>(`/api/v1/orgs/${org}/repos/${repo}/pipeline-runs/${runId}`)
}

export async function cancelPipelineRun(org: string, repo: string, runId: string) {
  return api<PipelineRun>(
    `/api/v1/orgs/${org}/repos/${repo}/pipeline-runs/${runId}/cancel`,
    { method: 'POST' },
  )
}

export async function listPipelineJobs(org: string, repo: string, runId: string) {
  return api<PipelineJob[]>(
    `/api/v1/orgs/${org}/repos/${repo}/pipeline-runs/${runId}/jobs`,
  )
}

export async function retryPipelineJob(
  org: string,
  repo: string,
  jobId: string,
  attemptId: string,
) {
  return api<PipelineJob>(
    `/api/v1/orgs/${org}/repos/${repo}/pipeline-jobs/${jobId}/retry`,
    {
      method: 'POST',
      body: JSON.stringify({ attempt_id: attemptId }),
    },
  )
}

export async function getDeploymentTrace(
  org: string,
  repo: string,
  deploymentId: string,
) {
  return api<DeploymentTrace>(
    `/api/v1/orgs/${org}/repos/${repo}/trace/deployments/${deploymentId}`,
  )
}

export async function submitReview(
  org: string,
  repo: string,
  prNumber: number,
  decision: 'approve' | 'request_changes',
) {
  return api<void>(
    `/api/v1/orgs/${org}/repos/${repo}/pull-requests/${prNumber}/reviews`,
    {
      method: 'POST',
      body: JSON.stringify({ decision }),
    },
  )
}

export async function setPullRequestCheck(
  org: string,
  repo: string,
  prNumber: number,
  checkName: string,
  state: 'success' | 'failure',
  attemptId: string,
) {
  return api<void>(
    `/api/v1/orgs/${org}/repos/${repo}/pull-requests/${prNumber}/checks/${checkName}`,
    {
      method: 'PATCH',
      body: JSON.stringify({ state, attempt_id: attemptId }),
    },
  )
}

export async function mergePullRequest(org: string, repo: string, prNumber: number) {
  return api<{ head_oid: string }>(
    `/api/v1/orgs/${org}/repos/${repo}/pull-requests/${prNumber}/merge`,
    { method: 'POST' },
  )
}

export async function pollPipeline(
  org: string,
  repo: string,
  runId: string,
  timeoutMs = 120_000,
) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    const run = await getPipeline(org, repo, runId)
    if (run.state === 'succeeded' || run.state === 'failed' || run.state === 'canceled') {
      return run
    }
    await new Promise((r) => setTimeout(r, 500))
  }
  throw new Error('pipeline timeout')
}

export async function createRelease(
  org: string,
  repo: string,
  body: {
    version: string
    tag_name: string
    commit_oid: string
    pipeline_run_id: string
    pr_id?: string
  },
) {
  return api<Release>(`/api/v1/orgs/${org}/repos/${repo}/releases`, {
    method: 'POST',
    body: JSON.stringify(body),
  })
}

export async function publishRelease(org: string, repo: string, releaseId: string) {
  return api<Release>(`/api/v1/orgs/${org}/repos/${repo}/releases/${releaseId}/publish`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({}),
  })
}

export type OrgSettings = {
  display_name: string
  preferences: { default_collaborator_role: string }
  pipeline_executor: string
  agent_executor: string
  compose_adapter_mode: string
  product_profile: string
  require_pipeline_oci: boolean
  runner_dependencies: string[]
}

export type RepoGrant = {
  principal_id: string
  handle: string
  role: string
}

export type RunnerTokenMasked = {
  id: string
  name: string
  token_prefix: string
  scopes: string[]
  scope_mode: string
}

export type OrgMember = {
  principal_id: string
  handle: string
  org_role: string
  principal_status: string
  membership_status: string
}

export type PatMasked = {
  id: string
  name: string
  token_prefix: string
  scopes: string[]
  scope_mode: string
  expires_at: string
}

export type PatCreated = {
  id: string
  token: string
  note: string
}

export async function getOrgSettings(org: string) {
  return api<OrgSettings>(`/api/v1/orgs/${org}/settings`)
}

export async function patchOrgSettings(
  org: string,
  body: { display_name?: string; preferences?: { default_collaborator_role: string } },
) {
  return api<OrgSettings>(`/api/v1/orgs/${org}/settings`, {
    method: 'PATCH',
    body: JSON.stringify(body),
  })
}

export async function listRepoGrants(org: string, repo: string) {
  return api<RepoGrant[]>(`/api/v1/orgs/${org}/repos/${repo}/grants`)
}

export async function grantRepoCollaborator(
  org: string,
  repo: string,
  handle: string,
  role: string,
) {
  return api<void>(`/api/v1/orgs/${org}/repos/${repo}/collaborators`, {
    method: 'POST',
    body: JSON.stringify({ handle, role }),
  })
}

export async function listRunnerTokens(org: string) {
  return api<RunnerTokenMasked[]>(`/api/v1/orgs/${org}/runner-tokens`)
}

export type AuditEvent = {
  id: string
  actor_id: string
  action: string
  resource_type: string
  resource_id: string
  decision: string
  occurred_at: string
  metadata?: unknown
}

export type RunnerCapabilities = {
  pipeline_executor: string
  agent_executor: string
  deploy_executor: string
  lease_seconds: number
  scopes_accepted: string[]
  note: string
}

export async function listAuditEvents(org: string, limit = 25) {
  return api<AuditEvent[]>(
    `/api/v1/orgs/${org}/audit/events?limit=${encodeURIComponent(String(limit))}`,
  )
}

export async function getRunnerCapabilities() {
  return api<RunnerCapabilities>('/api/v1/runner/capabilities')
}

export async function createScopedPat(
  org: string,
  name: string,
  scopes: string[],
  opts: { scope_mode?: string; repo_ids?: string[]; runner_handle?: string },
) {
  return api<PatCreated>('/api/v1/tokens', {
    method: 'POST',
    body: JSON.stringify({
      name,
      org,
      scopes,
      scope_mode: opts.scope_mode ?? 'selected',
      repo_ids: opts.repo_ids,
      runner_handle: opts.runner_handle,
    }),
  })
}

export async function listOrgMembers(org: string) {
  return api<OrgMember[]>(`/api/v1/orgs/${org}/members`)
}

export async function inviteOrgMember(
  org: string,
  handle: string,
  password: string,
  role: string,
) {
  return api<void>(`/api/v1/orgs/${org}/members`, {
    method: 'POST',
    body: JSON.stringify({ handle, password, role }),
  })
}

export async function patchOrgMemberRole(org: string, handle: string, role: string) {
  return api<void>(`/api/v1/orgs/${org}/members/${encodeURIComponent(handle)}`, {
    method: 'PATCH',
    body: JSON.stringify({ role }),
  })
}

export async function disableOrgMember(org: string, handle: string) {
  return api<void>(
    `/api/v1/orgs/${org}/members/${encodeURIComponent(handle)}/disable`,
    { method: 'POST' },
  )
}

export async function revokeOrgMemberSessions(org: string, handle: string) {
  return api<{ sessions_revoked: number }>(
    `/api/v1/orgs/${org}/members/${encodeURIComponent(handle)}/revoke-sessions`,
    { method: 'POST' },
  )
}

export async function listMyTokens() {
  return api<PatMasked[]>('/api/v1/tokens')
}

export async function revokePat(tokenId: string) {
  return api<void>(`/api/v1/tokens/${tokenId}`, { method: 'DELETE' })
}

/** User PAT with default read/write scopes (shown once on create). */
export async function createUserPat(org: string, name: string) {
  return api<PatCreated>('/api/v1/tokens', {
    method: 'POST',
    body: JSON.stringify({ name, org }),
  })
}

export async function listDeployments(
  org: string,
  repo: string,
  options?: {
    limit?: number
    offset?: number
    state?: string
    environment_slug?: string
  },
) {
  const params = new URLSearchParams()
  if (options?.limit != null) {
    params.set('limit', String(options.limit))
  }
  if (options?.offset != null) {
    params.set('offset', String(options.offset))
  }
  if (options?.state) {
    params.set('state', options.state)
  }
  if (options?.environment_slug) {
    params.set('environment_slug', options.environment_slug)
  }
  const qs = params.toString()
  return api<DeploymentListResponse>(
    `/api/v1/orgs/${org}/repos/${repo}/deployments${qs ? `?${qs}` : ''}`,
  )
}

export async function getDeployment(org: string, repo: string, deploymentId: string) {
  return api<Deployment>(
    `/api/v1/orgs/${org}/repos/${repo}/deployments/${deploymentId}`,
  )
}

export async function approveDeployment(
  org: string,
  repo: string,
  deploymentId: string,
  expectedPlanHash: string,
  decision: 'approve' | 'reject',
  reason: string,
) {
  return api<DeploymentApprovalResult>(
    `/api/v1/orgs/${org}/repos/${repo}/deployments/${deploymentId}/approval`,
    {
      method: 'POST',
      body: JSON.stringify({
        expected_plan_hash: expectedPlanHash,
        decision,
        reason,
      }),
    },
  )
}

export async function startDeployment(
  org: string,
  repo: string,
  releaseId: string,
  healthTarget?: string,
  environmentSlug?: string,
  attemptId?: string,
) {
  const body: {
    release_id: string
    health_target?: string
    environment_slug?: string
    attempt_id?: string
  } = { release_id: releaseId }
  if (healthTarget) {
    body.health_target = healthTarget
  }
  if (environmentSlug) {
    body.environment_slug = environmentSlug
  }
  if (attemptId) {
    body.attempt_id = attemptId
  }
  return api<Deployment>(`/api/v1/orgs/${org}/repos/${repo}/deployments`, {
    method: 'POST',
    body: JSON.stringify(body),
  })
}

export async function rollbackDeployment(
  org: string,
  repo: string,
  targetDeploymentId: string,
  reason?: string,
  attemptId?: string,
) {
  const body: {
    target_deployment_id: string
    reason?: string
    attempt_id?: string
  } = { target_deployment_id: targetDeploymentId }
  if (reason) {
    body.reason = reason
  }
  if (attemptId) {
    body.attempt_id = attemptId
  }
  return api<Deployment>(`/api/v1/orgs/${org}/repos/${repo}/deployments/rollback`, {
    method: 'POST',
    body: JSON.stringify(body),
  })
}
