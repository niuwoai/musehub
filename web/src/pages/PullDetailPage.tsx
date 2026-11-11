import { useCallback, useEffect, useMemo, useState } from 'react'
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
  const [pr, setPr] = useState<PullRequestDetail | null>(null)
  const [files, setFiles] = useState<{ new_path?: string; change_type?: string }[]>([])
  const [comments, setComments] = useState<DiscussionComment[]>([])
  const [comment, setComment] = useState('')
  const [path, setPath] = useState('')
  const [diffText, setDiffText] = useState('')
  const [pick, setPick] = useState<{ line: number; side: 'LEFT' | 'RIGHT' } | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)

  const reload = useCallback(async () => {
    if (!slug || !prNumber) return
    const [detail, diff, thread] = await Promise.all([
      getPullRequest(org, slug, prNumber),
      getPullRequestDiff(org, slug, prNumber),
      listPrComments(org, slug, prNumber),
    ])
    setPr(detail)
    const fileList = (diff.files as { new_path?: string; change_type?: string }[] | undefined) ?? []
    setFiles(fileList)
    setComments(thread)
  }, [org, slug, prNumber])

  useEffect(() => {
    void reload().catch(() => {})
  }, [reload])

  async function loadPath(p: string) {
    setPath(p)
    setPick(null)
    const diff = await getPullRequestDiff(org, slug!, prNumber, p)
    setDiffText(String((diff as { unified?: string }).unified ?? ''))
  }

  const pickLines = useMemo(() => diffPickLines(diffText), [diffText])

  if (!slug || !prNumber) return null
  if (!pr) return <p role="status">{t('common.loading')}</p>

  const isAuthor = session === pr.author_handle

  return (
    <Panel data-testid="pull-detail">
      <p>
        <Link to={paths.repoPulls(org, slug)}>← {t('nav.repoPulls')}</Link>
      </p>
      <h2 data-testid="pull-detail-title">
        #{pr.number} {pr.title}
      </h2>
      <p data-testid="pull-detail-branches">
        {pr.source_branch} → {pr.target_branch}
      </p>
      <button
        type="button"
        data-testid="pull-sync"
        onClick={() => {
          setActionError(null)
          void syncPullRequest(org, slug, prNumber, pr.row_version)
            .then(reload)
            .catch((e) => setActionError(e instanceof Error ? e.message : t('pull.syncFailed')))
        }}
      >
        {t('pull.syncHead')}
      </button>
      {actionError ? <p role="alert">{actionError}</p> : null}
      <ul data-testid="pull-diff-files">
        {files.map((f) => (
          <li key={f.new_path ?? f.change_type}>
            <button type="button" onClick={() => void loadPath(f.new_path ?? '')}>
              {f.change_type} {f.new_path}
            </button>
          </li>
        ))}
      </ul>
      {path ? (
        <>
          <pre data-testid="pull-file-diff" style={{ maxHeight: 200, overflow: 'auto' }}>
            {diffText}
          </pre>
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
        </>
      ) : null}
      <section data-testid="pull-discussion">
        <h3>{t('pull.review')}</h3>
        {!isAuthor ? (
          <button
            type="button"
            data-testid="pull-review-approve"
            onClick={() => void submitPrReview(org, slug, prNumber, 'approve').then(reload)}
          >
            {t('pull.approve')}
          </button>
        ) : (
          <p data-testid="pull-self-approve-blocked">{t('pull.selfApproveBlocked')}</p>
        )}
        <ul>
          {comments.map((c) => (
            <li key={c.id} data-testid="pull-comment" data-outdated={c.outdated}>
              <strong>{c.author_handle}</strong>
              {c.outdated ? ` ${t('pull.outdatedAnchor')}` : ''}: {c.body}
            </li>
          ))}
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
                    pr_revision_id: pr.row_version,
                  }
                : undefined
            void postPrComment(org, slug, prNumber, comment, anchor)
              .then(() => {
                setComment('')
                setPick(null)
                return reload()
              })
              .catch((err) =>
                setActionError(err instanceof Error ? err.message : t('pull.commentFailed')),
              )
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
