import { useCallback, useEffect, useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import { Panel } from '../components/Panel'
import { useWorkspace } from '../app/WorkspaceProvider'
import { DEFAULT_ORG, paths } from '../app/paths'
import { createIssue, listIssueAssignees, listIssuesPage, type Issue } from '../api'

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
  const { org = DEFAULT_ORG, slug: routeSlug } = useParams<{ org: string; slug?: string }>()
  const envLimit = import.meta.env.VITE_E2E_ISSUE_PAGE_LIMIT
  const pageLimit = Math.min(
    100,
    Math.max(1, envLimit ? Number(envLimit) : 25),
  )
  const {
    t,
    selectedRepo: workspaceRepo,
    repoContextTransitioning,
    activeRepoSlug: workspaceActiveRepoSlug,
    lastRun,
    agentRunStarting,
    onAcceptPlan,
    onRunAgent,
    onCancelRun,
    onCreatePrFromRun,
    loadIssues,
  } = useWorkspace()
  const activeRepoSlug = routeSlug ?? workspaceActiveRepoSlug ?? workspaceRepo ?? null
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
  const [newIssueTitle, setNewIssueTitle] = useState('')
  const [newIssueAssignee, setNewIssueAssignee] = useState('')

  const filters = { state: filterState, label: filterLabel, assignee: filterAssignee }

  const fetchPage = useCallback(
    async (cursor?: string, append = false, opts?: { keepNotice?: boolean }) => {
      if (!activeRepoSlug) return
      setListError(null)
      if (!opts?.keepNotice) {
        setListNotice(null)
      }
      const page = await listIssuesPage(org, activeRepoSlug, {
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
    [org, activeRepoSlug, filterState, filterLabel, filterAssignee, pageLimit],
  )

  const loadAssigneeOptions = useCallback(() => {
    if (!activeRepoSlug) return Promise.resolve()
    return listIssueAssignees(org, activeRepoSlug)
      .then((opts) => setAssigneeOptions(opts.map((o) => o.handle)))
      .catch(() => setAssigneeOptions([]))
  }, [org, activeRepoSlug])

  useEffect(() => {
    if (!activeRepoSlug) return
    if (workspaceRepo && workspaceRepo !== activeRepoSlug) {
      void loadIssues(activeRepoSlug)
    }
    void fetchPage().catch((e) => setListError(e instanceof Error ? e.message : t('issues.loadFailed')))
  }, [activeRepoSlug, fetchPage, loadIssues, t, workspaceRepo])

  useEffect(() => {
    if (!activeRepoSlug) return
    void loadAssigneeOptions()
  }, [activeRepoSlug, loadAssigneeOptions])

  const canCreateIssue =
    newIssueTitle.trim().length > 0 && Boolean(activeRepoSlug) && !repoContextTransitioning

  async function handleCreateIssue() {
    const title = newIssueTitle.trim()
    if (!title || !activeRepoSlug) {
      setListError(t('issues.titleRequired'))
      return
    }
    if (repoContextTransitioning) {
      setListError(t('issues.repoContextTransitioning'))
      return
    }
    setListError(null)
    setListNotice(null)
    const created = await createIssue(org, activeRepoSlug, title, {
      assignee_handle: newIssueAssignee.trim() || undefined,
    })
    setNewIssueTitle('')
    setNewIssueAssignee('')
    if (!created || !activeRepoSlug) return
    if (issueMatchesFilters(created, filters)) {
      setItems((prev) => {
        const merged = [created, ...prev.filter((i) => i.id !== created.id)]
        merged.sort((a, b) => b.number - a.number)
        return merged
      })
      await loadIssues(activeRepoSlug)
      void fetchPage(undefined, false).catch((e) =>
        setListError(e instanceof Error ? e.message : t('issues.loadFailed')),
      )
    } else {
      setListNotice(t('issues.createdHiddenByFilters', { number: created.number }))
      await loadIssues(activeRepoSlug)
      void fetchPage(undefined, false, { keepNotice: true }).catch((e) =>
        setListError(e instanceof Error ? e.message : t('issues.loadFailed')),
      )
    }
  }

  async function refreshAll() {
    await Promise.all([loadIssues(activeRepoSlug!), fetchPage(), loadAssigneeOptions()])
  }

  if (!activeRepoSlug) {
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
      <input
        data-testid="issue-create-title"
        placeholder={t('issues.newTitlePlaceholder')}
        value={newIssueTitle}
        onChange={(e) => setNewIssueTitle(e.target.value)}
      />
      <select
        data-testid="issue-create-assignee"
        value={newIssueAssignee}
        onChange={(e) => setNewIssueAssignee(e.target.value)}
      >
        <option value="">{t('issues.filterAny')}</option>
        {assigneeOptions.map((handle) => (
          <option key={handle} value={handle}>
            {handle}
          </option>
        ))}
      </select>
      <button
        type="button"
        data-testid="create-issue"
        disabled={!canCreateIssue}
        onClick={() => void handleCreateIssue()}
      >
        {t('app.createIssue')}
      </button>
      {repoContextTransitioning ? (
        <p role="status" data-testid="issues-repo-transition">
          {t('issues.repoContextTransitioning')}
        </p>
      ) : null}
      {listError ? <p role="alert">{listError}</p> : null}
      {listNotice ? <p role="status" data-testid="issue-list-notice">{listNotice}</p> : null}
      <ul data-testid="issue-list">
        {items.map((issue) => (
          <li key={issue.id} data-testid={`issue-row-${issue.number}`}>
            <Link
              to={paths.repoIssue(org, activeRepoSlug, issue.number)}
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
