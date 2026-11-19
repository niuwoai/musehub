import { FormEvent, useCallback, useEffect, useRef, useState } from 'react'
import { Link, useSearchParams } from 'react-router-dom'
import { Panel } from '../components/Panel'
import { useWorkspace } from '../app/WorkspaceProvider'
import { paths } from '../app/paths'
import {
  createRelease,
  getPipelineJobLogs,
  listPipelineRuns,
  listPipelineJobs,
  listReleases,
  publishRelease,
  type PipelineJob,
  type PipelineRunHistoryItem,
  type ReleaseSummary,
} from '../api'

const PIPELINE_HISTORY_PAGE_SIZE = 30

export function RepoDeliveryPage() {
  const ws = useWorkspace()
  const [searchParams] = useSearchParams()
  const deepLinkKeyRef = useRef('')
  const [jobLogText, setJobLogText] = useState<string | null>(null)
  const [jobLogKey, setJobLogKey] = useState<string | null>(null)
  const [jobLogError, setJobLogError] = useState<string | null>(null)
  const [releases, setReleases] = useState<ReleaseSummary[]>([])
  const [releasesLoading, setReleasesLoading] = useState(false)
  const [releasesError, setReleasesError] = useState<string | null>(null)
  const [pipelineHistory, setPipelineHistory] = useState<PipelineRunHistoryItem[]>([])
  const [pipelineHistoryLoading, setPipelineHistoryLoading] = useState(false)
  const [pipelineHistoryError, setPipelineHistoryError] = useState<string | null>(null)
  const [pipelineHistoryCursor, setPipelineHistoryCursor] = useState<string | null>(null)
  const [pipelineHistoryHasMore, setPipelineHistoryHasMore] = useState(false)
  const [releaseVersion, setReleaseVersion] = useState('')
  const [releaseTag, setReleaseTag] = useState('')
  const [releaseRunId, setReleaseRunId] = useState('')
  const [releaseNotes, setReleaseNotes] = useState('')
  const [releaseAssets, setReleaseAssets] = useState<NonNullable<PipelineJob['artifacts']>[number][]>([])
  const [releaseAssetIds, setReleaseAssetIds] = useState<string[]>([])
  const [releaseBusy, setReleaseBusy] = useState(false)
  const [releaseActionError, setReleaseActionError] = useState<string | null>(null)
  const [releaseActionStatus, setReleaseActionStatus] = useState<string | null>(null)
  const {
    t,
    activeOrg,
    activeRepoSlug,
    repoContextTransitioning,
    mergedPr,
    error: workspaceError,
    pipelineRun,
    pipelineJobs,
    pipelineRetryBusy,
    pipelineCancelBusy,
    publishReleaseBusy,
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

  const refreshReleases = useCallback(() => {
    if (!activeRepoSlug || repoContextTransitioning) return Promise.resolve()
    setReleasesLoading(true)
    setReleasesError(null)
    return listReleases(activeOrg, activeRepoSlug)
      .then((rows) => setReleases(rows))
      .catch((err) => {
        setReleases([])
        setReleasesError(
          err instanceof Error ? err.message : t('app.releasesLoadFailed'),
        )
      })
      .finally(() => setReleasesLoading(false))
  }, [activeOrg, activeRepoSlug, repoContextTransitioning, t])

  const refreshPipelineHistory = useCallback(async () => {
    if (!activeRepoSlug || repoContextTransitioning) return
    setPipelineHistoryLoading(true)
    setPipelineHistoryError(null)
    try {
      const page = await listPipelineRuns(activeOrg, activeRepoSlug, { limit: PIPELINE_HISTORY_PAGE_SIZE })
      setPipelineHistory(page.items)
      setPipelineHistoryCursor(page.next_cursor)
      setPipelineHistoryHasMore(page.has_more)
    } catch (reason) {
      setPipelineHistory([])
      setPipelineHistoryCursor(null)
      setPipelineHistoryHasMore(false)
      setPipelineHistoryError(reason instanceof Error ? reason.message : t('app.pipelineHistoryLoadFailed'))
    } finally {
      setPipelineHistoryLoading(false)
    }
  }, [activeOrg, activeRepoSlug, repoContextTransitioning, t])

  async function loadMorePipelineHistory() {
    if (!activeRepoSlug || repoContextTransitioning || pipelineHistoryLoading) return
    setPipelineHistoryLoading(true)
    setPipelineHistoryError(null)
    try {
      const page = await listPipelineRuns(activeOrg, activeRepoSlug, {
        beforeId: pipelineHistoryCursor,
        limit: PIPELINE_HISTORY_PAGE_SIZE,
      })
      setPipelineHistory((current) => [...current, ...page.items])
      setPipelineHistoryCursor(page.next_cursor)
      setPipelineHistoryHasMore(page.has_more)
    } catch (reason) {
      setPipelineHistoryError(reason instanceof Error ? reason.message : t('app.pipelineHistoryLoadFailed'))
    } finally {
      setPipelineHistoryLoading(false)
    }
  }

  useEffect(() => {
    void refreshReleases()
  }, [refreshReleases, publishedReleaseId, pipelineRun?.id])

  useEffect(() => {
    void refreshPipelineHistory()
  }, [refreshPipelineHistory, pipelineRun?.id])

  useEffect(() => {
    if (!activeRepoSlug || repoContextTransitioning || !releaseRunId) {
      setReleaseAssets([])
      setReleaseAssetIds([])
      return
    }
    void listPipelineJobs(activeOrg, activeRepoSlug, releaseRunId)
      .then((jobs) => {
        const available = jobs.flatMap((job) => job.artifacts ?? [])
        setReleaseAssets(available)
        setReleaseAssetIds(available.map((artifact) => artifact.id))
      })
      .catch(() => {
        setReleaseAssets([])
        setReleaseAssetIds([])
      })
  }, [activeOrg, activeRepoSlug, repoContextTransitioning, releaseRunId])

  useEffect(() => {
    if (!activeRepoSlug || repoContextTransitioning) return
    const dep = searchParams.get('deployment')
    const pipe = searchParams.get('pipeline')
    if (!dep && !pipe) return
    const key = `${activeRepoSlug}:${dep ?? ''}:${pipe ?? ''}`
    if (deepLinkKeyRef.current === key) return
    deepLinkKeyRef.current = key
    if (dep) {
      setDeploymentLookupId(dep)
      void onPickDeploymentFromList(dep)
    }
    if (pipe) void onFocusPipelineRun(pipe)
  }, [
    activeRepoSlug,
    repoContextTransitioning,
    searchParams,
    setDeploymentLookupId,
    onPickDeploymentFromList,
    onFocusPipelineRun,
  ])

  async function loadJobLogs(job: PipelineJob) {
    if (!activeRepoSlug || repoContextTransitioning || !pipelineRun) return
    setJobLogError(null)
    setJobLogKey(job.job_key)
    try {
      const chunk = await getPipelineJobLogs(
        activeOrg,
        activeRepoSlug,
        job.id,
        pipelineRun.run_attempt_id,
      )
      setJobLogText(chunk.text || t('app.prCiJobLogMissing'))
    } catch (err) {
      setJobLogError(err instanceof Error ? err.message : 'log fetch failed')
      setJobLogText(null)
    }
  }

  if (!activeRepoSlug) {
    return <p role="status">{t('repo.selectFirst')}</p>
  }
  const repoSlug = activeRepoSlug

  async function submitManualRelease(event: FormEvent) {
    event.preventDefault()
    const run = pipelineHistory.find((item) => item.id === releaseRunId && item.state === 'succeeded')
    if (!run) {
      setReleaseActionError(t('app.releasePipelineRequired'))
      return
    }
    setReleaseBusy(true)
    setReleaseActionError(null)
    setReleaseActionStatus(null)
    try {
      await createRelease(activeOrg, repoSlug, {
        version: releaseVersion.trim(),
        tag_name: releaseTag.trim(),
        commit_oid: run.source_oid,
        pipeline_run_id: run.id,
        release_notes: releaseNotes.trim(),
        artifact_ids: releaseAssetIds,
      })
      setReleaseActionStatus(t('app.releaseCreated'))
      setReleaseVersion('')
      setReleaseTag('')
      setReleaseRunId('')
      setReleaseNotes('')
      setReleaseAssetIds([])
      await refreshReleases()
    } catch (reason) {
      setReleaseActionError(reason instanceof Error ? reason.message : t('app.releaseCreateFailed'))
    } finally {
      setReleaseBusy(false)
    }
  }

  async function publishDraftRelease(releaseId: string) {
    setReleaseBusy(true)
    setReleaseActionError(null)
    setReleaseActionStatus(null)
    try {
      await publishRelease(activeOrg, repoSlug, releaseId)
      setReleaseActionStatus(t('app.releasePublished'))
      await refreshReleases()
    } catch (reason) {
      setReleaseActionError(reason instanceof Error ? reason.message : t('app.releasePublishFailed'))
    } finally {
      setReleaseBusy(false)
    }
  }

  const deliveryActionsDisabled =
    repoContextTransitioning || publishReleaseBusy

  return (
    <Panel data-testid="delivery-panel">
      <h2>{activeRepoSlug}</h2>
      {repoContextTransitioning ? (
        <p role="status" data-testid="delivery-repo-transition">
          {t('app.deliveryRepoContextTransitioning')}
        </p>
      ) : null}
      {workspaceError ? (
        <p role="alert" data-testid="delivery-workspace-error">
          {workspaceError}
        </p>
      ) : null}
      <h3>{t('app.deliveryTitle')}</h3>
      <p style={{ color: '#9a6700' }} data-testid="delivery-mode-note">
        {t('app.deliveryModeNote')}
      </p>
      <section data-testid="delivery-releases">
        <h4>{t('app.releasesTitle')}</h4>
        <form data-testid="release-create-form" onSubmit={(event) => void submitManualRelease(event)}>
          <label>
            {t('app.releaseVersion')}
            <input required value={releaseVersion} onChange={(event) => setReleaseVersion(event.target.value)} />
          </label>
          <label>
            {t('app.releaseTag')}
            <input required value={releaseTag} onChange={(event) => setReleaseTag(event.target.value)} />
          </label>
          <label>
            {t('app.releaseNotes')}
            <textarea value={releaseNotes} onChange={(event) => setReleaseNotes(event.target.value)} rows={4} />
          </label>
          <label>
            {t('app.releasePipeline')}
            <select required value={releaseRunId} onChange={(event) => setReleaseRunId(event.target.value)}>
              <option value="">{t('app.releaseChoosePipeline')}</option>
              {pipelineHistory.filter((run) => run.state === 'succeeded').map((run) => (
                <option key={run.id} value={run.id}>
                  {run.workflow_name} · {run.source_oid.slice(0, 7)} · {run.created_at}
                </option>
              ))}
            </select>
          </label>
          <fieldset>
            <legend>{t('app.releaseAssets')}</legend>
            <p>{t('app.releaseAssetsHint')}</p>
            {releaseAssets.length === 0 ? <p role="status">{t('app.releaseNoAssets')}</p> : null}
            {releaseAssets.map((artifact) => (
              <label key={artifact.id}>
                <input
                  type="checkbox"
                  checked={releaseAssetIds.includes(artifact.id)}
                  onChange={(event) => setReleaseAssetIds((ids) => event.target.checked
                    ? [...ids, artifact.id]
                    : ids.filter((id) => id !== artifact.id))}
                />
                {artifact.name} · {artifact.kind} · {artifact.sha256.slice(0, 12)}
              </label>
            ))}
          </fieldset>
          <button type="submit" disabled={releaseBusy || !pipelineHistory.some((run) => run.id === releaseRunId && run.state === 'succeeded')}>
            {releaseBusy ? t('common.loading') : t('app.createRelease')}
          </button>
        </form>
        {releaseActionError ? <p role="alert">{releaseActionError}</p> : null}
        {releaseActionStatus ? <p role="status">{releaseActionStatus}</p> : null}
        {releasesLoading ? (
          <p role="status" data-testid="delivery-releases-loading">
            {t('common.loading')}
          </p>
        ) : null}
        {releasesError ? (
          <p role="alert" data-testid="delivery-releases-error">
            {releasesError}
          </p>
        ) : null}
        {!releasesLoading && !releasesError && releases.length === 0 ? (
          <p role="status" data-testid="delivery-releases-empty">
            {t('app.releasesEmpty')}
          </p>
        ) : null}
        {!releasesLoading && !releasesError && releases.length > 0 ? (
          <ul data-testid="delivery-release-list">
            {releases.map((rel) => (
              <li key={rel.id} data-testid={`release-row-${rel.tag_name}`}>
                <details open={searchParams.get('release') === rel.id}>
                  <summary><code>{rel.version}</code> · {rel.tag_name} · {rel.state}</summary>
                  {rel.release_notes ? <p style={{ whiteSpace: 'pre-wrap' }}>{rel.release_notes}</p> : null}
                  <h5>{t('app.releaseAssets')}</h5>
                  {rel.assets.length === 0 ? <p>{t('app.releaseNoAssets')}</p> : (
                    <ul>
                      {rel.assets.map((asset) => (
                        <li key={asset.id}>
                          <a
                            href={`/api/v1/orgs/${activeOrg}/repos/${repoSlug}/pipeline-runs/${rel.pipeline_run_id}/artifacts/${asset.id}`}
                            download={asset.name}
                          >
                            {asset.name}
                          </a>{' '}· {asset.sha256} · {asset.byte_length} bytes
                        </li>
                      ))}
                    </ul>
                  )}
                  <p>{t('app.releaseCommit')}: {rel.commit_oid}</p>
                  <p>{t('app.releasePipeline')}: {rel.pipeline_run_id}</p>
                  {rel.manifest_sha256 ? <p>{t('app.releaseManifest')}: {rel.manifest_sha256}</p> : null}
                  {rel.oci_image_ref ? <p>{t('app.releaseOciRef')}: {rel.oci_image_ref}</p> : null}
                  {rel.state === 'draft' ? (
                    <button type="button" disabled={releaseBusy} onClick={() => void publishDraftRelease(rel.id)}>
                      {t('app.publishRelease')}
                    </button>
                  ) : null}
                </details>
              </li>
            ))}
          </ul>
        ) : null}
      </section>
      <section data-testid="pipeline-history">
        <h4>{t('app.pipelineHistoryTitle')}</h4>
        {pipelineHistoryLoading && pipelineHistory.length === 0 ? (
          <p role="status">{t('common.loading')}</p>
        ) : null}
        {pipelineHistoryError ? <p role="alert">{pipelineHistoryError}</p> : null}
        {!pipelineHistoryLoading && !pipelineHistoryError && pipelineHistory.length === 0 ? (
          <p>{t('app.pipelineHistoryEmpty')}</p>
        ) : null}
        <ul>
          {pipelineHistory.map((run) => (
            <li key={run.id} data-testid={`pipeline-history-${run.id}`}>
              <button type="button" onClick={() => void onFocusPipelineRun(run.id)}>
                {run.workflow_name} · {run.state} · {run.source_oid.slice(0, 7)} · {run.created_at}
              </button>
              {run.error_code ? ` · ${run.error_code}` : ''}
            </li>
          ))}
        </ul>
        {pipelineHistoryHasMore ? (
          <button
            type="button"
            data-testid="pipeline-history-load-more"
            disabled={pipelineHistoryLoading}
            onClick={() => void loadMorePipelineHistory()}
          >
            {pipelineHistoryLoading ? t('common.loading') : t('app.pipelineHistoryLoadMore')}
          </button>
        ) : null}
      </section>
      {mergedPr?.head_oid ? (
        <>
          <p>
            main @{mergedPr.head_oid.slice(0, 7)} (PR #{mergedPr.number})
            {mergedPr.issue_number != null ? (
              <>
                {' · '}
                <Link
                  to={paths.repoIssue(activeOrg, activeRepoSlug, mergedPr.issue_number)}
                  data-testid="delivery-merged-issue-link"
                >
                  issue #{mergedPr.issue_number}
                </Link>
              </>
            ) : null}
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
          {pipelineJobs.some((job) => job.result?.junit || (job.artifacts?.length ?? 0) > 0) ? (
            <section data-testid="pipeline-job-results">
              <h4>{t('app.pipelineArtifactsTitle')}</h4>
              <ul>
                {pipelineJobs.map((job) => (
                  <li key={job.id}>
                    {job.result?.junit ? (
                      <p data-testid={`pipeline-junit-summary-${job.job_key}`}>
                        {t('app.pipelineTestSummary', job.result.junit)}
                      </p>
                    ) : null}
                    {job.artifacts?.map((artifact) => (
                      <a
                        key={artifact.id}
                        href={artifact.download_url}
                        data-testid={`pipeline-artifact-${job.job_key}-${artifact.id}`}
                        download={artifact.name}
                      >
                        {t('app.pipelineDownloadArtifact', { name: artifact.name })}
                      </a>
                    ))}
                  </li>
                ))}
              </ul>
            </section>
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
          {pipelineRun?.state === 'succeeded' ? (
            <button
              type="button"
              data-testid="publish-pipeline-release"
              disabled={deliveryActionsDisabled}
              onClick={() => void ws.onPublishPipelineRelease().then(() => refreshReleases())}
            >
              {publishReleaseBusy
                ? t('common.loading')
                : t('app.publishPipelineRelease')}
            </button>
          ) : null}
          <button
            type="button"
            data-testid="release-and-deploy-mock"
            disabled={deliveryActionsDisabled}
            onClick={() => void ws.onReleaseAndDeploy()}
          >
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
              <div data-testid={`deployment-version-observation-${d.id}`}>
                <p>
                  {t('app.deploymentDesired')}: {d.desired_version} · {d.desired_commit_oid.slice(0, 12)}
                  {d.desired_oci_image_ref ? ` · ${d.desired_oci_image_ref}` : ''}
                </p>
                <p>
                  {t('app.deploymentObserved')}:{' '}
                  {d.observed_oci_digest ?? d.observed_source_oid ?? t('app.deploymentNoObservation')}
                </p>
                {d.observed_at ? <time dateTime={d.observed_at}>{t('app.deploymentObservedAt')}: {d.observed_at}</time> : null}
              </div>
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
                to={paths.repoIssue(activeOrg, activeRepoSlug, trace.issue_number)}
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
                to={paths.repoPull(activeOrg, activeRepoSlug, trace.pr_number)}
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
