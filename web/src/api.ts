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
  agent_run_id?: string | null
  artifacts: { name: string; sha256: string; byte_length: number }[]
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
