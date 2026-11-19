import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { useWorkspace } from '../app/WorkspaceProvider'
import { DEFAULT_ORG, paths } from '../app/paths'
import { Panel } from '../components/Panel'
import {
  getPullRequest,
  getPullRequestDiff,
  listPrComments,
  postPrComment,
  submitPrReview,
  syncPullRequest,
  type DiscussionComment,
  type PullRequestDetail,
} from '../api'

type DiffPick = { line: number; side: 'LEFT' | 'RIGHT'; text: string }
type CommentAnchorLocation = { path: string; line: number; side: 'LEFT' | 'RIGHT' }

function commentAnchorLocation(anchor: unknown): CommentAnchorLocation | null {
  if (typeof anchor !== 'object' || anchor === null || Array.isArray(anchor)) return null
  const value = anchor as Record<string, unknown>
  if (
    typeof value.path !== 'string' ||
    value.path.trim().length === 0 ||
    !Number.isSafeInteger(value.line) ||
    typeof value.line !== 'number' ||
    value.line < 1 ||
    (value.side !== 'LEFT' && value.side !== 'RIGHT')
  ) {
    return null
  }
  return { path: value.path, line: value.line, side: value.side }
}

function diffPickLines(unified: string): DiffPick[] {
  const picks: DiffPick[] = []
  let newLine = 0
  let oldLine = 0
  let inHunk = false
  for (const raw of unified.split('\n')) {
    if (raw.startsWith('@@')) {
      const m = raw.match(/@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/)
      if (m) {
        oldLine = Number(m[1])
        newLine = Number(m[2])
        inHunk = true
      }
      continue
    }
    if (!inHunk || raw.startsWith('+++') || raw.startsWith('---') || raw.startsWith('\\')) {
      continue
    }
    if (raw.startsWith('+')) {
      picks.push({ line: newLine, side: 'RIGHT', text: raw.slice(1) })
      newLine += 1
    } else if (raw.startsWith('-')) {
      picks.push({ line: oldLine, side: 'LEFT', text: raw.slice(1) })
      oldLine += 1
    } else if (raw.startsWith(' ')) {
      picks.push({ line: newLine, side: 'RIGHT', text: raw.slice(1) })
      picks.push({ line: oldLine, side: 'LEFT', text: raw.slice(1) })
      oldLine += 1
      newLine += 1
    }
  }
  return picks
}

