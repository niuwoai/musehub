import { useCallback, useEffect, useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { Panel } from '../components/Panel'
import { DEFAULT_ORG, paths } from '../app/paths'
import {
  acceptPlan,
  cancelAgentRun,
  createPlan,
  getAgentRun,
  getIssue,
  getIssueDeliveryTrace,
  listIssueComments,
  patchIssue,
  postIssueComment,
  replaceIssueCriteria,
  respondAgentRun,
  startAgentRun,
  type AgentRun,
  type DiscussionComment,
  type IssueDeliveryTrace,
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
  const [criteriaText, setCriteriaText] = useState('')
  const [criteriaError, setCriteriaError] = useState<string | null>(null)
  const [trace, setTrace] = useState<IssueDeliveryTrace | null>(null)
  const [agentRun, setAgentRun] = useState<AgentRun | null>(null)
  const [agentBusy, setAgentBusy] = useState(false)
  const [agentInput, setAgentInput] = useState('')
  const [agentError, setAgentError] = useState<string | null>(null)
  const [closeReason, setCloseReason] = useState('completed')
  const [lifecycleError, setLifecycleError] = useState<string | null>(null)

  const reload = useCallback(async () => {
    if (!slug || !issueNumber) return
    const [detail, thread, delivery] = await Promise.all([
      getIssue(org, slug, issueNumber),
      listIssueComments(org, slug, issueNumber),
      getIssueDeliveryTrace(org, slug, issueNumber).catch(() => null),
    ])
    setIssue(detail)
    setLabels((detail.labels ?? []).join(', '))
    setAssignee(detail.assignee_handle ?? '')
    setComments(thread)
    setTrace(delivery)
  }, [org, slug, issueNumber])

  useEffect(() => {
    if (!issue || criteriaText.trim() !== '') return
    if ((issue.criteria?.length ?? 0) > 0) {
      setCriteriaText(issue.criteria!.map((c) => c.description).join('\n'))
    }
  }, [issue, criteriaText])

  useEffect(() => {
    void reload().catch(() => {})
  }, [reload])

  useEffect(() => {
    if (!agentRun || !['queued', 'running', 'waiting_input'].includes(agentRun.state)) {
      return
    }
    const id = agentRun.id
    const timer = window.setInterval(() => {
      void getAgentRun(id)
        .then(setAgentRun)
        .catch(() => {})
    }, 800)
    return () => window.clearInterval(timer)
  }, [agentRun])

  if (!slug || !issueNumber) return null
  if (!issue) return <p role="status">{t('common.loading')}</p>

  const revision = issue.revision ?? 1

  return (
    <Panel data-testid="issue-detail">
      <p>
        <Link to={paths.repoIssues(org, slug)}>← {t('nav.repoIssues')}</Link>
      </p>
      <h2 data-testid="issue-detail-title">
        #{issue.number} {issue.title}
      </h2>
      <p data-testid="issue-detail-state">
        {issue.state} · {t('issueDetail.revision', { version: revision })}
        {issue.close_reason ? ` · ${t('issueDetail.closedAs', { reason: issue.close_reason })}` : ''}
      </p>

      <section data-testid="issue-lifecycle">
        <h3>{t('issueDetail.lifecycleTitle')}</h3>
        {issue.state === 'closed' ? (
          <button
            type="button"
            data-testid="issue-reopen"
            onClick={() => {
              setLifecycleError(null)
              void patchIssue(org, slug, issueNumber, { state: 'open' })
                .then(reload)
                .catch((err) =>
                  setLifecycleError(
                    err instanceof Error ? err.message : t('issueDetail.saveFailed'),
                  ),
                )
            }}
          >
            {t('issueDetail.reopenIssue')}
          </button>
        ) : (
          <>
            {issue.state === 'open' ? (
              <button
                type="button"
                data-testid="issue-mark-in-progress"
                onClick={() => {
                  setLifecycleError(null)
                  void patchIssue(org, slug, issueNumber, { state: 'in_progress' })
                    .then(reload)
                    .catch((err) =>
                      setLifecycleError(
                        err instanceof Error ? err.message : t('issueDetail.saveFailed'),
                      ),
                    )
                }}
              >
                {t('issueDetail.markInProgress')}
              </button>
            ) : null}
            <label>
              {t('issueDetail.closeReason')}
              <input
                data-testid="issue-close-reason"
                value={closeReason}
                onChange={(e) => setCloseReason(e.target.value)}
                placeholder={t('issueDetail.closeReasonPlaceholder')}
              />
            </label>
            <button
              type="button"
              data-testid="issue-close"
              onClick={() => {
                setLifecycleError(null)
                if (!closeReason.trim()) {
                  setLifecycleError(t('issueDetail.closeReasonRequired'))
                  return
                }
                void patchIssue(org, slug, issueNumber, {
                  state: 'closed',
                  close_reason: closeReason.trim(),
                })
                  .then(reload)
                  .catch((err) =>
                    setLifecycleError(
                      err instanceof Error ? err.message : t('issueDetail.saveFailed'),
                    ),
                  )
              }}
            >
              {t('issueDetail.closeIssue')}
            </button>
          </>
        )}
        {lifecycleError ? <p role="alert">{lifecycleError}</p> : null}
      </section>

      <pre data-testid="issue-detail-body">{issue.body}</pre>

      <section data-testid="issue-criteria">
        <h3>{t('issueDetail.criteriaTitle')}</h3>
        {issue.criteria && issue.criteria.length > 0 ? (
          <ul data-testid="issue-criteria-list">
            {issue.criteria.map((c) => (
              <li key={c.ordinal}>
                {c.required ? '✓' : '○'} {c.description}{' '}
                <span style={{ color: '#57606a' }}>({c.verification_method})</span>
              </li>
            ))}
          </ul>
        ) : (
          <p role="status">{t('issueDetail.criteriaEmpty')}</p>
        )}
        <form
          onSubmit={(e) => {
            e.preventDefault()
            setCriteriaError(null)
            const lines = criteriaText
              .split('\n')
              .map((s) => s.trim())
              .filter(Boolean)
            if (lines.length === 0) {
              setCriteriaError(t('issueDetail.criteriaRequired'))
              return
            }
            void replaceIssueCriteria(
              org,
              slug,
              issueNumber,
              revision,
              lines.map((description) => ({ description })),
            )
              .then(() => {
                setCriteriaText(lines.join('\n'))
                return reload()
              })
              .catch((err) =>
                setCriteriaError(err instanceof Error ? err.message : t('issueDetail.saveFailed')),
              )
          }}
        >
          <label>
            {t('issueDetail.criteriaEdit')}
            <textarea
              data-testid="issue-criteria-input"
              value={criteriaText}
              onChange={(e) => setCriteriaText(e.target.value)}
              rows={4}
            />
          </label>
          {criteriaError ? <p role="alert">{criteriaError}</p> : null}
          <button type="submit" data-testid="issue-criteria-save">
            {t('issueDetail.criteriaSave')}
          </button>
        </form>
      </section>

      <section data-testid="issue-plan-agent">
        <h3>{t('issueDetail.planTitle')}</h3>
        {issue.plan ? (
          <p data-testid="issue-plan-state">
            v{issue.plan.version} {issue.plan.state}: {issue.plan.body.slice(0, 120)}
          </p>
        ) : (
          <p role="status">{t('issueDetail.planMissing')}</p>
        )}
        <button
          type="button"
          data-testid="issue-accept-plan"
          onClick={() => {
            setAgentError(null)
            void createPlan(org, slug, issueNumber, t('issueDetail.planDefaultBody'))
              .then(() => acceptPlan(org, slug, issueNumber))
              .then(() => reload())
              .catch((err) =>
                setAgentError(err instanceof Error ? err.message : t('issueDetail.planFailed')),
              )
          }}
        >
          {t('app.acceptPlan')}
        </button>
        <button
          type="button"
          data-testid="issue-run-agent"
          disabled={agentBusy}
          onClick={() => {
            setAgentError(null)
            setAgentBusy(true)
            void startAgentRun(org, slug, issue.id, t('issueDetail.agentTask'))
              .then((run) => {
                setAgentRun(run)
                return reload()
              })
              .catch((err) =>
                setAgentError(err instanceof Error ? err.message : t('issueDetail.agentFailed')),
              )
              .finally(() => setAgentBusy(false))
          }}
        >
          {t('app.runReferenceAgent')}
        </button>
        {agentRun ? (
          <div data-testid="issue-agent-run">
            <p>
              {agentRun.id.slice(0, 8)}… {agentRun.state}
            </p>
            {agentRun.state === 'waiting_input' ? (
              <form
                onSubmit={(e) => {
                  e.preventDefault()
                  void respondAgentRun(agentRun.id, agentInput)
                    .then(() => getAgentRun(agentRun.id))
                    .then(setAgentRun)
                    .then(() => setAgentInput(''))
                }}
              >
                <input
                  data-testid="issue-agent-respond"
                  value={agentInput}
                  onChange={(e) => setAgentInput(e.target.value)}
                  placeholder={t('issueDetail.agentRespondPlaceholder')}
                />
                <button type="submit">{t('issueDetail.agentRespond')}</button>
              </form>
            ) : null}
            {['queued', 'running', 'waiting_input'].includes(agentRun.state) ? (
              <button
                type="button"
                data-testid="issue-cancel-agent"
                onClick={() => void cancelAgentRun(agentRun.id).then(() => setAgentRun(null))}
              >
                {t('app.cancelAgent')}
              </button>
            ) : null}
          </div>
        ) : null}
        {agentError ? <p role="alert">{agentError}</p> : null}
      </section>

      {trace ? (
        <section data-testid="issue-delivery-trace">
          <h3>{t('issueDetail.deliveryTraceTitle')}</h3>
          <p>
            {t('issueDetail.deliveryPrs', { count: trace.pull_requests.length })}
            <Link to={paths.repoDelivery(org, slug!)} style={{ marginLeft: '0.75rem' }}>
              {t('issueDetail.deliveryViewDelivery')}
            </Link>
          </p>
          <ul data-testid="issue-trace-prs">
            {trace.pull_requests.length === 0 ? (
              <li role="status">{t('issueDetail.deliveryTraceEmptyPrs')}</li>
            ) : (
              trace.pull_requests.map((pr) => (
                <li key={pr.id} data-testid={`issue-trace-pr-${pr.number}`}>
                  <Link to={paths.repoPull(org, slug!, pr.number)}>#{pr.number}</Link> · {pr.state}
                  {pr.merged_oid ? ` @ ${pr.merged_oid.slice(0, 7)}` : ''}
                </li>
              ))
            )}
          </ul>
          <p>{t('issueDetail.deliveryPipelines', { count: trace.pipeline_runs.length })}</p>
          <ul data-testid="issue-trace-pipelines">
            {trace.pipeline_runs.length === 0 ? (
              <li role="status">{t('issueDetail.deliveryTraceEmptyPipelines')}</li>
            ) : (
              trace.pipeline_runs.map((run) => (
                <li key={run.id} data-testid={`issue-trace-pipeline-${run.id.slice(0, 8)}`}>
                  {run.id.slice(0, 8)}… {run.state} @ {run.source_oid.slice(0, 7)}
                </li>
              ))
            )}
          </ul>
          <p>{t('issueDetail.deliveryDeployments', { count: trace.deployments.length })}</p>
          <ul data-testid="issue-trace-deployments">
            {trace.deployments.length === 0 ? (
              <li role="status">{t('issueDetail.deliveryTraceEmptyDeployments')}</li>
            ) : (
              trace.deployments.map((dep) => (
                <li key={dep.id} data-testid={`issue-trace-deployment-${dep.id.slice(0, 8)}`}>
                  <Link to={paths.repoDelivery(org, slug!)}>{dep.environment_slug}</Link> · {dep.state}{' '}
                  <span data-testid="issue-trace-deployment-id">{dep.id}</span>
                </li>
              ))
            )}
          </ul>
          {trace.gaps.length > 0 ? (
            <p data-testid="issue-trace-gaps" style={{ color: '#9a6700' }}>
              {t('app.traceGaps')}: {trace.gaps.join(', ')}
            </p>
          ) : null}
        </section>
      ) : null}

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
