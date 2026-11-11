import { useCallback, useEffect, useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
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
  const { t } = useTranslation()
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
  const [saveError, setSaveError] = useState<string | null>(null)
  const [commentError, setCommentError] = useState<string | null>(null)

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
  if (!issue) return <p role="status">{t('common.loading')}</p>

  return (
    <Panel data-testid="issue-detail">
      <p>
        <Link to={paths.repoIssues(org, slug)}>← {t('nav.repoIssues')}</Link>
      </p>
      <h2 data-testid="issue-detail-title">
        #{issue.number} {issue.title}
      </h2>
      <p data-testid="issue-detail-state">{issue.state}</p>
      <pre data-testid="issue-detail-body">{issue.body}</pre>
      <form
        onSubmit={(e) => {
          e.preventDefault()
          setSaveError(null)
          void patchIssue(org, slug, issueNumber, {
            labels: labels.split(',').map((s) => s.trim()).filter(Boolean),
            assignee_handle: assignee,
          })
            .then(reload)
            .catch((err) =>
              setSaveError(err instanceof Error ? err.message : t('issueDetail.saveFailed')),
            )
        }}
      >
        <label>
          {t('issueDetail.labels')}
          <input data-testid="issue-labels" value={labels} onChange={(e) => setLabels(e.target.value)} />
        </label>
        <label>
          {t('issueDetail.assignee')}
          <input data-testid="issue-assignee" value={assignee} onChange={(e) => setAssignee(e.target.value)} />
        </label>
        {saveError ? <p role="alert">{saveError}</p> : null}
        <button type="submit" data-testid="issue-save-meta">
          {t('issueDetail.save')}
        </button>
      </form>
      <section data-testid="issue-discussion">
        <h3>{t('issueDetail.discussion')}</h3>
        <ul>
          {comments.length === 0 ? (
            <li role="status">{t('issueDetail.noComments')}</li>
          ) : (
            comments.map((c) => (
              <li key={c.id} data-testid="issue-comment">
                <strong>{c.author_handle}</strong>: {c.body}
              </li>
            ))
          )}
        </ul>
        <form
          onSubmit={(e) => {
            e.preventDefault()
            setCommentError(null)
            void postIssueComment(org, slug, issueNumber, comment)
              .then(() => {
                setComment('')
                return reload()
              })
              .catch((err) =>
                setCommentError(err instanceof Error ? err.message : t('issueDetail.commentFailed')),
              )
          }}
        >
          <textarea
            data-testid="issue-comment-input"
            value={comment}
            onChange={(e) => setComment(e.target.value)}
          />
          {commentError ? <p role="alert">{commentError}</p> : null}
          <button type="submit" data-testid="issue-comment-submit">
            {t('issueDetail.addComment')}
          </button>
        </form>
      </section>
    </Panel>
  )
}
