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
  if (response.status === 204) {
    return undefined as T
  }
  return (await response.json()) as T
}

export type Principal = { id: string; handle: string }
export type Repository = { id: string; slug: string; state: string; clone_url: string }
export type Issue = { id: string; number: number; title: string; state: string }
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
}

export type PipelineRun = {
  id: string
  source_oid: string
  workflow_name: string
  state: string
  run_attempt_id: string
  error_code?: string | null
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

export type Deployment = {
  id: string
  environment_slug: string
  release_id: string
  state: string
  adapter_kind: string
}

export async function login(handle: string, password: string) {
  return api<{ ok: true }>('/api/v1/auth/login', {
    method: 'POST',
    body: JSON.stringify({ handle, password }),
  })
}

export async function me() {
  return api<Principal>('/api/v1/me')
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

export async function listIssues(org: string, repo: string) {
  return api<Issue[]>(`/api/v1/orgs/${org}/repos/${repo}/issues`)
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

export async function listPullRequests(org: string, repo: string) {
  return api<PullRequest[]>(`/api/v1/orgs/${org}/repos/${repo}/pull-requests`)
}

export async function createPullRequest(
  org: string,
  repo: string,
  title: string,
  sourceBranch: string,
) {
  return api<PullRequest>(`/api/v1/orgs/${org}/repos/${repo}/pull-requests`, {
    method: 'POST',
    body: JSON.stringify({ title, source_branch: sourceBranch }),
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

export async function startDeployment(
  org: string,
  repo: string,
  releaseId: string,
  healthTarget?: string,
) {
  const body: { release_id: string; health_target?: string } = { release_id: releaseId }
  if (healthTarget) {
    body.health_target = healthTarget
  }
  return api<Deployment>(`/api/v1/orgs/${org}/repos/${repo}/deployments`, {
    method: 'POST',
    body: JSON.stringify(body),
  })
}
