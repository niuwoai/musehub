import { useCallback, useEffect, useState } from 'react'
import { Link, useParams } from 'react-router-dom'
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

export function PullDetailPage() {
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
    const diff = await getPullRequestDiff(org, slug!, prNumber, p)
    setDiffText(String((diff as { unified?: string }).unified ?? ''))
  }

  if (!slug || !prNumber) return null
  if (!pr) return <p role="status">loading…</p>

  const isAuthor = session === (pr as { author_handle?: string }).author_handle

  return (
    <Panel data-testid="pull-detail">
      <p>
        <Link to={paths.repoPulls(org, slug)}>← pulls</Link>
      </p>
      <h2 data-testid="pull-detail-title">
        #{pr.number} {pr.title}
      </h2>
      <p data-testid="pull-detail-branches">
        {pr.source_branch} → {pr.target_branch}
      </p>
      <button type="button" data-testid="pull-sync" onClick={() => void syncPullRequest(org, slug, prNumber).then(reload)}>
        sync head
      </button>
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
        <pre data-testid="pull-file-diff" style={{ maxHeight: 280, overflow: 'auto' }}>
          {diffText}
        </pre>
      ) : null}
      <section data-testid="pull-discussion">
        <h3>Review</h3>
        {!isAuthor ? (
          <button
            type="button"
            data-testid="pull-review-approve"
            onClick={() => void submitPrReview(org, slug, prNumber, 'approve').then(reload)}
          >
            approve
          </button>
        ) : (
          <p data-testid="pull-self-approve-blocked">authors cannot approve</p>
        )}
        <ul>
          {comments.map((c) => (
            <li key={c.id} data-testid="pull-comment" data-outdated={c.outdated}>
              <strong>{c.author_handle}</strong>
              {c.outdated ? ' (outdated)' : ''}: {c.body}
            </li>
          ))}
        </ul>
        <form
          onSubmit={(e) => {
            e.preventDefault()
            void postPrComment(org, slug, prNumber, comment, path
              ? { path, line: 1, side: 'RIGHT' }
              : undefined,
            ).then(() => {
              setComment('')
              return reload()
            })
          }}
        >
          <textarea data-testid="pull-comment-input" value={comment} onChange={(e) => setComment(e.target.value)} />
          <button type="submit" data-testid="pull-comment-submit">
            comment
          </button>
        </form>
      </section>
    </Panel>
  )
}
