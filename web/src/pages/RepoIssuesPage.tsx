import { useCallback, useEffect, useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import { Panel } from '../components/Panel'
import { useWorkspace } from '../app/WorkspaceProvider'
import { DEFAULT_ORG, paths } from '../app/paths'
import { listIssueAssignees, listIssuesPage, type Issue } from '../api'

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
  const envLimit = import.meta.env.VITE_E2E_ISSUE_PAGE_LIMIT
  const pageLimit = Math.min(
    100,
    Math.max(1, envLimit ? Number(envLimit) : 25),
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
  const [assigneeOptions, setAssigneeOptions] = useState<string[]>([])

  const filters = { state: filterState, label: filterLabel, assignee: filterAssignee }

  const fetchPage = useCallback(
    async (cursor?: string, append = false, opts?: { keepNotice?: boolean }) => {
      if (!selectedRepo) return
      setListError(null)
      if (!opts?.keepNotice) {
        setListNotice(null)
      }
      const page = await listIssuesPage(org, selectedRepo, {
        state: filterState || undefined,
        label: filterLabel || undefined,
        assignee: filterAssignee || undefined,
        cursor,
        limit: pageLimit,
      })
      setItems((prev) => {
        const merged = append ? [...prev, ...page.items] : page.items
        const seen = new Set<string>()
        return merged.filter((i) => {
          if (seen.has(i.id)) return false
          seen.add(i.id)
          return true
        })
      })
      setHasMore(page.has_more)
      setNextCursor(page.next_cursor)
      return page
    },
    [org, selectedRepo, filterState, filterLabel, filterAssignee, pageLimit],
  )

  const loadAssigneeOptions = useCallback(() => {
    if (!selectedRepo) return Promise.resolve()
    return listIssueAssignees(org, selectedRepo)
      .then((opts) => setAssigneeOptions(opts.map((o) => o.handle)))
      .catch(() => setAssigneeOptions([]))
  }, [org, selectedRepo])

  useEffect(() => {
    if (!selectedRepo) return
    void fetchPage().catch((e) => setListError(e instanceof Error ? e.message : t('issues.loadFailed')))
  }, [selectedRepo, fetchPage, t])

  useEffect(() => {
    if (!selectedRepo) return
    void loadAssigneeOptions()
  }, [selectedRepo, loadAssigneeOptions])

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
      await loadIssues(selectedRepo)
      void fetchPage(undefined, false).catch((e) =>
        setListError(e instanceof Error ? e.message : t('issues.loadFailed')),
      )
    } else {
      setListNotice(t('issues.createdHiddenByFilters', { number: created.number }))
      await loadIssues(selectedRepo)
      void fetchPage(undefined, false, { keepNotice: true }).catch((e) =>
        setListError(e instanceof Error ? e.message : t('issues.loadFailed')),
      )
    }
  }

  async function refreshAll() {
    await Promise.all([loadIssues(selectedRepo!), fetchPage(), loadAssigneeOptions()])
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
          <select
            data-testid="issue-filter-state"
            value={filterState}
            onChange={(e) => setFilterState(e.target.value)}
          >
            <option value="">{t('issues.filterAny')}</option>
            <option value="open">{t('issues.stateOpen')}</option>
            <option value="in_progress">{t('issues.stateInProgress')}</option>
            <option value="closed">{t('issues.stateClosed')}</option>
          </select>
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
          <select
            data-testid="issue-filter-assignee"
            value={filterAssignee}
            onChange={(e) => setFilterAssignee(e.target.value)}
          >
            <option value="">{t('issues.filterAny')}</option>
            {assigneeOptions.map((handle) => (
              <option key={handle} value={handle}>
                {handle}
              </option>
            ))}
          </select>
        </label>
        <button
          type="button"
          data-testid="issue-apply-filters"
          onClick={() =>
            void Promise.all([loadAssigneeOptions(), fetchPage()]).catch((e) =>
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
            <span data-testid={`issue-state-${issue.number}`}>{issue.state}</span>
            {issue.assignee_handle ? (
              <span data-testid={`issue-assignee-${issue.number}`}> @{issue.assignee_handle}</span>
            ) : null}{' '}
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
