import { FormEvent, useEffect, useState } from 'react'
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
  grantRepoCollaborator,
  listRepoGrants,
  listRunnerTokens,
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
  const [error, setError] = useState<string | null>(null)
  const [loginForm, setLoginForm] = useState({ handle: 'admin', password: '' })
  const [pipelineRun, setPipelineRun] = useState<PipelineRun | null>(null)
  const [deploymentId, setDeploymentId] = useState<string | null>(null)
  const [trace, setTrace] = useState<DeploymentTrace | null>(null)
  const [settings, setSettings] = useState<{ pipeline_executor: string } | null>(null)
  const [grants, setGrants] = useState<{ handle: string; role: string }[]>([])
  const [grantHandle, setGrantHandle] = useState('')
  const [mintedTokenPrefix, setMintedTokenPrefix] = useState<string | null>(null)

  useEffect(() => {
    me()
      .then((principal) => setSession(principal.handle))
      .catch(() => setSession(null))
  }, [])

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
      setSettings(await getOrgSettings(DEFAULT_ORG))
      const g = await listRepoGrants(DEFAULT_ORG, slug)
      setGrants(g)
      const masked = await listRunnerTokens(DEFAULT_ORG)
      setMintedTokenPrefix(masked[0]?.token_prefix ?? null)
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
    for (let i = 0; i < 120; i++) {
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
    const run = await startAgentRun(
      DEFAULT_ORG,
      selectedRepo,
      issue.id,
      `Implement: ${issue.title}`,
    )
    setLastRun(run)
    await pollRun(run.id)
    if (selectedRepo) await loadIssues(selectedRepo)
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
          <p>
            {t('app.signedInAs')} <strong>{session}</strong>
          </p>
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
                <p data-testid="org-settings">
                  executor: {settings.pipeline_executor}
                  {mintedTokenPrefix ? ` · runner token prefix: ${mintedTokenPrefix}` : ''}
                </p>
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
              {lastRun ? (
                <div data-testid="last-agent-run">
                  <p>
                    {lastRun.adapter_kind} / {lastRun.state}
                    {lastRun.result_commit_oid
                      ? ` → ${lastRun.result_commit_oid.slice(0, 7)}`
                      : ''}
                    {lastRun.budget_unknown ? ' (budget unknown)' : ''}
                  </p>
                  {lastRun.state === 'running' || lastRun.state === 'queued' ? (
                    <button type="button" onClick={() => void onCancelRun()}>
                      {t('app.cancelAgent')}
                    </button>
                  ) : null}
                  {lastRun.state === 'succeeded' ? (
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
