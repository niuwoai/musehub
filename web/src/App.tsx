import { FormEvent, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'

import {
  acceptPlan,
  AgentRun,
  cancelAgentRun,
  createIssue,
  createPlan,
  createPullRequest,
  createRelease,
  createRepo,
  DeploymentTrace,
  getOrgSettings,
  patchOrgSettings,
  grantRepoCollaborator,
  listRepoGrants,
  listRunnerTokens,
  listAuditEvents,
  listOrgMembers,
  inviteOrgMember,
  patchOrgMemberRole,
  disableOrgMember,
  listMyTokens,
  createUserPat,
  createScopedPat,
  revokePat,
  logout,
  getRunnerCapabilities,
  type AuditEvent,
  type OrgMember,
  type PatMasked,
  type RunnerCapabilities,
  getAgentRun,
  getDeploymentTrace,
  Issue,
  listIssues,
  listPullRequests,
  listRepos,
  login,
  me,
  mergePullRequest,
  PipelineRun,
  pollPipeline,
  publishRelease,
  PullRequest,
  Repository,
  setPullRequestCheck,
  startAgentRun,
  startDeployment,
  startPipeline,
  submitReview,
} from './api'

const appVersion = import.meta.env.VITE_APP_VERSION
const DEFAULT_ORG = 'demo'

export default function App() {
  const { t, i18n } = useTranslation()
  const isChinese = i18n.language.startsWith('zh')
  const nextLanguage = isChinese ? 'en' : 'zh'

  const [session, setSession] = useState<string | null>(null)
  const [repos, setRepos] = useState<Repository[]>([])
  const [selectedRepo, setSelectedRepo] = useState<string | null>(null)
  const [issues, setIssues] = useState<Issue[]>([])
  const [pulls, setPulls] = useState<PullRequest[]>([])
  const [lastRun, setLastRun] = useState<AgentRun | null>(null)
  const [agentRunStarting, setAgentRunStarting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [loginForm, setLoginForm] = useState({ handle: 'admin', password: '' })
  const [pipelineRun, setPipelineRun] = useState<PipelineRun | null>(null)
  const [deploymentId, setDeploymentId] = useState<string | null>(null)
  const [trace, setTrace] = useState<DeploymentTrace | null>(null)
  const [settings, setSettings] = useState<Awaited<ReturnType<typeof getOrgSettings>> | null>(null)
  const [orgDisplayName, setOrgDisplayName] = useState('')
  const [grants, setGrants] = useState<{ handle: string; role: string }[]>([])
  const [grantHandle, setGrantHandle] = useState('')
  const [mintedPatFull, setMintedPatFull] = useState<string | null>(null)
  const sessionEpochRef = useRef(0)
  const [orgMembers, setOrgMembers] = useState<OrgMember[]>([])
  const [membersAdmin, setMembersAdmin] = useState(false)
  const [memberHandle, setMemberHandle] = useState('')
  const [memberPassword, setMemberPassword] = useState('')
  const [myTokens, setMyTokens] = useState<PatMasked[]>([])
  const [patName, setPatName] = useState('')
  const [patScopeMode, setPatScopeMode] = useState<'all_accessible' | 'selected'>('all_accessible')
  const [patRepoId, setPatRepoId] = useState('')
  const [iamBusy, setIamBusy] = useState(false)
  const [auditEvents, setAuditEvents] = useState<AuditEvent[]>([])
  const [runnerCaps, setRunnerCaps] = useState<RunnerCapabilities | null>(null)
  const [runnerTokens, setRunnerTokens] = useState<
    Awaited<ReturnType<typeof listRunnerTokens>>
  >([])

  useEffect(() => {
    me()
      .then((principal) => setSession(principal.handle))
      .catch(() => setSession(null))
  }, [])

  function bumpSessionEpoch() {
    sessionEpochRef.current += 1
    return sessionEpochRef.current
  }

  function sessionEpochActive(epoch: number) {
    return sessionEpochRef.current === epoch
  }

  useEffect(() => {
    const epoch = bumpSessionEpoch()
    if (!session) {
      setAuditEvents([])
      setRunnerCaps(null)
      setRunnerTokens([])
      setMyTokens([])
      setOrgMembers([])
      setMembersAdmin(false)
      setMintedPatFull(null)
      return
    }
    void (async () => {
      const pats = await listMyTokens().catch(() => [] as PatMasked[])
      if (!sessionEpochActive(epoch)) {
        return
      }
      setMyTokens(pats)
      const caps = await getRunnerCapabilities().catch(() => null)
      if (!sessionEpochActive(epoch)) {
        return
      }
      setRunnerCaps(caps)
      const audit = await listAuditEvents(DEFAULT_ORG, 30).catch(() => [] as AuditEvent[])
      if (!sessionEpochActive(epoch)) {
        return
      }
      setAuditEvents(audit)
      const runner = await listRunnerTokens(DEFAULT_ORG).catch(
        () => [] as Awaited<ReturnType<typeof listRunnerTokens>>,
      )
      if (!sessionEpochActive(epoch)) {
        return
      }
      setRunnerTokens(runner)
      try {
        const members = await listOrgMembers(DEFAULT_ORG)
        if (!sessionEpochActive(epoch)) {
          return
        }
        setOrgMembers(members)
        setMembersAdmin(true)
      } catch {
        if (!sessionEpochActive(epoch)) {
          return
        }
        setOrgMembers([])
        setMembersAdmin(false)
      }
    })()
  }, [session])

  async function refreshIamPanels() {
    const epoch = sessionEpochRef.current
    try {
      const pats = await listMyTokens()
      if (!sessionEpochActive(epoch)) {
        return
      }
      setMyTokens(pats)
    } catch (err) {
      if (sessionEpochActive(epoch)) {
        setError(err instanceof Error ? err.message : 'PAT list failed')
      }
      return
    }
    const audit = await listAuditEvents(DEFAULT_ORG, 40).catch(() => null)
    if (sessionEpochActive(epoch) && audit) {
      setAuditEvents(audit)
    }
    if (membersAdmin) {
      const members = await listOrgMembers(DEFAULT_ORG).catch(() => null)
      if (sessionEpochActive(epoch) && members) {
        setOrgMembers(members)
      }
    }
  }

  async function onLogout() {
    setError(null)
    bumpSessionEpoch()
    setMintedPatFull(null)
    try {
      await logout()
    } catch {
      /* cookie may already be cleared */
    }
    setSession(null)
  }

  async function refreshRepos() {
    const data = await listRepos(DEFAULT_ORG)
    setRepos(data)
  }

  async function onLogin(event: FormEvent) {
    event.preventDefault()
    setError(null)
    try {
      await login(loginForm.handle, loginForm.password)
      const principal = await me()
      bumpSessionEpoch()
      setMintedPatFull(null)
      setSession(principal.handle)
      await refreshRepos()
    } catch (err) {
      setError(err instanceof Error ? err.message : 'login failed')
    }
  }

  async function onCreateRepo() {
    const slug = `repo-${Date.now().toString(36)}`
    await createRepo(DEFAULT_ORG, slug)
    await refreshRepos()
  }

  async function loadIssues(slug: string) {
    setSelectedRepo(slug)
    setError(null)
    try {
      const prs = await listPullRequests(DEFAULT_ORG, slug)
      setPulls(prs)
      const data = await listIssues(DEFAULT_ORG, slug)
      setIssues(data)
      const s = await getOrgSettings(DEFAULT_ORG)
      setSettings(s)
      setOrgDisplayName(s.display_name)
      const g = await listRepoGrants(DEFAULT_ORG, slug)
      setGrants(g)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'failed to load repository')
    }
  }

  async function onCreateIssue() {
    if (!selectedRepo) return
    await createIssue(DEFAULT_ORG, selectedRepo, 'Slice issue')
    await loadIssues(selectedRepo)
  }

  async function onAcceptPlan(issue: Issue) {
    if (!selectedRepo) return
    await createPlan(DEFAULT_ORG, selectedRepo, issue.number, 'UI plan draft')
    await acceptPlan(DEFAULT_ORG, selectedRepo, issue.number)
  }

  async function pollRun(runId: string) {
    // Up to ~3 minutes — matches Playwright delivery-flow budgets on slow CI agents.
    for (let i = 0; i < 360; i++) {
      const run = await getAgentRun(runId)
      setLastRun(run)
      if (['succeeded', 'failed', 'canceled'].includes(run.state)) {
        return run
      }
      await new Promise((resolve) => setTimeout(resolve, 500))
    }
    throw new Error('agent run timeout')
  }

  async function onRunAgent(issue: Issue) {
    if (!selectedRepo) return
    setAgentRunStarting(true)
    try {
      const run = await startAgentRun(
        DEFAULT_ORG,
        selectedRepo,
        issue.id,
        `Implement: ${issue.title}`,
      )
      setLastRun(run)
      await pollRun(run.id)
      await loadIssues(selectedRepo)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'agent run failed')
    } finally {
      setAgentRunStarting(false)
    }
  }

  async function onCancelRun() {
    if (!lastRun) return
    await cancelAgentRun(lastRun.id)
    await pollRun(lastRun.id)
  }

  async function onCreatePrFromRun() {
    if (!selectedRepo || !lastRun?.result_commit_oid) return
    const branch = `agent/${lastRun.id}`
    await createPullRequest(
      DEFAULT_ORG,
      selectedRepo,
      `PR for ${branch}`,
      branch,
    )
    await loadIssues(selectedRepo)
  }

  const mergedPr = pulls.find((p) => p.state === 'merged' && p.head_oid)

  async function onApproveAndMerge(pr: PullRequest) {
    if (!selectedRepo) return
    setError(null)
    try {
      await submitReview(DEFAULT_ORG, selectedRepo, pr.number, 'approve')
      const attempt = crypto.randomUUID()
      await setPullRequestCheck(
        DEFAULT_ORG,
        selectedRepo,
        pr.number,
        'product-ci',
        'success',
        attempt,
      )
      await mergePullRequest(DEFAULT_ORG, selectedRepo, pr.number)
      await loadIssues(selectedRepo)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'merge failed')
    }
  }

  async function onRunDeliveryPipeline() {
    if (!selectedRepo || !mergedPr?.head_oid) return
    setError(null)
    try {
      const run = await startPipeline(DEFAULT_ORG, selectedRepo, mergedPr.head_oid)
      setPipelineRun(run)
      const done = await pollPipeline(DEFAULT_ORG, selectedRepo, run.id)
      setPipelineRun(done)
      if (done.state !== 'succeeded') {
        throw new Error(done.error_code ?? done.state)
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : 'pipeline failed')
    }
  }

  async function onReleaseAndDeploy() {
    if (!selectedRepo || !mergedPr?.head_oid || !pipelineRun || pipelineRun.state !== 'succeeded') {
      return
    }
    setError(null)
    try {
      const version = `0.0.${Date.now().toString(36)}`
      const rel = await createRelease(DEFAULT_ORG, selectedRepo, {
        version,
        tag_name: `v${version}`,
        commit_oid: mergedPr.head_oid,
        pipeline_run_id: pipelineRun.id,
        pr_id: mergedPr.id,
      })
      await publishRelease(DEFAULT_ORG, selectedRepo, rel.id)
      const dep = await startDeployment(DEFAULT_ORG, selectedRepo, rel.id)
      setDeploymentId(dep.id)
      const tv = await getDeploymentTrace(DEFAULT_ORG, selectedRepo, dep.id)
      setTrace(tv)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'release/deploy failed')
    }
  }

  async function onLoadTrace() {
    if (!selectedRepo || !deploymentId) return
    setTrace(await getDeploymentTrace(DEFAULT_ORG, selectedRepo, deploymentId))
  }

  return (
    <main
      style={{
        minHeight: '100vh',
        margin: 0,
        padding: '2rem',
        fontFamily:
          '-apple-system, BlinkMacSystemFont, "Segoe UI", "Noto Sans SC", Roboto, sans-serif',
        color: '#1f2328',
        background: '#f6f7f9',
      }}
    >
      <header style={{ marginBottom: '1.5rem' }}>
        <h1 style={{ margin: 0 }}>{t('app.name')}</h1>
        <p style={{ color: '#57606a' }}>
          {t('app.tagline', { version: appVersion })}
        </p>
        <button type="button" onClick={() => void i18n.changeLanguage(nextLanguage)}>
          {t('app.switchLanguage')}
        </button>
      </header>

      {session ? (
        <section>
          {error ? (
            <p role="alert" data-testid="app-alert">
              {error}
            </p>
          ) : null}
          <p>
            {t('app.signedInAs')}{' '}
            <strong data-testid="session-handle">{session}</strong>
            <button type="button" data-testid="logout" onClick={() => void onLogout()}>
              Log out
            </button>
          </p>
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: '1rem', marginBottom: '1rem' }}>
            <Panel style={{ flex: '1 1 18rem' }} data-testid="audit-panel">
              <h3>Audit (read-only)</h3>
              {auditEvents.length === 0 ? (
                <p>No audit events yet.</p>
              ) : (
                <ul style={{ maxHeight: '12rem', overflow: 'auto', margin: 0, paddingLeft: '1.2rem' }}>
                  {auditEvents.map((ev) => (
                    <li key={ev.id}>
                      <code>{ev.occurred_at.slice(0, 19)}</code> {ev.action}{' '}
                      <span>
                        {ev.resource_type}/{ev.resource_id.slice(0, 8)}…
                      </span>{' '}
                      · {ev.decision}
                    </li>
                  ))}
                </ul>
              )}
            </Panel>
            <Panel style={{ flex: '1 1 18rem' }} data-testid="org-members">
              <h3>Organization members</h3>
              {!membersAdmin ? (
                <p>Member admin requires org owner/admin.</p>
              ) : (
                <>
                  <ul data-testid="org-members-list" style={{ paddingLeft: '1.2rem' }}>
                    {orgMembers.map((m) => (
                      <li key={m.principal_id}>
                        {m.handle} · {m.org_role} · {m.principal_status}
                        {m.principal_status === 'active' && m.handle !== session ? (
                          <>
                            {' '}
                            <select
                              aria-label={`role-${m.handle}`}
                              value={m.org_role}
                              onChange={(e) => {
                                void (async () => {
                                  setError(null)
                                  try {
                                    await patchOrgMemberRole(
                                      DEFAULT_ORG,
                                      m.handle,
                                      e.target.value,
                                    )
                                    await refreshIamPanels()
                                  } catch (err) {
                                    setError(
                                      err instanceof Error
                                        ? err.message
                                        : 'role update failed',
                                    )
                                  }
                                })()
                              }}
                            >
                              <option value="member">member</option>
                              <option value="admin">admin</option>
                              <option value="owner">owner</option>
                            </select>
                            <button
                              type="button"
                              onClick={() => {
                                void (async () => {
                                  setError(null)
                                  try {
                                    await disableOrgMember(DEFAULT_ORG, m.handle)
                                    await refreshIamPanels()
                                  } catch (err) {
                                    setError(
                                      err instanceof Error ? err.message : 'disable failed',
                                    )
                                  }
                                })()
                              }}
                            >
                              Disable
                            </button>
                          </>
                        ) : null}
                      </li>
                    ))}
                  </ul>
                  <div>
                    <input
                      data-testid="member-handle"
                      placeholder="handle"
                      value={memberHandle}
                      onChange={(e) => setMemberHandle(e.target.value)}
                    />
                    <input
                      data-testid="member-password"
                      type="password"
                      placeholder="initial password"
                      value={memberPassword}
                      onChange={(e) => setMemberPassword(e.target.value)}
                    />
                    <button
                      type="button"
                      data-testid="invite-member"
                      disabled={iamBusy}
                      onClick={() => {
                        void (async () => {
                          setError(null)
                          setIamBusy(true)
                          try {
                            await inviteOrgMember(
                              DEFAULT_ORG,
                              memberHandle.trim(),
                              memberPassword,
                              'member',
                            )
                            setMemberHandle('')
                            setMemberPassword('')
                            await refreshIamPanels()
                          } catch (err) {
                            setError(
                              err instanceof Error ? err.message : 'member invite failed',
                            )
                          } finally {
                            setIamBusy(false)
                          }
                        })()
                      }}
                    >
                      Add member
                    </button>
                  </div>
                </>
              )}
            </Panel>
            <Panel style={{ flex: '1 1 18rem' }} data-testid="pat-management">
              <h3>Personal access tokens</h3>
              {mintedPatFull ? (
                <p data-testid="pat-minted-once">
                  Copy now (shown once): <code>{mintedPatFull}</code>
                </p>
              ) : (
                <p>Minted tokens are shown once at creation only.</p>
              )}
              <ul data-testid="pat-masked-list">
                {myTokens.map((t) => (
                  <li key={t.id}>
                    {t.name}: <code>{t.token_prefix}…</code> ({t.scope_mode}, exp{' '}
                    {t.expires_at.slice(0, 10)})
                    <button
                      type="button"
                      data-testid={`revoke-pat-${t.id}`}
                      onClick={() => {
                        void (async () => {
                          const epoch = sessionEpochRef.current
                          setError(null)
                          try {
                            await revokePat(t.id)
                            if (!sessionEpochActive(epoch)) {
                              return
                            }
                            setMintedPatFull(null)
                            await refreshIamPanels()
                          } catch (err) {
                            if (sessionEpochActive(epoch)) {
                              setError(
                                err instanceof Error ? err.message : 'PAT revoke failed',
                              )
                            }
                          }
                        })()
                      }}
                    >
                      Revoke
                    </button>
                  </li>
                ))}
              </ul>
              <input
                data-testid="pat-name"
                placeholder="token name"
                value={patName}
                onChange={(e) => setPatName(e.target.value)}
              />
              <label>
                Scope mode{' '}
                <select
                  data-testid="pat-scope-mode"
                  value={patScopeMode}
                  onChange={(e) =>
                    setPatScopeMode(e.target.value as 'all_accessible' | 'selected')
                  }
                >
                  <option value="all_accessible">all_accessible</option>
                  <option value="selected">selected (one repo)</option>
                </select>
              </label>
              {patScopeMode === 'selected' ? (
                <label>
                  Repo{' '}
                  <select
                    data-testid="pat-repo-id"
                    value={patRepoId}
                    onChange={(e) => setPatRepoId(e.target.value)}
                  >
                    <option value="">— select —</option>
                    {repos.map((r) => (
                      <option key={r.id} value={r.id}>
                        {r.slug}
                      </option>
                    ))}
                  </select>
                </label>
              ) : null}
              <button
                type="button"
                data-testid="create-pat"
                disabled={iamBusy}
                onClick={() => {
                  void (async () => {
                    const epoch = sessionEpochRef.current
                    setError(null)
                    setIamBusy(true)
                    const name = patName.trim() || 'console-pat'
                    try {
                      let created
                      if (patScopeMode === 'selected') {
                        if (!patRepoId) {
                          setError('select a repo for selected-scope PAT')
                          return
                        }
                        created = await createScopedPat(DEFAULT_ORG, name, ['repo:read'], {
                          scope_mode: 'selected',
                          repo_ids: [patRepoId],
                        })
                      } else {
                        created = await createUserPat(DEFAULT_ORG, name)
                      }
                      if (!sessionEpochActive(epoch)) {
                        return
                      }
                      setMintedPatFull(created.token)
                      setPatName('')
                      await refreshIamPanels()
                    } catch (err) {
                      if (sessionEpochActive(epoch)) {
                        setError(err instanceof Error ? err.message : 'PAT create failed')
                      }
                    } finally {
                      if (sessionEpochActive(epoch)) {
                        setIamBusy(false)
                      }
                    }
                  })()
                }}
              >
                Create PAT
              </button>
            </Panel>
            <Panel style={{ flex: '1 1 18rem' }} data-testid="runner-status-panel">
              <h3>Runner status</h3>
              {runnerCaps ? (
                <>
                  <p data-testid="runner-executors">
                    pipeline {runnerCaps.pipeline_executor} · agent {runnerCaps.agent_executor} ·
                    deploy {runnerCaps.deploy_executor}
                  </p>
                  <p>lease {runnerCaps.lease_seconds}s · scopes: {runnerCaps.scopes_accepted.join(', ')}</p>
                  <p style={{ color: '#57606a', fontSize: '0.9rem' }}>{runnerCaps.note}</p>
                </>
              ) : (
                <p>Unable to load runner capabilities.</p>
              )}
              {runnerTokens.length > 0 ? (
                <ul>
                  {runnerTokens.map((tok) => (
                    <li key={tok.id}>
                      {tok.name}: <code>{tok.token_prefix}…</code> ({tok.scope_mode})
                    </li>
                  ))}
                </ul>
              ) : (
                <p>No runner tokens (masked list only).</p>
              )}
            </Panel>
          </div>
          <button type="button" onClick={() => void onCreateRepo()}>
            {t('app.createRepo')}
          </button>
          <ul>
            {repos.map((repo) => (
              <li key={repo.id}>
                <button type="button" onClick={() => void loadIssues(repo.slug)}>
                  {repo.slug}
                </button>
                <code style={{ marginLeft: '0.5rem' }}>{repo.clone_url}</code>
              </li>
            ))}
          </ul>
          {selectedRepo ? (
            <Panel style={{ marginTop: '1rem' }}>
              <h2>{selectedRepo}</h2>
              {settings ? (
                <div data-testid="org-settings">
                  <label>
                    Org display name
                    <input
                      data-testid="org-display-name"
                      value={orgDisplayName}
                      onChange={(e) => setOrgDisplayName(e.target.value)}
                    />
                  </label>
                  <button
                    type="button"
                    data-testid="save-org-settings"
                    onClick={async () => {
                      const next = await patchOrgSettings(DEFAULT_ORG, {
                        display_name: orgDisplayName,
                        preferences: {
                          default_collaborator_role:
                            settings.preferences.default_collaborator_role || 'reader',
                        },
                      })
                      setSettings(next)
                    }}
                  >
                    Save org settings
                  </button>
                  <p>
                    pipeline:                     {settings.pipeline_executor} · agent:{' '}
                    {settings.agent_executor} · compose:{' '}
                    {settings.compose_adapter_mode} ({settings.product_profile})
                  </p>
                </div>
              ) : null}
              <div data-testid="repo-grants" style={{ marginBottom: '1rem' }}>
                <strong>Repo grants</strong>
                <ul>
                  {grants.map((g) => (
                    <li key={g.handle}>
                      {g.handle} ({g.role})
                    </li>
                  ))}
                </ul>
                <input
                  placeholder="handle"
                  value={grantHandle}
                  onChange={(e) => setGrantHandle(e.target.value)}
                />
                <button
                  type="button"
                  onClick={async () => {
                    if (!selectedRepo || !grantHandle.trim()) return
                    await grantRepoCollaborator(
                      DEFAULT_ORG,
                      selectedRepo,
                      grantHandle.trim(),
                      'reader',
                    )
                    setGrants(await listRepoGrants(DEFAULT_ORG, selectedRepo))
                    setGrantHandle('')
                  }}
                >
                  Grant reader
                </button>
              </div>
              <button
                type="button"
                data-testid="refresh-repo"
                onClick={() => void loadIssues(selectedRepo)}
              >
                {t('app.refreshRepo')}
              </button>
              <button type="button" onClick={() => void onCreateIssue()}>
                {t('app.createIssue')}
              </button>
              <ul>
                {issues.map((issue) => (
                  <li key={issue.id}>
                    #{issue.number} {issue.title}{' '}
                    <button type="button" onClick={() => void onAcceptPlan(issue)}>
                      {t('app.acceptPlan')}
                    </button>
                    <button type="button" onClick={() => void onRunAgent(issue)}>
                      {t('app.runReferenceAgent')}
                    </button>
                  </li>
                ))}
              </ul>
              {lastRun || agentRunStarting ? (
                <div data-testid="last-agent-run">
                  <p>
                    {agentRunStarting && !lastRun
                      ? t('app.agentStarting', { defaultValue: 'starting agent run…' })
                      : `${lastRun!.adapter_kind} / ${lastRun!.state}`}
                    {lastRun?.result_commit_oid
                      ? ` → ${lastRun.result_commit_oid.slice(0, 7)}`
                      : ''}
                    {lastRun?.budget_unknown ? ' (budget unknown)' : ''}
                  </p>
                  {lastRun &&
                  (lastRun.state === 'running' || lastRun.state === 'queued') ? (
                    <button type="button" onClick={() => void onCancelRun()}>
                      {t('app.cancelAgent')}
                    </button>
                  ) : null}
                  {lastRun?.state === 'succeeded' ? (
                    <button type="button" onClick={() => void onCreatePrFromRun()}>
                      {t('app.createPullRequest')}
                    </button>
                  ) : null}
                </div>
              ) : null}
              {pulls.length > 0 ? (
                <ul data-testid="pull-request-list">
                  {pulls.map((pr) => (
                    <li key={pr.id}>
                      #{pr.number} {pr.title} ({pr.state}) {pr.source_branch} →{' '}
                      {pr.target_branch}
                      {pr.state === 'open' ? (
                        <button type="button" onClick={() => void onApproveAndMerge(pr)}>
                          {t('app.mergeShortcut')}
                        </button>
                      ) : null}
                    </li>
                  ))}
                </ul>
              ) : null}
              <Panel style={{ marginTop: '1rem' }} data-testid="delivery-panel">
                <h3>{t('app.deliveryTitle')}</h3>
                <p style={{ color: '#9a6700' }} data-testid="delivery-mode-note">
                  {t('app.deliveryModeNote')}
                </p>
                {mergedPr?.head_oid ? (
                  <>
                    <p>
                      main @{mergedPr.head_oid.slice(0, 7)} (PR #{mergedPr.number})
                    </p>
                    {pipelineRun ? (
                      <p data-testid="pipeline-state">
                        pipeline {pipelineRun.id.slice(0, 8)}… {pipelineRun.state}
                      </p>
                    ) : null}
                    <button type="button" onClick={() => void onRunDeliveryPipeline()}>
                      {t('app.runPipelineOnMain')}
                    </button>
                    <button type="button" onClick={() => void onReleaseAndDeploy()}>
                      {t('app.releaseAndDeployMock')}
                    </button>
                    {deploymentId ? (
                      <button type="button" onClick={() => void onLoadTrace()}>
                        {t('app.viewTrace')}
                      </button>
                    ) : null}
                    {trace ? (
                      <pre data-testid="delivery-trace" style={{ fontSize: '0.85rem' }}>
                        {JSON.stringify(
                          {
                            adapter: trace.deployment_adapter,
                            mock: trace.adapter_is_mock,
                            release_oci_image_ref: trace.release_oci_image_ref,
                            applied_oci_digest: trace.applied_oci_digest,
                            verified_source_oid: trace.verified_source_oid,
                            commit_oid: trace.commit_oid,
                            gaps: trace.gaps,
                            artifacts: trace.artifacts.length,
                          },
                          null,
                          2,
                        )}
                      </pre>
                    ) : null}
                    {trace && trace.gaps.length > 0 ? (
                      <p role="status">{t('app.traceGaps')}: {trace.gaps.join(', ')}</p>
                    ) : null}
                  </>
                ) : (
                  <p>{t('app.noMergedPr')}</p>
                )}
              </Panel>
            </Panel>
          ) : null}
        </section>
      ) : (
        <form onSubmit={(e) => void onLogin(e)} style={{ maxWidth: '20rem' }}>
          <label>
            {t('app.loginHandle')}
            <input
              data-testid="login-handle-input"
              value={loginForm.handle}
              onChange={(e) =>
                setLoginForm({ ...loginForm, handle: e.target.value })
              }
            />
          </label>
          <label>
            {t('app.loginPassword')}
            <input
              type="password"
              value={loginForm.password}
              onChange={(e) =>
                setLoginForm({ ...loginForm, password: e.target.value })
              }
            />
          </label>
          {error ? <p role="alert">{error}</p> : null}
          <button type="submit">{t('app.login')}</button>
        </form>
      )}
    </main>
  )
}

function Panel({
  children,
  style,
  ...rest
}: React.HTMLAttributes<HTMLDivElement> & {
  children: React.ReactNode
  style?: React.CSSProperties
}) {
  return (
    <div
      style={{
        background: '#fff',
        border: '1px solid #d8dee4',
        borderRadius: '0.5rem',
        padding: '1rem',
        ...style,
      }}
      {...rest}
    >
      {children}
    </div>
  )
}
