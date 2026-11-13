import { useEffect, useRef, useState } from 'react'
import { Link, useSearchParams } from 'react-router-dom'
import { Panel } from '../components/Panel'
import { useWorkspace } from '../app/WorkspaceProvider'
import { paths } from '../app/paths'
import { getPipelineJobLogs, type PipelineJob } from '../api'

export function RepoDeliveryPage() {
  const ws = useWorkspace()
  const [searchParams] = useSearchParams()
  const deepLinkKeyRef = useRef('')
  const [jobLogText, setJobLogText] = useState<string | null>(null)
  const [jobLogKey, setJobLogKey] = useState<string | null>(null)
  const [jobLogError, setJobLogError] = useState<string | null>(null)
  const {
    t,
    activeOrg,
    selectedRepo,
    mergedPr,
    pipelineRun,
    pipelineJobs,
    pipelineRetryBusy,
    pipelineCancelBusy,
    publishedReleaseId,
    deploymentListMode,
    deploymentListLoading,
    deploymentListLoadingMore,
    deploymentListError,
    deploymentSummaries,
    deploymentListHasMore,
    deploymentLookupId,
    setDeploymentLookupId,
    activeDeployment,
    deploymentApprovalBusy,
    deploymentRollbackBusy,
    deploymentId,
    trace,
    onPickDeploymentFromList,
    onFocusPipelineRun,
  } = ws

  useEffect(() => {
    if (!selectedRepo) return
    const dep = searchParams.get('deployment')
    const pipe = searchParams.get('pipeline')
    if (!dep && !pipe) return
    const key = `${selectedRepo}:${dep ?? ''}:${pipe ?? ''}`
    if (deepLinkKeyRef.current === key) return
    deepLinkKeyRef.current = key
    if (dep) {
      setDeploymentLookupId(dep)
      void onPickDeploymentFromList(dep)
    }
    if (pipe) void onFocusPipelineRun(pipe)
  }, [
    selectedRepo,
    searchParams,
    setDeploymentLookupId,
    onPickDeploymentFromList,
    onFocusPipelineRun,
  ])

  async function loadJobLogs(job: PipelineJob) {
    if (!selectedRepo || !pipelineRun) return
    setJobLogError(null)
    setJobLogKey(job.job_key)
    try {
      const chunk = await getPipelineJobLogs(
        activeOrg,
        selectedRepo,
        job.id,
        pipelineRun.run_attempt_id,
      )
      setJobLogText(chunk.text || t('app.prCiJobLogMissing'))
    } catch (err) {
      setJobLogError(err instanceof Error ? err.message : 'log fetch failed')
      setJobLogText(null)
    }
  }

  if (!selectedRepo) {
    return <p role="status">{t('repo.selectFirst')}</p>
  }

  return (
    <Panel data-testid="delivery-panel">
      <h2>{selectedRepo}</h2>
      <h3>{t('app.deliveryTitle')}</h3>
      <p style={{ color: '#9a6700' }} data-testid="delivery-mode-note">
        {t('app.deliveryModeNote')}
      </p>
      {mergedPr?.head_oid ? (
        <>
          <p>
            main @{mergedPr.head_oid.slice(0, 7)} (PR #{mergedPr.number})
          </p>
          {pipelineRun ? (
            <p data-testid="pipeline-state">
              pipeline {pipelineRun.id.slice(0, 8)}… {pipelineRun.state}
            </p>
          ) : null}
          {pipelineRun?.state === 'cancel_requested' ? (
            <p data-testid="pipeline-cancel-pending" style={{ color: '#9a6700' }}>
              {t('app.pipelineCancelPending')}
            </p>
          ) : null}
          {pipelineRun &&
          ['queued', 'running', 'cancel_requested', 'canceled'].includes(pipelineRun.state) ? (
            <p data-testid="pipeline-cancel-honesty" style={{ fontSize: '0.9rem' }}>
              {t('app.pipelineCancelHonesty')}
            </p>
          ) : null}
          {pipelineRun && ['queued', 'running'].includes(pipelineRun.state) ? (
            <button
              type="button"
              data-testid="pipeline-cancel"
              disabled={pipelineCancelBusy || pipelineRetryBusy}
              onClick={() => void ws.onCancelPipelineRun()}
            >
              {t('app.pipelineCancelRun')}
            </button>
          ) : null}
          {pipelineRun?.state === 'canceled' && pipelineJobs.length > 0 ? (
            <ul data-testid="pipeline-cancel-job-list">
              {pipelineJobs.map((job) => (
                <li key={job.id}>
                  {job.job_key} ({job.state})
                </li>
              ))}
            </ul>
          ) : null}
          {pipelineRun?.state === 'failed' ? (
            <div data-testid="pipeline-failure-panel">
              <p data-testid="pipeline-failure-code">{pipelineRun.error_code ?? 'failed'}</p>
              {pipelineJobs.length > 0 ? (
                <ul data-testid="pipeline-job-list">
                  {pipelineJobs.map((job) => (
                    <li key={job.id}>
                      {job.job_key} ({job.state})
                      <button
                        type="button"
                        data-testid={`pipeline-job-logs-${job.job_key}`}
                        onClick={() => void loadJobLogs(job)}
                      >
                        {t('app.pipelineViewJobLogs')}
                      </button>
                      {job.retry_allowed ? (
                        <button
                          type="button"
                          data-testid={`pipeline-retry-${job.job_key}`}
                          disabled={pipelineRetryBusy}
                          onClick={() => void ws.onRetryPipelineJob(job.id)}
                        >
                          {t('app.pipelineRetryJob')}
                        </button>
                      ) : null}
                    </li>
                  ))}
                </ul>
              ) : null}
            </div>
          ) : null}
          {jobLogKey ? (
            <div data-testid="pipeline-job-log-panel">
              <h4>{t('app.pipelineJobLogTitle', { job: jobLogKey })}</h4>
              {jobLogError ? <p role="alert">{jobLogError}</p> : null}
              {jobLogText ? (
                <pre data-testid="pipeline-job-log-text" style={{ whiteSpace: 'pre-wrap' }}>
                  {jobLogText}
                </pre>
              ) : null}
            </div>
          ) : null}
          <button type="button" onClick={() => void ws.onRunDeliveryPipeline()}>
            {t('app.runPipelineOnMain')}
          </button>
          <button type="button" onClick={() => void ws.onReleaseAndDeploy()}>
            {t('app.releaseAndDeployMock')}
          </button>
        </>
      ) : (
        <p>{t('app.noMergedPr')}</p>
      )}
      {publishedReleaseId ? (
        <button
          type="button"
          data-testid="deploy-production"
          onClick={() => void ws.onDeployProduction()}
        >
          {t('app.deployProductionProtected')}
        </button>
      ) : null}
      <div data-testid="deployment-pending-section">
        <h4>{t('app.deploymentListTitle')}</h4>
        <div>
          <button
            type="button"
            data-testid="deployment-view-pending"
            aria-pressed={deploymentListMode === 'pending'}
            disabled={deploymentListLoading}
            onClick={() => ws.onDeploymentListModeChange('pending')}
          >
            {t('app.deploymentViewPending')}
          </button>
          <button
            type="button"
            data-testid="deployment-view-all"
            aria-pressed={deploymentListMode === 'all'}
            disabled={deploymentListLoading}
            onClick={() => ws.onDeploymentListModeChange('all')}
          >
            {t('app.deploymentViewAll')}
          </button>
        </div>
        <button
          type="button"
          data-testid="refresh-deployments"
          disabled={deploymentListLoading || deploymentListLoadingMore}
          onClick={() => void ws.refreshDeploymentList()}
        >
          {t('app.refreshDeployments')}
        </button>
        {deploymentListLoading ? <p role="status">{t('app.deploymentListLoading')}</p> : null}
        {deploymentListLoadingMore ? (
          <p role="status">{t('app.deploymentListLoadingMore')}</p>
        ) : null}
        {deploymentListError ? <p role="alert">{deploymentListError}</p> : null}
        {!deploymentListLoading &&
        !deploymentListError &&
        deploymentSummaries.length === 0 ? (
          <p data-testid="deployment-list-empty">
            {deploymentListMode === 'pending'
              ? t('app.deploymentListEmpty')
              : t('app.deploymentListEmptyAll')}
          </p>
        ) : null}
        <ul data-testid="deployment-list">
          {deploymentSummaries.map((d) => (
            <li key={d.id} data-testid={`deployment-list-item-${d.id}`}>
              <button
                type="button"
                data-testid={`deployment-pick-${d.id}`}
                onClick={() => void ws.onPickDeploymentFromList(d.id)}
              >
                <span data-testid={`deployment-row-env-${d.id}`}>{d.environment_slug}</span>
                {' · '}
                <span data-testid={`deployment-row-state-${d.id}`}>{d.state}</span>
                {' · '}
                <span data-testid={`deployment-row-updated-${d.id}`}>
                  {ws.formatDeploymentWhen(d.updated_at)}
                </span>
                {d.plan_hash ? ` · ${d.plan_hash.slice(0, 8)}…` : ''}
              </button>
              {d.state === 'succeeded' ? (
                <button
                  type="button"
                  data-testid={`deployment-rollback-${d.id}`}
                  disabled={deploymentRollbackBusy}
                  onClick={() => void ws.onRollbackToTarget(d.id)}
                >
                  {t('app.rollbackDeployment')}
                </button>
              ) : null}
            </li>
          ))}
        </ul>
        {deploymentListHasMore ? (
          <button
            type="button"
            data-testid="deployment-load-more"
            disabled={deploymentListLoading || deploymentListLoadingMore}
            onClick={() => ws.onLoadMoreDeployments()}
          >
            {t('app.deploymentLoadMore')}
          </button>
        ) : null}
      </div>
      <label>
        {t('app.loadDeploymentById')}
        <input
          data-testid="deployment-lookup-id"
          value={deploymentLookupId}
          onChange={(e) => setDeploymentLookupId(e.target.value)}
        />
      </label>
      <button type="button" data-testid="load-deployment" onClick={() => void ws.onLoadDeploymentById()}>
        {t('app.loadDeployment')}
      </button>
      {activeDeployment?.state === 'failed' ? (
        <div data-testid="deployment-failure-panel">
          <p>{t('app.deploymentFailedTitle')}</p>
          {(() => {
            const { code, detail } = ws.deploymentHealthFailureText(
              activeDeployment.health_summary,
            )
            return (
              <>
                <p data-testid="deployment-failure-code">
                  {code || t('app.deploymentFailureUnknown')}
                </p>
                {detail ? <p data-testid="deployment-failure-detail">{detail}</p> : null}
              </>
            )
          })()}
          {(() => {
            const target = ws.latestSucceededRollbackTarget(
              activeDeployment.environment_slug,
              activeDeployment.id,
            )
            return target ? (
              <button
                type="button"
                data-testid="deployment-rollback-known-good"
                disabled={deploymentRollbackBusy}
                onClick={() => void ws.onRollbackToTarget(target.id)}
              >
                {t('app.rollbackToKnownGood', { id: target.id.slice(0, 8) })}
              </button>
            ) : (
              <p data-testid="deployment-rollback-unavailable">{t('app.rollbackNoKnownGood')}</p>
            )
          })()}
        </div>
      ) : null}
      {activeDeployment?.state === 'awaiting_approval' ? (
        <div data-testid="deployment-approval">
          <p data-testid="deployment-approval-state">
            {t('app.deploymentAwaitingApproval', {
              env: activeDeployment.environment_slug,
            })}
          </p>
          <p data-testid="deployment-plan-hash">{activeDeployment.plan_hash?.slice(0, 12)}…</p>
          <button
            type="button"
            data-testid="approve-deployment"
            disabled={deploymentApprovalBusy}
            onClick={() => void ws.onDeploymentDecision('approve')}
          >
            {t('app.approveDeployment')}
          </button>
          <button
            type="button"
            data-testid="reject-deployment"
            disabled={deploymentApprovalBusy}
            onClick={() => void ws.onDeploymentDecision('reject')}
          >
            {t('app.rejectDeployment')}
          </button>
        </div>
      ) : null}
      {deploymentId ? (
        <button type="button" onClick={() => void ws.onLoadTrace()}>
          {t('app.viewTrace')}
        </button>
      ) : null}
      {trace ? (
        <div data-testid="delivery-trace-links">
          {trace.issue_number != null ? (
            <p>
              {t('delivery.linkedIssue')}:{' '}
              <Link
                to={paths.repoIssue(activeOrg, selectedRepo, trace.issue_number)}
                data-testid="delivery-trace-issue-link"
              >
                #{trace.issue_number}
              </Link>
            </p>
          ) : null}
          {trace.pr_number != null ? (
            <p>
              {t('delivery.linkedPull')}:{' '}
              <Link
                to={paths.repoPull(activeOrg, selectedRepo, trace.pr_number)}
                data-testid="delivery-trace-pr-link"
              >
                #{trace.pr_number}
              </Link>
            </p>
          ) : null}
        </div>
      ) : null}
      {trace ? (
        <pre data-testid="delivery-trace" style={{ fontSize: '0.85rem' }}>
          {JSON.stringify(
            {
              adapter: trace.deployment_adapter,
              mock: trace.adapter_is_mock,
              release_oci_image_ref: trace.release_oci_image_ref,
              applied_oci_digest: trace.applied_oci_digest,
              verified_source_oid: trace.verified_source_oid,
              commit_oid: trace.commit_oid,
              gaps: trace.gaps,
              artifacts: trace.artifacts.length,
            },
            null,
            2,
          )}
        </pre>
      ) : null}
      {trace && trace.gaps.length > 0 ? (
        <p role="status">
          {t('app.traceGaps')}: {trace.gaps.join(', ')}
        </p>
      ) : null}
    </Panel>
  )
}
