import { FormEvent, useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { createPullRequest } from '../api'
import { Panel } from '../components/Panel'
import { useWorkspace } from '../app/WorkspaceProvider'
import { DEFAULT_ORG, paths } from '../app/paths'

export function RepoPullsPage() {
  const { org = DEFAULT_ORG } = useParams<{ org: string }>()
  const navigate = useNavigate()
  const [title, setTitle] = useState('')
  const [sourceBranch, setSourceBranch] = useState('')
  const [targetBranch, setTargetBranch] = useState('main')
  const [createBusy, setCreateBusy] = useState(false)
  const [createError, setCreateError] = useState<string | null>(null)
  const {
    t,
    selectedRepo,
    pulls,
    prCiFailure,
    selectedPrNumber,
    agentRunStarting,
    onInspectPrCi,
    onApproveAndMerge,
    onStartPrAgentFix,
    loadIssues,
  } = useWorkspace()

  if (!selectedRepo) {
    return <p role="status">{t('repo.selectFirst')}</p>
  }
  const repoSlug = selectedRepo

  async function submitPullRequest(event: FormEvent) {
    event.preventDefault()
    setCreateBusy(true)
    setCreateError(null)
    try {
      const created = await createPullRequest(org, repoSlug, title.trim(), sourceBranch.trim(), {
        target_branch: targetBranch.trim(),
      })
      await loadIssues(repoSlug)
      navigate(paths.repoPull(org, repoSlug, created.number))
    } catch (reason) {
      setCreateError(reason instanceof Error ? reason.message : t('app.createPullRequestFailed'))
    } finally {
      setCreateBusy(false)
    }
  }

  return (
    <Panel data-testid="repo-pulls-page">
      <h2>{t('nav.repoPulls')}</h2>
      <details data-testid="create-pull-request">
        <summary>{t('app.createPullRequest')}</summary>
        <p>{t('app.pullRequestNeedsPushedBranch')}</p>
        <form onSubmit={(event) => void submitPullRequest(event)}>
          <label>
            {t('app.pullRequestTitle')}
            <input
              data-testid="pull-create-title"
              required
              value={title}
              onChange={(event) => setTitle(event.target.value)}
            />
          </label>
          <label>
            {t('app.pullRequestSourceBranch')}
            <input
              data-testid="pull-create-source-branch"
              required
              value={sourceBranch}
              onChange={(event) => setSourceBranch(event.target.value)}
            />
          </label>
          <label>
            {t('app.pullRequestTargetBranch')}
            <input
              data-testid="pull-create-target-branch"
              required
              value={targetBranch}
              onChange={(event) => setTargetBranch(event.target.value)}
            />
          </label>
          <button type="submit" disabled={createBusy} data-testid="pull-create-submit">
            {createBusy ? t('common.loading') : t('app.createPullRequest')}
          </button>
        </form>
        {createError ? <p role="alert">{createError}</p> : null}
      </details>
      <ul data-testid="pull-request-list">
        {pulls.map((pr) => (
          <li key={pr.id} data-testid={`pull-row-${pr.number}`}>
            <Link to={paths.repoPull(org, selectedRepo, pr.number)} data-testid={`pull-link-${pr.number}`}>
              #{pr.number} {pr.title}
            </Link>{' '}
            ({pr.state}) {pr.source_branch} → {pr.target_branch}
            {pr.issue_number != null ? (
              <>
                {' '}
                <Link
                  to={paths.repoIssue(org, selectedRepo, pr.issue_number)}
                  data-testid={`pull-issue-link-${pr.number}`}
                >
                  issue #{pr.issue_number}
                </Link>
              </>
            ) : null}
            {pr.state === 'open' ? (
              <>
                <button type="button" onClick={() => void onInspectPrCi(pr)}>
                  {t('app.prCiInspect')}
                </button>
                <button type="button" onClick={() => void onApproveAndMerge(pr)}>
                  {t('app.mergeShortcut')}
                </button>
              </>
            ) : null}
          </li>
        ))}
      </ul>
      {pulls.length === 0 ? <p>{t('repo.noPulls')}</p> : null}
      {prCiFailure && selectedPrNumber != null ? (
        <div data-testid="pr-ci-failure-panel">
          <h3>{t('app.prCiFailureTitle', { number: selectedPrNumber })}</h3>
          <p data-testid="pr-ci-head-oid">head {prCiFailure.head_oid.slice(0, 7)}</p>
          <p data-testid="pr-ci-fix-rounds">
            {t('app.prCiFixRounds', {
              used: prCiFailure.fix_rounds_used,
              max: prCiFailure.fix_rounds_max,
            })}
          </p>
          {prCiFailure.pipeline_error_code || prCiFailure.pipeline_state ? (
            <p data-testid="pr-ci-pipeline-summary">
              pipeline {prCiFailure.pipeline_state ?? '—'}
              {prCiFailure.pipeline_error_code ? ` (${prCiFailure.pipeline_error_code})` : ''}
            </p>
          ) : null}
          {prCiFailure.checks_failed.length > 0 ? (
            <ul data-testid="pr-ci-check-list">
              {prCiFailure.checks_failed.map((c) => (
                <li key={c.check_name}>
                  {c.check_name}: {c.state}
                  <span data-testid={`pr-ci-check-log-note-${c.check_name}`}>
                    {' '}
                    — {c.log_location_note}
                  </span>
                </li>
              ))}
            </ul>
          ) : null}
          {prCiFailure.failed_jobs.length > 0 ? (
            <ul data-testid="pr-ci-job-list">
              {prCiFailure.failed_jobs.map((j) => (
                <li key={j.id} data-testid={`pr-ci-job-${j.job_key}`}>
                  {j.job_key} ({j.state})
                  <div data-testid={`pr-ci-job-log-path-${j.job_key}`}>
                    {t('app.prCiJobLogPath')}: {j.log_api_path}
                  </div>
                  {j.log_has_chunks && j.log_excerpt ? (
                    <pre
                      data-testid={`pr-ci-job-log-excerpt-${j.job_key}`}
                      style={{ fontSize: '0.85rem', whiteSpace: 'pre-wrap' }}
                    >
                      {j.log_excerpt}
                    </pre>
                  ) : (
                    <p data-testid={`pr-ci-job-log-missing-${j.job_key}`}>
                      {t('app.prCiJobLogMissing')}
                    </p>
                  )}
                </li>
              ))}
            </ul>
          ) : null}
          {prCiFailure.agent_fix_allowed ? (
            <button
              type="button"
              data-testid="pr-agent-fix"
              disabled={agentRunStarting}
              onClick={() => void onStartPrAgentFix()}
            >
              {t('app.prAgentFix')}
            </button>
          ) : (
            <p data-testid="pr-agent-fix-denied" style={{ color: '#9a6700' }}>
              {t('app.prAgentFixDenied')}
              {prCiFailure.agent_fix_denial_reason
                ? `: ${prCiFailure.agent_fix_denial_reason}`
                : ''}
            </p>
          )}
        </div>
      ) : null}
    </Panel>
  )
}
