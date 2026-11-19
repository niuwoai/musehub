import { useCallback, useEffect, useRef, useState } from 'react'
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
  listIssueAssignees,
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
  const [planText, setPlanText] = useState('')
  const [planBusy, setPlanBusy] = useState(false)
  const [planError, setPlanError] = useState<string | null>(null)
  const startAgentButton = useRef<HTMLButtonElement>(null)
  const focusRunAfterPlanAcceptance = useRef(false)
  const [trace, setTrace] = useState<IssueDeliveryTrace | null>(null)
  const [agentRun, setAgentRun] = useState<AgentRun | null>(null)
  const [agentBusy, setAgentBusy] = useState(false)
  const [agentInput, setAgentInput] = useState('')
  const [agentError, setAgentError] = useState<string | null>(null)
  const [closeReason, setCloseReason] = useState('completed')
  const [lifecycleError, setLifecycleError] = useState<string | null>(null)
  const [assigneeOptions, setAssigneeOptions] = useState<string[]>([])
  const metadataEditVersion = useRef(0)

  useEffect(() => {
    if (!slug) return
    void listIssueAssignees(org, slug)
      .then((opts) => setAssigneeOptions(opts.map((o) => o.handle)))
      .catch(() => setAssigneeOptions([]))
  }, [org, slug])

  const reload = useCallback(async () => {
    if (!slug || !issueNumber) return
    const metadataVersionAtStart = metadataEditVersion.current
    const [detail, thread, delivery] = await Promise.all([
      getIssue(org, slug, issueNumber),
      listIssueComments(org, slug, issueNumber),
      getIssueDeliveryTrace(org, slug, issueNumber).catch(() => null),
    ])
    setIssue(detail)
    if (metadataVersionAtStart === metadataEditVersion.current) {
      setLabels((detail.labels ?? []).join(', '))
      setAssignee(detail.assignee_handle ?? '')
    }
    setComments(thread)
    setTrace(delivery)
  }, [org, slug, issueNumber])

  useEffect(() => {
    setPlanText(issue?.plan?.body ?? t('issueDetail.planDefaultBody'))
  }, [issue?.current_revision_id, issue?.plan?.id, t])

  useEffect(() => {
    if (!focusRunAfterPlanAcceptance.current) return
    if (issue?.plan?.state !== 'accepted' || planText.trim() !== issue.plan.body.trim()) return
    focusRunAfterPlanAcceptance.current = false
    startAgentButton.current?.focus()
  }, [issue, planText])

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
            {issue.state === 'in_progress' ? (
              <button
                type="button"
                data-testid="issue-mark-open"
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
                {t('issueDetail.markOpen')}
              </button>
            ) : null}
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

      <section id="issue-plan-agent" data-testid="issue-plan-agent">
        <h3>{t('issueDetail.planTitle')}</h3>
        {issue.plan ? (
          <p data-testid="issue-plan-state">
            v{issue.plan.version} {issue.plan.state}: {issue.plan.body.slice(0, 120)}
          </p>
        ) : (
          <p role="status">{t('issueDetail.planMissing')}</p>
        )}
        <label>
          {t('issueDetail.planEdit')}
          <textarea
            data-testid="issue-plan-input"
            value={planText}
            onChange={(e) => setPlanText(e.target.value)}
            rows={5}
          />
        </label>
        {planError ? <p role="alert">{planError}</p> : null}
        <button
          type="button"
          data-testid="issue-accept-plan"
          disabled={planBusy || !planText.trim() ||
            (issue.plan?.state === 'accepted' && planText.trim() === issue.plan.body.trim())}
          onClick={() => {
            setPlanError(null)
            if (!planText.trim()) {
              setPlanError(t('issueDetail.planRequired'))
              return
            }
            setPlanBusy(true)
            void createPlan(org, slug, issueNumber, planText.trim())
              .then(() => {
                focusRunAfterPlanAcceptance.current = true
                return acceptPlan(org, slug, issueNumber)
              })
              .then(() => reload())
              .catch((err) => {
                focusRunAfterPlanAcceptance.current = false
                setPlanError(err instanceof Error ? err.message : t('issueDetail.planFailed'))
              })
              .finally(() => setPlanBusy(false))
          }}
        >
          {t('app.acceptPlan')}
        </button>
        {!(issue.plan?.state === 'accepted' && planText.trim() === issue.plan.body.trim()) ? (
          <p role="status">{t('issueDetail.agentRequiresAcceptedPlan')}</p>
        ) : null}
        <button
          type="button"
          ref={startAgentButton}
          data-testid="issue-run-agent"
          disabled={agentBusy || !(issue.plan?.state === 'accepted' && planText.trim() === issue.plan.body.trim())}
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
          <div id="issue-agent-run" data-testid="issue-agent-run">
            <p>
              {agentRun.id.slice(0, 8)}… {agentRun.state} · {agentRun.adapter_kind}
            </p>
            <dl data-testid="issue-agent-diagnostics">
              <dt>{t('issueDetail.agentProgress')}</dt><dd>{agentRun.recent_progress
                ? t(`issueDetail.agentProgressStages.${agentRun.recent_progress}`, { defaultValue: t('issueDetail.agentProgressUnknown') })
                : t('issueDetail.agentProgressUnknown')}</dd>
              <dt>{t('issueDetail.agentFrozenInput')}</dt><dd>
                {agentRun.issue_revision != null ? `r${agentRun.issue_revision}` : t('issueDetail.agentRevisionUnknown')}
                {agentRun.accepted_plan_version != null ? ` · plan v${agentRun.accepted_plan_version}` : ''}
              </dd>
              <dt>{t('issueDetail.agentBase')}</dt><dd><code>{agentRun.base_oid ?? t('issueDetail.agentBaseUnknown')}</code></dd>
              <dt>{t('issueDetail.agentBudget')}</dt><dd>
                {agentRun.budget_tokens_limit != null
                  ? t('issueDetail.agentBudgetValue', { used: agentRun.usage_tokens, limit: agentRun.budget_tokens_limit })
                  : t('issueDetail.agentBudgetUnset', { used: agentRun.usage_tokens })}
                {agentRun.budget_unknown ? ` · ${t('issueDetail.agentBudgetUnknown')}` : ''}
              </dd>
            </dl>
            {agentRun.error_code ? (
              <p role="alert" data-testid="issue-agent-error">
                {t(`issueDetail.agentErrors.${agentRun.error_code}`, { defaultValue: t('issueDetail.agentErrorGeneric', { code: agentRun.error_code }) })}
              </p>
            ) : null}
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
          <p>{t('issueDetail.deliveryPlans', { count: trace.plans.length })}</p>
          <ul data-testid="issue-trace-plans">
            {trace.plans.map((plan) => (
              <li key={plan.id}>
                <a href="#issue-plan-agent">{t('issueDetail.deliveryPlanVersion', { version: plan.version })}</a> · {plan.state}
              </li>
            ))}
          </ul>
          <p>{t('issueDetail.deliveryAgentRuns', { count: trace.agent_runs.length })}</p>
          <ul data-testid="issue-trace-agent-runs">
            {trace.agent_runs.map((run) => (
              <li key={run.id}>
                <a href="#issue-agent-run">{run.id.slice(0, 8)}…</a> · {run.state}
                {run.error_code ? ` · ${run.error_code}` : ''}
              </li>
            ))}
          </ul>
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
                  <Link
                    to={paths.repoDeliveryDeepLink(org, slug!, { pipeline: run.id })}
                    data-testid="issue-trace-pipeline-link"
                  >
                    {run.id.slice(0, 8)}…
                  </Link>{' '}
                  {run.state} @ {run.source_oid.slice(0, 7)}
                </li>
              ))
            )}
          </ul>
          <p>{t('issueDetail.deliveryReleases', { count: trace.releases.length })}</p>
          <ul data-testid="issue-trace-releases">
            {trace.releases.map((release) => (
              <li key={release.id}>
                <Link to={`${paths.repoDelivery(org, slug!)}?release=${encodeURIComponent(release.id)}`}>
                  {release.version} · {release.tag_name}
                </Link>{' '}· {release.state}
              </li>
            ))}
          </ul>
          <p>{t('issueDetail.deliveryDeployments', { count: trace.deployments.length })}</p>
          <ul data-testid="issue-trace-deployments">
            {trace.deployments.length === 0 ? (
              <li role="status">{t('issueDetail.deliveryTraceEmptyDeployments')}</li>
            ) : (
              trace.deployments.map((dep) => (
                <li key={dep.id} data-testid={`issue-trace-deployment-${dep.id.slice(0, 8)}`}>
                  <Link
                    to={paths.repoDeliveryDeepLink(org, slug!, { deployment: dep.id })}
                    data-testid="issue-trace-deployment-link"
                  >
                    {dep.environment_slug}
                  </Link>{' '}
                  · {dep.state}{' '}
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
          <input
            data-testid="issue-labels"
            value={labels}
            onChange={(e) => {
              metadataEditVersion.current += 1
              setLabels(e.target.value)
            }}
          />
        </label>
        <label>
          {t('issueDetail.assignee')}
          <select
            data-testid="issue-assignee"
            value={assignee}
            onChange={(e) => {
              metadataEditVersion.current += 1
              setAssignee(e.target.value)
            }}
          >
            <option value="">{t('issueDetail.assigneeNone')}</option>
            {assignee && !assigneeOptions.includes(assignee) ? (
              <option value={assignee}>{assignee}</option>
            ) : null}
            {assigneeOptions.map((handle) => (
              <option key={handle} value={handle}>
                {handle}
              </option>
            ))}
          </select>
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
