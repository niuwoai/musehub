import { useCallback, useEffect, useState } from 'react'
import { Link, useParams, useSearchParams } from 'react-router-dom'
import { Panel } from '../components/Panel'
import { useWorkspace } from '../app/WorkspaceProvider'
import { DEFAULT_ORG, paths } from '../app/paths'
import { listIssuesPage, type Issue } from '../api'

function issueMatchesFilters(
  issue: Issue,
  filters: { state: string; label: string; assignee: string },
): boolean {
  if (filters.state && issue.state !== filters.state) return false
  if (filters.label && !(issue.labels ?? []).includes(filters.label)) return false
  if (filters.assignee && issue.assignee_handle !== filters.assignee) return false
  return true
}

export function RepoIssuesPage() {
  const { org = DEFAULT_ORG } = useParams<{ org: string }>()
  const [searchParams] = useSearchParams()
  const pageLimit = Math.min(
    100,
    Math.max(1, Number(searchParams.get('e2e_issue_limit')) || 25),
  )
  const {
    t,
    selectedRepo,
    lastRun,
    agentRunStarting,
    onCreateIssue,
    onAcceptPlan,
    onRunAgent,
    onCancelRun,
    onCreatePrFromRun,
    loadIssues,
  } = useWorkspace()
  const [items, setItems] = useState<Issue[]>([])
  const [hasMore, setHasMore] = useState(false)
  const [nextCursor, setNextCursor] = useState<string | null>(null)
  const [filterState, setFilterState] = useState('')
  const [filterLabel, setFilterLabel] = useState('')
  const [filterAssignee, setFilterAssignee] = useState('')
  const [listError, setListError] = useState<string | null>(null)
  const [listNotice, setListNotice] = useState<string | null>(null)
  const [loadingMore, setLoadingMore] = useState(false)

  const filters = { state: filterState, label: filterLabel, assignee: filterAssignee }

  const fetchPage = useCallback(
    async (cursor?: string, append = false) => {
      if (!selectedRepo) return
      setListError(null)
      setListNotice(null)
      const page = await listIssuesPage(org, selectedRepo, {
        state: filterState || undefined,
        label: filterLabel || undefined,
        assignee: filterAssignee || undefined,
        cursor,
        limit: pageLimit,
      })
      setItems((prev) => (append ? [...prev, ...page.items] : page.items))
      setHasMore(page.has_more)
      setNextCursor(page.next_cursor)
    },
    [org, selectedRepo, filterState, filterLabel, filterAssignee, pageLimit],
  )

  useEffect(() => {
    if (!selectedRepo) return
    void fetchPage().catch((e) => setListError(e instanceof Error ? e.message : t('issues.loadFailed')))
  }, [selectedRepo, fetchPage, t])

  async function handleCreateIssue() {
    setListError(null)
    setListNotice(null)
    const created = await onCreateIssue()
    if (!created || !selectedRepo) return
    if (issueMatchesFilters(created, filters)) {
      setItems((prev) => {
        const merged = [created, ...prev.filter((i) => i.id !== created.id)]
        merged.sort((a, b) => b.number - a.number)
        return merged
      })
    } else {
      setListNotice(t('issues.createdHiddenByFilters', { number: created.number }))
    }
    await loadIssues(selectedRepo)
    void fetchPage().catch((e) => setListError(e instanceof Error ? e.message : t('issues.loadFailed')))
  }

  async function refreshAll() {
    await Promise.all([loadIssues(selectedRepo!), fetchPage()])
  }

  if (!selectedRepo) {
    return <p role="status">{t('repo.selectFirst')}</p>
  }

  return (
    <Panel>
      <h2>{t('nav.repoIssues')}</h2>
      <div data-testid="issue-filters">
        <label>
          {t('issues.filterState')}
          <input
            data-testid="issue-filter-state"
            value={filterState}
            onChange={(e) => setFilterState(e.target.value)}
            placeholder="open"
          />
        </label>
        <label>
          {t('issues.filterLabel')}
          <input
            data-testid="issue-filter-label"
            value={filterLabel}
            onChange={(e) => setFilterLabel(e.target.value)}
          />
        </label>
        <label>
          {t('issues.filterAssignee')}
          <input
            data-testid="issue-filter-assignee"
            value={filterAssignee}
            onChange={(e) => setFilterAssignee(e.target.value)}
          />
        </label>
        <button
          type="button"
          data-testid="issue-apply-filters"
          onClick={() =>
            void fetchPage().catch((e) =>
              setListError(e instanceof Error ? e.message : t('issues.loadFailed')),
            )
          }
        >
          {t('issues.applyFilters')}
        </button>
      </div>
      <button type="button" data-testid="refresh-repo" onClick={() => void refreshAll()}>
        {t('app.refreshRepo')}
      </button>
      <button type="button" data-testid="create-issue" onClick={() => void handleCreateIssue()}>
        {t('app.createIssue')}
      </button>
      {listError ? <p role="alert">{listError}</p> : null}
      {listNotice ? <p role="status" data-testid="issue-list-notice">{listNotice}</p> : null}
      <ul data-testid="issue-list">
        {items.map((issue) => (
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
      {hasMore ? (
        <button
          type="button"
          data-testid="issue-load-more"
          disabled={loadingMore}
          onClick={() => {
            if (!nextCursor) return
            setLoadingMore(true)
            void fetchPage(nextCursor, true)
              .catch((e) => setListError(e instanceof Error ? e.message : t('issues.loadFailed')))
              .finally(() => setLoadingMore(false))
          }}
        >
          {loadingMore ? t('issues.loadingMore') : t('issues.loadMore')}
        </button>
      ) : null}
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
