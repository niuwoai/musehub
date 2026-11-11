import { Link, useParams } from 'react-router-dom'
import { Panel } from '../components/Panel'
import { useWorkspace } from '../app/WorkspaceProvider'
import { DEFAULT_ORG, paths } from '../app/paths'

export function RepoIssuesPage() {
  const { org = DEFAULT_ORG } = useParams<{ org: string }>()
  const {
    t,
    selectedRepo,
    issues,
    lastRun,
    agentRunStarting,
    onCreateIssue,
    onAcceptPlan,
    onRunAgent,
    onCancelRun,
    onCreatePrFromRun,
    loadIssues,
  } = useWorkspace()

  if (!selectedRepo) {
    return <p role="status">{t('repo.selectFirst')}</p>
  }

  return (
    <Panel>
      <h2>{t('nav.repoIssues')}</h2>
      <button type="button" data-testid="refresh-repo" onClick={() => void loadIssues(selectedRepo)}>
        {t('app.refreshRepo')}
      </button>
      <button type="button" onClick={() => void onCreateIssue()}>
        {t('app.createIssue')}
      </button>
      <ul>
        {issues.map((issue) => (
          <li key={issue.id} data-testid={`issue-row-${issue.number}`}>
            <Link
              to={paths.repoIssue(org, selectedRepo, issue.number)}
              data-testid={`issue-link-${issue.number}`}
            >
              #{issue.number} {issue.title}
            </Link>{' '}
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
        <div data-testid="last-agent-run" data-agent-run-id={lastRun?.id ?? ''}>
          <p>
            {agentRunStarting && !lastRun
              ? t('app.agentStarting', { defaultValue: 'starting agent run…' })
              : `${lastRun!.adapter_kind} / ${lastRun!.state}`}
            {lastRun?.result_commit_oid ? ` → ${lastRun.result_commit_oid.slice(0, 7)}` : ''}
            {lastRun?.budget_unknown ? ' (budget unknown)' : ''}
          </p>
          {lastRun && (lastRun.state === 'running' || lastRun.state === 'queued') ? (
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
    </Panel>
  )
}
