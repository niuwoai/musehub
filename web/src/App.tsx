import { FormEvent, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

import {
  acceptPlan,
  AgentRun,
  cancelAgentRun,
  createIssue,
  createPlan,
  createPullRequest,
  createRepo,
  getAgentRun,
  Issue,
  listIssues,
  listPullRequests,
  listRepos,
  login,
  me,
  PullRequest,
  Repository,
  startAgentRun,
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
    const data = await listIssues(DEFAULT_ORG, slug)
    setIssues(data)
    const prs = await listPullRequests(DEFAULT_ORG, slug)
    setPulls(prs)
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
                    </li>
                  ))}
                </ul>
              ) : null}
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
}: {
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
    >
      {children}
    </div>
  )
}
