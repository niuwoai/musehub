import { useCallback, useEffect, useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import { Panel } from '../components/Panel'
import { DEFAULT_ORG, paths } from '../app/paths'
import {
  getIssue,
  listIssueComments,
  patchIssue,
  postIssueComment,
  type DiscussionComment,
  type IssueDetail,
} from '../api'

export function IssueDetailPage() {
  const { org = DEFAULT_ORG, slug, number: numParam } = useParams<{
    org: string
    slug: string
    number: string
  }>()
  const issueNumber = Number(numParam)
  const [issue, setIssue] = useState<IssueDetail | null>(null)
  const [comments, setComments] = useState<DiscussionComment[]>([])
  const [comment, setComment] = useState('')
  const [labels, setLabels] = useState('')
  const [assignee, setAssignee] = useState('')

  const reload = useCallback(async () => {
    if (!slug || !issueNumber) return
    const [detail, thread] = await Promise.all([
      getIssue(org, slug, issueNumber),
      listIssueComments(org, slug, issueNumber),
    ])
    setIssue(detail)
    setLabels((detail.labels ?? []).join(', '))
    setAssignee(detail.assignee_handle ?? '')
    setComments(thread)
  }, [org, slug, issueNumber])

  useEffect(() => {
    void reload().catch(() => {})
  }, [reload])

  if (!slug || !issueNumber) return null
  if (!issue) return <p role="status">loading…</p>

  return (
    <Panel data-testid="issue-detail">
      <p>
        <Link to={paths.repoIssues(org, slug)}>← issues</Link>
      </p>
      <h2 data-testid="issue-detail-title">
        #{issue.number} {issue.title}
      </h2>
      <p data-testid="issue-detail-state">{issue.state}</p>
      <pre data-testid="issue-detail-body">{issue.body}</pre>
      <form
        onSubmit={(e) => {
          e.preventDefault()
          void patchIssue(org, slug, issueNumber, {
            labels: labels.split(',').map((s) => s.trim()).filter(Boolean),
            assignee_handle: assignee,
          }).then(reload)
        }}
      >
        <label>
          labels
          <input data-testid="issue-labels" value={labels} onChange={(e) => setLabels(e.target.value)} />
        </label>
        <label>
          assignee
          <input data-testid="issue-assignee" value={assignee} onChange={(e) => setAssignee(e.target.value)} />
        </label>
        <button type="submit" data-testid="issue-save-meta">
          save
        </button>
      </form>
      <section data-testid="issue-discussion">
        <h3>Discussion</h3>
        <ul>
          {comments.map((c) => (
            <li key={c.id} data-testid="issue-comment">
              <strong>{c.author_handle}</strong>: {c.body}
            </li>
          ))}
        </ul>
        <form
          onSubmit={(e) => {
            e.preventDefault()
            void postIssueComment(org, slug, issueNumber, comment).then(() => {
              setComment('')
              return reload()
            })
          }}
        >
          <textarea
            data-testid="issue-comment-input"
            value={comment}
            onChange={(e) => setComment(e.target.value)}
          />
          <button type="submit" data-testid="issue-comment-submit">
            comment
          </button>
        </form>
      </section>
    </Panel>
  )
}