export function PullDetailPage() {
  const { t } = useTranslation()
  const { session } = useWorkspace()
  const { org = DEFAULT_ORG, slug, number: numParam } = useParams<{
    org: string
    slug: string
    number: string
  }>()
  const prNumber = Number(numParam)
  const pageKey = JSON.stringify([org, slug ?? '', prNumber])
  const [pr, setPr] = useState<PullRequestDetail | null>(null)
  const [loadedKey, setLoadedKey] = useState<string | null>(null)
  const [pageLoadErrorKey, setPageLoadErrorKey] = useState<string | null>(null)
  const reloadSequence = useRef(0)
  const [files, setFiles] = useState<{ new_path?: string; change_type?: string }[]>([])
  const [comments, setComments] = useState<DiscussionComment[]>([])
  const [comment, setComment] = useState('')
  const [path, setPath] = useState('')
  const [diffText, setDiffText] = useState('')
  const [diffLoadingPath, setDiffLoadingPath] = useState<string | null>(null)
  const [diffErrorPath, setDiffErrorPath] = useState<string | null>(null)
  const diffSequence = useRef(0)
  const [pick, setPick] = useState<{ line: number; side: 'LEFT' | 'RIGHT' } | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)
  const [reviewState, setReviewState] = useState<'idle' | 'pending' | 'failed' | 'submitted'>('idle')
  const [reviewBody, setReviewBody] = useState('')
  const [reviewDecision, setReviewDecision] = useState<'approve' | 'request_changes' | 'comment'>('comment')
  const reviewSequence = useRef(0)

  const reload = useCallback(async () => {
    if (!slug || !prNumber) return
    const requestId = ++reloadSequence.current
    const requestKey = JSON.stringify([org, slug, prNumber])
    setPageLoadErrorKey(null)
    try {
      const [detail, diff, thread] = await Promise.all([
        getPullRequest(org, slug, prNumber),
        getPullRequestDiff(org, slug, prNumber),
        listPrComments(org, slug, prNumber),
      ])
      if (requestId !== reloadSequence.current) return
      setPr(detail)
      const fileList = (diff.files as { new_path?: string; change_type?: string }[] | undefined) ?? []
      setFiles(fileList)
      setComments(thread)
      setLoadedKey(requestKey)
    } catch (error) {
      if (requestId === reloadSequence.current) setPageLoadErrorKey(requestKey)
      throw error
    }
  }, [org, slug, prNumber])

  useEffect(() => {
    void reload().catch(() => {})
    return () => {
      reloadSequence.current += 1
      diffSequence.current += 1
    }
  }, [reload])

  const currentPr = loadedKey === pageKey ? pr : null

  useEffect(() => {
    setPath('')
    setDiffText('')
    setDiffLoadingPath(null)
    setDiffErrorPath(null)
    setPick(null)
    setActionError(null)
    setReviewState('idle')
    reviewSequence.current += 1
  }, [pageKey])

  async function loadPath(p: string) {
    const requestId = ++diffSequence.current
    setPath(p)
    setPick(null)
    setDiffText('')
    setDiffErrorPath(null)
    setDiffLoadingPath(p)
    try {
      const diff = await getPullRequestDiff(org, slug!, prNumber, p)
      if (requestId !== diffSequence.current) return
      const typed = diff as { unified?: string | null; binary?: boolean; truncated?: boolean }
      if (typed.binary) {
        setDiffText(t('code.binaryFile', { path: p }))
      } else if (typed.truncated) {
        setDiffText(
          `${t('code.truncatedFile', { path: p })}\n\n${String(typed.unified ?? '')}`,
        )
      } else {
        setDiffText(String(typed.unified ?? ''))
      }
    } catch {
      if (requestId === diffSequence.current) setDiffErrorPath(p)
    } finally {
      if (requestId === diffSequence.current) setDiffLoadingPath(null)
    }
  }

  async function submitReview(decision: 'approve' | 'request_changes' | 'comment') {
    const requestId = ++reviewSequence.current
    setReviewDecision(decision)
    setReviewState('pending')
    try {
      await submitPrReview(org, slug!, prNumber, decision, reviewBody.trim() || undefined)
      if (requestId !== reviewSequence.current) return
      setReviewState('submitted')
      void reload().catch(() => {})
    } catch {
      if (requestId === reviewSequence.current) setReviewState('failed')
    }
  }

  const pickLines = useMemo(() => diffPickLines(diffText), [diffText])

  if (!slug || !prNumber) return null
  if (!currentPr) {
    return pageLoadErrorKey === pageKey ? (
      <Panel>
        <p role="alert">{t('pull.detailLoadFailed')}</p>
        <button type="button" onClick={() => void reload().catch(() => {})}>
          {t('common.retry')}
        </button>
      </Panel>
    ) : (
      <p role="status">{t('common.loading')}</p>
    )
  }

  const isAuthor = session === currentPr.author_handle

  function stalePrMessage(err: unknown): string | null {
    const msg = err instanceof Error ? err.message : ''
    return /stale pull request revision/i.test(msg) ? msg : null
  }

  return (
    <Panel data-testid="pull-detail">
      <p>
        <Link to={paths.repoPulls(org, slug)}>← {t('nav.repoPulls')}</Link>
      </p>
      <h2 data-testid="pull-detail-title">
        #{currentPr.number} {currentPr.title}
      </h2>
      <p data-testid="pull-detail-branches">
        {currentPr.source_branch} → {currentPr.target_branch}
      </p>
      <p data-testid="pull-row-version">{t('pull.rowVersion', { version: currentPr.row_version })}</p>
      <section data-testid="pull-checks" aria-live="polite">
        <h3>{t('pull.checks')}</h3>
        <p
          data-testid="pull-merge-preconditions"
          data-passed={currentPr.merge_preconditions_passed}
          role="status"
        >
          {currentPr.merge_preconditions_passed
            ? t('pull.gatesPassed')
            : t('pull.gatesBlocked')}
        </p>
        <p>
          {t('pull.approvalGate')}:{' '}
          {currentPr.has_current_approval ? t('pull.gatePassed') : t('pull.gatePending')}
        </p>
        <p>
          {t('pull.checksGate')}:{' '}
          {currentPr.checks_gate_passed ? t('pull.gatePassed') : t('pull.gatePending')}
        </p>
        {currentPr.checks.length === 0 ? (
          <p>{t('pull.noChecks')}</p>
        ) : (
          <ul>
            {currentPr.checks.map((check) => (
              <li key={check.check_name} data-testid="pull-check">
                <strong>{check.check_name}</strong>
                {check.required ? ` · ${t('pull.requiredCheck')}` : ''}
                {' · '}
                {check.state == null
                  ? t('pull.checkMissing')
                  : t(`pull.checkState.${check.state}`)}
                {check.state === 'success' && !check.valid
                  ? ` · ${t('pull.checkReportInvalid')}`
                  : ''}
              </li>
            ))}
          </ul>
        )}
      </section>
      {currentPr.issue_number != null ? (
        <p data-testid="pull-linked-issue">
          {t('pull.linkedIssue')}:{' '}
          <Link to={paths.repoIssue(org, slug, currentPr.issue_number)}>#{currentPr.issue_number}</Link>
        </p>
      ) : (
        <p data-testid="pull-linked-issue-missing" role="status">
          {t('pull.noLinkedIssue')}
        </p>
      )}
      <button
        type="button"
        data-testid="pull-sync"
        onClick={() => {
          setActionError(null)
          void syncPullRequest(org, slug, prNumber, currentPr.row_version)
            .then((updated) => {
              if (updated.row_version != null) {
                setPr((prev) =>
                  prev ? { ...prev, row_version: updated.row_version! } : prev,
                )
              }
              return reload()
            })
            .catch(async (e) => {
              if (stalePrMessage(e)) {
                await reload().catch(() => {})
                setActionError(t('pull.staleRevision'))
                return
              }
              setActionError(e instanceof Error ? e.message : t('pull.syncFailed'))
            })
        }}
      >
        {t('pull.syncHead')}
      </button>
      {pageLoadErrorKey === pageKey ? (
        <div role="alert">
          <p>{t('pull.detailLoadFailed')}</p>
          <button type="button" onClick={() => void reload().catch(() => {})}>
            {t('common.retry')}
          </button>
        </div>
      ) : null}
      {actionError ? <p role="alert">{actionError}</p> : null}
      <ul data-testid="pull-diff-files">
        {files.map((f) => (
          <li key={f.new_path ?? f.change_type}>
            <button
              type="button"
              disabled={diffLoadingPath !== null}
              onClick={() => void loadPath(f.new_path ?? '')}
            >
              {f.change_type} {f.new_path}
            </button>
          </li>
        ))}
      </ul>
      {path ? (
        <>
          {diffLoadingPath === path ? <p role="status">{t('pull.diffLoading')}</p> : null}
          {diffErrorPath === path ? (
            <div role="alert">
              <p>{t('pull.diffFailed')}</p>
              <button type="button" onClick={() => void loadPath(path)}>
                {t('common.retry')}
              </button>
            </div>
          ) : null}
          {diffLoadingPath !== path && diffErrorPath !== path ? (
            <pre data-testid="pull-file-diff" style={{ maxHeight: 200, overflow: 'auto' }}>
              {diffText}
            </pre>
          ) : null}
          {diffLoadingPath !== path && diffErrorPath !== path ? (
            <ul data-testid="pull-diff-line-picks">
              {pickLines.map((line) => (
                <li key={`${line.side}-${line.line}`}>
                  <button
                    type="button"
                    data-testid="pull-diff-line-pick"
                    data-side={line.side}
                    data-line={line.line}
                    aria-pressed={pick?.line === line.line && pick.side === line.side}
                    onClick={() => setPick({ line: line.line, side: line.side })}
                  >
                    {line.side}:{line.line} {line.text.slice(0, 48)}
                  </button>
                </li>
              ))}
            </ul>
          ) : null}
        </>
      ) : null}
      <section data-testid="pull-discussion">
        <h3>{t('pull.review')}</h3>
        <label htmlFor="pull-review-body">{t('pull.reviewBody')}</label>
        <textarea
          id="pull-review-body"
          data-testid="pull-review-body"
          value={reviewBody}
          onChange={(event) => setReviewBody(event.target.value)}
        />
        <div>
          {!isAuthor ? (
            <button
              type="button"
              data-testid="pull-review-approve"
              disabled={reviewState === 'pending'}
              aria-busy={reviewState === 'pending' && reviewDecision === 'approve'}
              onClick={() => void submitReview('approve')}
            >
              {t('pull.approve')}
            </button>
          ) : (
            <span data-testid="pull-self-approve-blocked">{t('pull.selfApproveBlocked')}</span>
          )}
          <button
            type="button"
            data-testid="pull-review-request-changes"
            disabled={reviewState === 'pending'}
            aria-busy={reviewState === 'pending' && reviewDecision === 'request_changes'}
            onClick={() => void submitReview('request_changes')}
          >
            {t('pull.requestChanges')}
          </button>
          <button
            type="button"
            data-testid="pull-review-comment"
            disabled={reviewState === 'pending'}
            aria-busy={reviewState === 'pending' && reviewDecision === 'comment'}
            onClick={() => void submitReview('comment')}
          >
            {t('pull.reviewComment')}
          </button>
        </div>
        {reviewState === 'pending' ? <p role="status">{t('pull.reviewPending')}</p> : null}
        {reviewState === 'failed' ? <p role="alert">{t('pull.reviewFailed')}</p> : null}
        {reviewState === 'submitted' ? (
          <p data-testid="pull-review-status" role="status">
            {t('pull.reviewSubmitted', { decision: t(`pull.decision.${reviewDecision}`) })}
          </p>
        ) : null}
        <ul>
          {comments.map((c) => {
            const location = commentAnchorLocation(c.anchor)
            return (
              <li key={c.id} data-testid="pull-comment" data-outdated={c.outdated}>
                <strong>{c.author_handle}</strong>
                {c.outdated ? ` ${t('pull.outdatedAnchor')}` : ''}
                {location ? (
                  <span data-testid="pull-comment-location">
                    {' · '}
                    {t('pull.commentLocation', {
                      path: location.path,
                      side: t(location.side === 'LEFT' ? 'pull.baseSide' : 'pull.headSide'),
                      line: location.line,
                    })}
                  </span>
                ) : null}
                : {c.body}
              </li>
            )
          })}
        </ul>
        <form
          onSubmit={(e) => {
            e.preventDefault()
            setActionError(null)
            const anchor =
              path && pick
                ? {
                    path,
                    line: pick.line,
                    side: pick.side,
                    pr_revision_id: currentPr.row_version,
                  }
                : undefined
            void postPrComment(org, slug, prNumber, comment, anchor)
              .then(() => {
                setComment('')
                setPick(null)
                return reload()
              })
              .catch(async (err) => {
                if (stalePrMessage(err)) {
                  await reload().catch(() => {})
                  setActionError(t('pull.staleRevision'))
                  return
                }
                setActionError(err instanceof Error ? err.message : t('pull.commentFailed'))
              })
          }}
        >
          <textarea data-testid="pull-comment-input" value={comment} onChange={(e) => setComment(e.target.value)} />
          <button type="submit" data-testid="pull-comment-submit">
            {t('pull.addComment')}
          </button>
        </form>
      </section>
    </Panel>
  )
}
