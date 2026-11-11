import { FormEvent, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'

import {
  acceptPlan,
  AgentRun,
  cancelAgentRun,
  createIssue,
  createPlan,
  createPullRequest,
  createRelease,
  createRepo,
  DeploymentTrace,
  getOrgSettings,
  patchOrgSettings,
  grantRepoCollaborator,
  listRepoGrants,
  listRunnerTokens,
  listAuditEvents,
  listOrgMembers,
  inviteOrgMember,
  patchOrgMemberRole,
  disableOrgMember,
  listMyTokens,
  createUserPat,
  createScopedPat,
  revokePat,
  changePassword,
  logout,
  getRunnerCapabilities,
  type AuditEvent,
  type OrgMember,
  type PatMasked,
  type RunnerCapabilities,
  getAgentRun,
  getDeployment,
  getDeploymentTrace,
  approveDeployment,
  rollbackDeployment,
  listDeployments,
  type Deployment,
  type DeploymentSummary,
  Issue,
  listIssues,
  listPullRequests,
  listRepos,
  login,
  me,
  cancelPipelineRun,
  getPipeline,
  listPipelineJobs,
  mergePullRequest,
  PipelineJob,
  PipelineRun,
  retryPipelineJob,
  publishRelease,
  PullRequest,
  Repository,
  setPullRequestCheck,
  startAgentRun,
  startDeployment,
  startPipeline,
  submitReview,
} from './api'

const appVersion = import.meta.env.VITE_APP_VERSION
const DEFAULT_ORG = 'demo'

export default function App() {
  const { t, i18n } = useTranslation()
  const isChinese = i18n.language.startsWith('zh')
  const nextLanguage = isChinese ? 'en' : 'zh'

  const [session, setSession] = useState<string | null>(null)
  const [repos, setRepos] = useState<Repository[]>([])
  const [selectedRepo, setSelectedRepo] = useState<string | null>(null)
  const [issues, setIssues] = useState<Issue[]>([])
  const [pulls, setPulls] = useState<PullRequest[]>([])
  const [lastRun, setLastRun] = useState<AgentRun | null>(null)
  const [agentRunStarting, setAgentRunStarting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [loginForm, setLoginForm] = useState({ handle: 'admin', password: '' })
  const [passwordChangeForm, setPasswordChangeForm] = useState({
    current: '',
    next: '',
  })
  const [passwordChangeBusy, setPasswordChangeBusy] = useState(false)
  const [pipelineRun, setPipelineRun] = useState<PipelineRun | null>(null)
  const [pipelineJobs, setPipelineJobs] = useState<PipelineJob[]>([])
  const [pipelineRetryBusy, setPipelineRetryBusy] = useState(false)
  const [pipelineCancelBusy, setPipelineCancelBusy] = useState(false)
  const pipelineRetryFlightRef = useRef<{ jobId: string; attemptId: string } | null>(null)
  const pipelinePollEpochRef = useRef(0)
  const [deploymentId, setDeploymentId] = useState<string | null>(null)
  const [activeDeployment, setActiveDeployment] = useState<Deployment | null>(null)
  const [publishedReleaseId, setPublishedReleaseId] = useState<string | null>(null)
  const [deploymentApprovalBusy, setDeploymentApprovalBusy] = useState(false)
  const [deploymentRollbackBusy, setDeploymentRollbackBusy] = useState(false)
  const [deploymentLookupId, setDeploymentLookupId] = useState('')
  const [deploymentSummaries, setDeploymentSummaries] = useState<DeploymentSummary[]>([])
  const [deploymentListMode, setDeploymentListMode] = useState<'pending' | 'all'>('pending')
  const [deploymentListLoading, setDeploymentListLoading] = useState(false)
  const [deploymentListLoadingMore, setDeploymentListLoadingMore] = useState(false)
  const [deploymentListError, setDeploymentListError] = useState<string | null>(null)
  const [deploymentListHasMore, setDeploymentListHasMore] = useState(false)
  const deploymentListFetchSeqRef = useRef(0)
  const deploymentListModeRef = useRef<'pending' | 'all'>('pending')
  const deploymentListModeEpochRef = useRef(0)
  const deploymentRollbackFlightRef = useRef<{
    targetId: string
    attemptId: string
  } | null>(null)
  const DEPLOYMENT_LIST_PAGE = 20
  const DEPLOYMENT_LIST_MAX_OFFSET = 10_000
  const [trace, setTrace] = useState<DeploymentTrace | null>(null)
  const [settings, setSettings] = useState<Awaited<ReturnType<typeof getOrgSettings>> | null>(null)
  const [orgDisplayName, setOrgDisplayName] = useState('')
  const [grants, setGrants] = useState<{ handle: string; role: string }[]>([])
  const [grantHandle, setGrantHandle] = useState('')
  const [mintedPatFull, setMintedPatFull] = useState<string | null>(null)
  const sessionEpochRef = useRef(0)
  const iamPanelsGenerationRef = useRef(0)
  const sessionHandleRef = useRef<string | null>(null)
  const orgMembersRevisionRef = useRef(0)
  /** Invalidated on logout / repo switch so deployment async work cannot repopulate UI. */
  const repoDeliveryGenerationRef = useRef(0)
  const selectedRepoRef = useRef<string | null>(null)
  /** Invalidated on logout so agent polling does not use IAM session epoch bumps. */
  const agentRunPollEpochRef = useRef(0)
  const pipelineRunIdRef = useRef<string | null>(null)

  type DeploymentOpContext = {
    sessionEpoch: number
    repoGeneration: number
    repo: string
  }

  type PipelineOpContext = DeploymentOpContext & {
    pipelineRunId: string | null
  }

  useEffect(() => {
    selectedRepoRef.current = selectedRepo
  }, [selectedRepo])

  useEffect(() => {
    pipelineRunIdRef.current = pipelineRun?.id ?? null
  }, [pipelineRun])

  function applyPipelineRun(run: PipelineRun | null) {
    pipelineRunIdRef.current = run?.id ?? null
    setPipelineRun(run)
  }

  useEffect(() => {
    deploymentListModeRef.current = deploymentListMode
  }, [deploymentListMode])

  function captureDeploymentOp(repoSlug?: string): DeploymentOpContext {
    return {
      sessionEpoch: sessionEpochRef.current,
      repoGeneration: repoDeliveryGenerationRef.current,
      repo: repoSlug ?? selectedRepoRef.current ?? '',
    }
  }

  function deploymentOpActive(ctx: DeploymentOpContext): boolean {
    return (
      sessionEpochRef.current === ctx.sessionEpoch &&
      repoDeliveryGenerationRef.current === ctx.repoGeneration &&
      selectedRepoRef.current === ctx.repo
    )
  }

  function capturePipelineOp(pipelineRunId: string | null): PipelineOpContext {
    return {
      ...captureDeploymentOp(),
      pipelineRunId,
    }
  }

  function pipelineOpActive(ctx: PipelineOpContext): boolean {
    return (
      deploymentOpActive(ctx) &&
      ctx.pipelineRunId !== null &&
      pipelineRunIdRef.current === ctx.pipelineRunId
    )
  }

  function bumpPipelinePollEpoch() {
    pipelinePollEpochRef.current += 1
  }

  function pipelinePollEpochActive(epoch: number) {
    return pipelinePollEpochRef.current === epoch
  }

  function resetDeploymentUiState() {
    setActiveDeployment(null)
    setDeploymentId(null)
    setPublishedReleaseId(null)
    setTrace(null)
    setDeploymentLookupId('')
    setDeploymentApprovalBusy(false)
    setDeploymentRollbackBusy(false)
    deploymentRollbackFlightRef.current = null
    pipelineRunIdRef.current = null
    setPipelineRun(null)
    setPipelineJobs([])
    setPipelineRetryBusy(false)
    setPipelineCancelBusy(false)
    pipelineRetryFlightRef.current = null
    bumpPipelinePollEpoch()
    setDeploymentSummaries([])
    setDeploymentListMode('pending')
    deploymentListModeRef.current = 'pending'
    deploymentListModeEpochRef.current += 1
    deploymentListFetchSeqRef.current += 1
    setDeploymentListError(null)
    setDeploymentListHasMore(false)
    setDeploymentListLoading(false)
    setDeploymentListLoadingMore(false)
  }

  function invalidateDeploymentWork() {
    repoDeliveryGenerationRef.current += 1
    resetDeploymentUiState()
  }

  function bumpAgentRunPollEpoch() {
    agentRunPollEpochRef.current += 1
  }

  function agentRunPollEpochActive(epoch: number) {
    return agentRunPollEpochRef.current === epoch
  }

  function resetAgentRunUi() {
    setLastRun(null)
    setAgentRunStarting(false)
  }

  function invalidateAgentRunWork() {
    bumpAgentRunPollEpoch()
    resetAgentRunUi()
  }

  function bumpOrgMembersRevision() {
    orgMembersRevisionRef.current += 1
  }

  function bumpIamPanelsGeneration() {
    iamPanelsGenerationRef.current += 1
  }

  function iamPanelsGenerationActive(generation: number) {
    return iamPanelsGenerationRef.current === generation
  }
  const [orgMembers, setOrgMembers] = useState<OrgMember[]>([])
  const [membersAdmin, setMembersAdmin] = useState(false)
  const [memberHandle, setMemberHandle] = useState('')
  const [memberPassword, setMemberPassword] = useState('')
  const [myTokens, setMyTokens] = useState<PatMasked[]>([])
  const [patName, setPatName] = useState('')
  const [patScopeMode, setPatScopeMode] = useState<'all_accessible' | 'selected'>('all_accessible')
  const [patRepoId, setPatRepoId] = useState('')
  const [iamBusy, setIamBusy] = useState(false)
  const iamBusyEpochRef = useRef<number | null>(null)
  const [auditEvents, setAuditEvents] = useState<AuditEvent[]>([])
  const [runnerCaps, setRunnerCaps] = useState<RunnerCapabilities | null>(null)
  const [runnerTokens, setRunnerTokens] = useState<
    Awaited<ReturnType<typeof listRunnerTokens>>
  >([])

  useEffect(() => {
    me()
      .then((principal) => setSession((prev) => prev ?? principal.handle))
      .catch(() => setSession((prev) => (prev === null ? null : prev)))
  }, [])

  function bumpSessionEpoch() {
    sessionEpochRef.current += 1
    return sessionEpochRef.current
  }

  function sessionEpochActive(epoch: number) {
    return sessionEpochRef.current === epoch
  }

  function beginIamOperation() {
    const opEpoch = sessionEpochRef.current
    iamBusyEpochRef.current = opEpoch
    setIamBusy(true)
    return opEpoch
  }

  function endIamOperation(opEpoch: number) {
    if (iamBusyEpochRef.current === opEpoch) {
      iamBusyEpochRef.current = null
      setIamBusy(false)
    }
  }

  function resetIamBusy() {
    iamBusyEpochRef.current = null
    setIamBusy(false)
  }

  useEffect(() => {
    const prevHandle = sessionHandleRef.current
    sessionHandleRef.current = session
    const identityChanged =
      prevHandle !== null && session !== null && prevHandle !== session
    if (session === null) {
      resetAgentRunUi()
    } else if (identityChanged) {
      invalidateAgentRunWork()
    }
    const epoch = bumpSessionEpoch()
    bumpIamPanelsGeneration()
    resetIamBusy()
    if (!session) {
      setAuditEvents([])
      setRunnerCaps(null)
      setRunnerTokens([])
      setMyTokens([])
      setOrgMembers([])
      setMembersAdmin(false)
      setMintedPatFull(null)
      bumpOrgMembersRevision()
      return
    }
    const panelsGeneration = iamPanelsGenerationRef.current
    const membersRevisionBaseline = orgMembersRevisionRef.current
    void (async () => {
      const pats = await listMyTokens().catch(() => [] as PatMasked[])
      if (!sessionEpochActive(epoch) || !iamPanelsGenerationActive(panelsGeneration)) {
        return
      }
      setMyTokens(pats)
      const caps = await getRunnerCapabilities().catch(() => null)
      if (!sessionEpochActive(epoch) || !iamPanelsGenerationActive(panelsGeneration)) {
        return
      }
      setRunnerCaps(caps)
      const audit = await listAuditEvents(DEFAULT_ORG, 30).catch(() => [] as AuditEvent[])
      if (!sessionEpochActive(epoch) || !iamPanelsGenerationActive(panelsGeneration)) {
        return
      }
      setAuditEvents(audit)
      const runner = await listRunnerTokens(DEFAULT_ORG).catch(
        () => [] as Awaited<ReturnType<typeof listRunnerTokens>>,
      )
      if (!sessionEpochActive(epoch) || !iamPanelsGenerationActive(panelsGeneration)) {
        return
      }
      setRunnerTokens(runner)
      try {
        const members = await listOrgMembers(DEFAULT_ORG)
        if (
          !sessionEpochActive(epoch) ||
          !iamPanelsGenerationActive(panelsGeneration) ||
          orgMembersRevisionRef.current !== membersRevisionBaseline
        ) {
          return
        }
        setOrgMembers(members)
        setMembersAdmin(true)
      } catch {
        if (!sessionEpochActive(epoch) || !iamPanelsGenerationActive(panelsGeneration)) {
          return
        }
        setOrgMembers([])
        setMembersAdmin(false)
      }
    })()
    return () => {
      bumpIamPanelsGeneration()
    }
  }, [session])

  async function refreshIamPanels() {
    const epoch = sessionEpochRef.current
    const panelsGeneration = iamPanelsGenerationRef.current
    try {
      const pats = await listMyTokens()
      if (!sessionEpochActive(epoch) || !iamPanelsGenerationActive(panelsGeneration)) {
        return
      }
      setMyTokens(pats)
    } catch (err) {
      if (sessionEpochActive(epoch) && iamPanelsGenerationActive(panelsGeneration)) {
        setError(err instanceof Error ? err.message : 'PAT list failed')
      }
      return
    }
    const audit = await listAuditEvents(DEFAULT_ORG, 40).catch(() => null)
    if (sessionEpochActive(epoch) && iamPanelsGenerationActive(panelsGeneration) && audit) {
      setAuditEvents(audit)
    }
    const members = await listOrgMembers(DEFAULT_ORG).catch(() => null)
    if (sessionEpochActive(epoch) && iamPanelsGenerationActive(panelsGeneration) && members) {
      bumpOrgMembersRevision()
      setOrgMembers(members)
      setMembersAdmin(true)
    }
  }

  async function onLogout() {
    setError(null)
    resetIamBusy()
    setPasswordChangeBusy(false)
    setPasswordChangeForm({ current: '', next: '' })
    invalidateAgentRunWork()
    invalidateDeploymentWork()
    bumpSessionEpoch()
    bumpIamPanelsGeneration()
    setMintedPatFull(null)
    selectedRepoRef.current = null
    setSelectedRepo(null)
    try {
      await logout()
    } catch {
      /* cookie may already be cleared */
    }
    setSession(null)
  }

  async function refreshRepos() {
    const data = await listRepos(DEFAULT_ORG)
    setRepos(data)
  }

  async function onLogin(event: FormEvent) {
    event.preventDefault()
    setError(null)
    try {
      await login(loginForm.handle, loginForm.password)
      const principal = await me()
      resetIamBusy()
      invalidateDeploymentWork()
      bumpSessionEpoch()
      bumpIamPanelsGeneration()
      setMintedPatFull(null)
      setSession(principal.handle)
      await Promise.all([refreshRepos(), refreshIamPanels()])
    } catch (err) {
      setError(err instanceof Error ? err.message : 'login failed')
    }
  }

  async function onChangePassword(event: FormEvent) {
    event.preventDefault()
    const opEpoch = sessionEpochRef.current
    setPasswordChangeBusy(true)
    setError(null)
    try {
      await changePassword(passwordChangeForm.current, passwordChangeForm.next)
      if (!sessionEpochActive(opEpoch)) {
        return
      }
      setPasswordChangeForm({ current: '', next: '' })
      const audit = await listAuditEvents(DEFAULT_ORG, 30).catch(() => null)
      if (sessionEpochActive(opEpoch) && audit) {
        setAuditEvents(audit)
      }
    } catch (err) {
      if (sessionEpochActive(opEpoch)) {
        setError(err instanceof Error ? err.message : 'password change failed')
      }
    } finally {
      if (sessionEpochActive(opEpoch)) {
        setPasswordChangeBusy(false)
      }
    }
  }

  async function onCreateRepo() {
    const slug = `repo-${Date.now().toString(36)}`
    await createRepo(DEFAULT_ORG, slug)
    await refreshRepos()
  }

  async function loadIssues(slug: string) {
    invalidateDeploymentWork()
    selectedRepoRef.current = slug
    setSelectedRepo(slug)
    const op = captureDeploymentOp(slug)
    setError(null)
    try {
      const prs = await listPullRequests(DEFAULT_ORG, slug)
      const data = await listIssues(DEFAULT_ORG, slug)
      const s = await getOrgSettings(DEFAULT_ORG)
      const g = await listRepoGrants(DEFAULT_ORG, slug)
      if (!deploymentOpActive(op)) {
        return
      }
      setPulls(prs)
      setIssues(data)
      setSettings(s)
      setOrgDisplayName(s.display_name)
      setGrants(g)
      if (deploymentOpActive(op)) {
        void refreshDeploymentList(op)
      }
    } catch (err) {
      if (deploymentOpActive(op)) {
        setError(err instanceof Error ? err.message : 'failed to load repository')
      }
    }
  }

  function deploymentHealthFailureText(health: Deployment['health_summary']) {
    if (!health || typeof health !== 'object') {
      return { code: '', detail: '' }
    }
    const code =
      typeof health.error_code === 'string'
        ? health.error_code
        : typeof health.status === 'string'
          ? health.status
          : ''
    const detail = typeof health.detail === 'string' ? health.detail : ''
    return { code, detail }
  }

  function latestSucceededRollbackTarget(
    environmentSlug: string,
    excludeId?: string,
  ): DeploymentSummary | undefined {
    return deploymentSummaries.find(
      (d) =>
        d.environment_slug === environmentSlug &&
        d.state === 'succeeded' &&
        d.id !== excludeId,
    )
  }

  function formatDeploymentWhen(iso: string) {
    const d = new Date(iso)
    if (Number.isNaN(d.getTime())) {
      return iso
    }
    return d.toISOString().replace('T', ' ').slice(0, 19)
  }

  function deploymentListIntentActive(modeEpoch: number, mode: 'pending' | 'all') {
    return (
      modeEpoch === deploymentListModeEpochRef.current &&
      mode === deploymentListModeRef.current
    )
  }

  function refreshDeploymentListAfterOp(op: DeploymentOpContext) {
    if (!deploymentOpActive(op)) {
      return
    }
    void refreshDeploymentList(op, {
      mode: deploymentListModeRef.current,
    })
  }

  async function refreshDeploymentList(
    op?: DeploymentOpContext,
    opts?: { append?: boolean; mode?: 'pending' | 'all' },
  ) {
    if (!selectedRepoRef.current) {
      return
    }
    const ctx = op ?? captureDeploymentOp()
    const mode = opts?.mode ?? deploymentListModeRef.current
    const append = opts?.append ?? false
    const listOffset = append ? deploymentSummaries.length : 0
    if (append && listOffset > DEPLOYMENT_LIST_MAX_OFFSET) {
      setDeploymentListError(
        `deployment list offset exceeds maximum (${DEPLOYMENT_LIST_MAX_OFFSET})`,
      )
      setDeploymentListHasMore(false)
      return
    }
    const modeEpochAtStart = deploymentListModeEpochRef.current
    const fetchSeq = (deploymentListFetchSeqRef.current += 1)
    if (append) {
      setDeploymentListLoadingMore(true)
    } else {
      setDeploymentListLoading(true)
      setDeploymentListLoadingMore(false)
      setDeploymentListError(null)
    }
    try {
      const list = await listDeployments(DEFAULT_ORG, ctx.repo, {
        limit: DEPLOYMENT_LIST_PAGE,
        offset: listOffset,
        ...(mode === 'pending' ? { state: 'awaiting_approval' } : {}),
      })
      if (
        !deploymentOpActive(ctx) ||
        fetchSeq !== deploymentListFetchSeqRef.current ||
        !deploymentListIntentActive(modeEpochAtStart, mode)
      ) {
        return
      }
      if (append) {
        setDeploymentSummaries((prev) => [...prev, ...list.items])
      } else {
        setDeploymentSummaries(list.items)
      }
      setDeploymentListHasMore(list.has_more)
    } catch (err) {
      if (
        deploymentOpActive(ctx) &&
        fetchSeq === deploymentListFetchSeqRef.current &&
        deploymentListIntentActive(modeEpochAtStart, mode)
      ) {
        setDeploymentListError(
          err instanceof Error ? err.message : 'failed to load deployments',
        )
        if (!append) {
          setDeploymentSummaries([])
        }
        setDeploymentListHasMore(false)
      }
    } finally {
      if (
        deploymentOpActive(ctx) &&
        fetchSeq === deploymentListFetchSeqRef.current &&
        deploymentListIntentActive(modeEpochAtStart, mode)
      ) {
        setDeploymentListLoading(false)
        setDeploymentListLoadingMore(false)
      }
    }
  }

  function onDeploymentListModeChange(mode: 'pending' | 'all') {
    if (mode === deploymentListMode) {
      return
    }
    deploymentListModeRef.current = mode
    deploymentListModeEpochRef.current += 1
    deploymentListFetchSeqRef.current += 1
    setDeploymentListMode(mode)
    setDeploymentSummaries([])
    setDeploymentListHasMore(false)
    setDeploymentListError(null)
    void refreshDeploymentList(captureDeploymentOp(), { mode })
  }

  function onLoadMoreDeployments() {
    if (deploymentListLoading || deploymentListLoadingMore || !deploymentListHasMore) {
      return
    }
    if (deploymentSummaries.length >= DEPLOYMENT_LIST_MAX_OFFSET) {
      setDeploymentListError(
        `deployment list offset exceeds maximum (${DEPLOYMENT_LIST_MAX_OFFSET})`,
      )
      setDeploymentListHasMore(false)
      return
    }
    void refreshDeploymentList(captureDeploymentOp(), {
      append: true,
      mode: deploymentListModeRef.current,
    })
  }

  async function onPickDeploymentFromList(id: string) {
    if (!selectedRepo) return
    const op = captureDeploymentOp()
    setDeploymentLookupId(id)
    setError(null)
    try {
      const dep = await getDeployment(DEFAULT_ORG, selectedRepo, id)
      if (!deploymentOpActive(op)) {
        return
      }
      setActiveDeployment(dep)
      setDeploymentId(dep.id)
      if (dep.state !== 'awaiting_approval') {
        const tv = await getDeploymentTrace(DEFAULT_ORG, selectedRepo, dep.id)
        if (!deploymentOpActive(op)) {
          return
        }
        setTrace(tv)
      } else if (deploymentOpActive(op)) {
        setTrace(null)
      }
    } catch (err) {
      if (deploymentOpActive(op)) {
        setError(err instanceof Error ? err.message : 'deployment load failed')
      }
    }
  }

  async function onCreateIssue() {
    if (!selectedRepo) return
    await createIssue(DEFAULT_ORG, selectedRepo, 'Slice issue')
    await loadIssues(selectedRepo)
  }

  async function onAcceptPlan(issue: Issue) {
    if (!selectedRepo) return
    await createPlan(DEFAULT_ORG, selectedRepo, issue.number, 'UI plan draft')
    await acceptPlan(DEFAULT_ORG, selectedRepo, issue.number)
  }

  async function pollRun(runId: string, pollEpoch: number) {
    // Up to ~3 minutes — matches Playwright delivery-flow budgets on slow CI agents.
    for (let i = 0; i < 360; i++) {
      if (!agentRunPollEpochActive(pollEpoch)) {
        return null
      }
      const run = await getAgentRun(runId)
      if (!agentRunPollEpochActive(pollEpoch)) {
        return null
      }
      setLastRun(run)
      if (['succeeded', 'failed', 'canceled'].includes(run.state)) {
        return run
      }
      await new Promise((resolve) => setTimeout(resolve, 500))
    }
    throw new Error('agent run timeout')
  }

  async function onRunAgent(issue: Issue) {
    if (!selectedRepo) return
    const pollEpoch = agentRunPollEpochRef.current
    setAgentRunStarting(true)
    try {
      const run = await startAgentRun(
        DEFAULT_ORG,
        selectedRepo,
        issue.id,
        `Implement: ${issue.title}`,
      )
      if (!agentRunPollEpochActive(pollEpoch)) {
        return
      }
      setLastRun(run)
      const finished = await pollRun(run.id, pollEpoch)
      if (!finished || !agentRunPollEpochActive(pollEpoch)) {
        return
      }
      await loadIssues(selectedRepo)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'agent run failed')
    } finally {
      setAgentRunStarting(false)
    }
  }

  async function onCancelRun() {
    if (!lastRun) return
    const pollEpoch = agentRunPollEpochRef.current
    await cancelAgentRun(lastRun.id)
    await pollRun(lastRun.id, pollEpoch)
  }

  async function onCreatePrFromRun() {
    if (!selectedRepo || !lastRun?.result_commit_oid) return
    const branch = `agent/${lastRun.id}`
    await createPullRequest(
      DEFAULT_ORG,
      selectedRepo,
      `PR for ${branch}`,
      branch,
    )
    await loadIssues(selectedRepo)
  }

  const mergedPr = pulls.find((p) => p.state === 'merged' && p.head_oid)

  async function onApproveAndMerge(pr: PullRequest) {
    if (!selectedRepo) return
    setError(null)
    try {
      await submitReview(DEFAULT_ORG, selectedRepo, pr.number, 'approve')
      const attempt = crypto.randomUUID()
      await setPullRequestCheck(
        DEFAULT_ORG,
        selectedRepo,
        pr.number,
        'product-ci',
        'success',
        attempt,
      )
      await mergePullRequest(DEFAULT_ORG, selectedRepo, pr.number)
      await loadIssues(selectedRepo)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'merge failed')
    }
  }

  async function loadPipelineJobsForRun(runId: string) {
    if (!selectedRepo) return
    const op = capturePipelineOp(runId)
    const jobs = await listPipelineJobs(DEFAULT_ORG, selectedRepo, runId)
    if (!pipelineOpActive(op)) return
    setPipelineJobs(jobs)
  }

  async function pollPipelineRun(runId: string, pollEpoch: number, op: PipelineOpContext) {
    if (!selectedRepo) return null
    const repo = selectedRepo
    const remotePipeline = runnerCaps?.pipeline_executor === 'remote'
    const maxIter = remotePipeline ? 600 : 240
    for (let i = 0; i < maxIter; i++) {
      if (!pipelinePollEpochActive(pollEpoch) || !pipelineOpActive(op)) {
        return null
      }
      const run = await getPipeline(DEFAULT_ORG, repo, runId)
      if (!pipelinePollEpochActive(pollEpoch) || !pipelineOpActive(op)) {
        return null
      }
      applyPipelineRun(run)
      if (run.state === 'succeeded' || run.state === 'failed' || run.state === 'canceled') {
        return run
      }
      await new Promise((resolve) => setTimeout(resolve, 500))
    }
    throw new Error('pipeline timeout')
  }

  async function onRunDeliveryPipeline() {
    if (!selectedRepo || !mergedPr?.head_oid) return
    const op = captureDeploymentOp()
    setError(null)
    setPipelineJobs([])
    bumpPipelinePollEpoch()
    const pollEpoch = pipelinePollEpochRef.current
    try {
      const run = await startPipeline(DEFAULT_ORG, selectedRepo, mergedPr.head_oid)
      if (!deploymentOpActive(op)) return
      applyPipelineRun(run)
      const pollOp = capturePipelineOp(run.id)
      const done = await pollPipelineRun(run.id, pollEpoch, pollOp)
      if (!done || !pipelineOpActive(pollOp)) return
      if (done.state === 'succeeded') {
        return
      }
      if (done.state === 'failed') {
        await loadPipelineJobsForRun(done.id)
        if (pipelineOpActive(pollOp)) {
          setError(done.error_code ?? 'pipeline failed')
        }
        return
      }
      if (done.state === 'canceled') {
        await loadPipelineJobsForRun(done.id)
        return
      }
      throw new Error(done.error_code ?? done.state)
    } catch (err) {
      if (deploymentOpActive(op)) {
        setError(err instanceof Error ? err.message : 'pipeline failed')
      }
    }
  }

  async function onCancelPipelineRun() {
    if (!selectedRepo || !pipelineRun || pipelineCancelBusy) return
    if (!['queued', 'running'].includes(pipelineRun.state)) return
    const op = capturePipelineOp(pipelineRun.id)
    setPipelineCancelBusy(true)
    setError(null)
    bumpPipelinePollEpoch()
    const pollEpoch = pipelinePollEpochRef.current
    const runId = pipelineRun.id
    try {
      const requested = await cancelPipelineRun(DEFAULT_ORG, selectedRepo, runId)
      if (!pipelineOpActive(op)) return
      applyPipelineRun(requested)
      const done = await pollPipelineRun(runId, pollEpoch, op)
      if (!done || !pipelineOpActive(op)) return
      if (done.state === 'canceled' || done.state === 'failed') {
        await loadPipelineJobsForRun(done.id)
      }
    } catch (err) {
      if (pipelineOpActive(op)) {
        setError(err instanceof Error ? err.message : 'pipeline cancel failed')
      }
    } finally {
      if (pipelineOpActive(op)) {
        setPipelineCancelBusy(false)
      }
    }
  }

  async function onRetryPipelineJob(jobId: string) {
    if (!selectedRepo || !pipelineRun || pipelineRetryBusy) return
    const inFlight = pipelineRetryFlightRef.current
    if (inFlight?.jobId === jobId) return
    const attemptId = crypto.randomUUID()
    pipelineRetryFlightRef.current = { jobId, attemptId }
    const op = capturePipelineOp(pipelineRun.id)
    setPipelineRetryBusy(true)
    setError(null)
    try {
      await retryPipelineJob(DEFAULT_ORG, selectedRepo, jobId, attemptId)
      if (!pipelineOpActive(op)) return
      bumpPipelinePollEpoch()
      const pollEpoch = pipelinePollEpochRef.current
      const done = await pollPipelineRun(pipelineRun.id, pollEpoch, op)
      if (!done || !pipelineOpActive(op)) return
      applyPipelineRun(done)
      setPipelineJobs([])
      if (done.state === 'failed') {
        await loadPipelineJobsForRun(done.id)
        if (pipelineOpActive(op)) {
          setError(done.error_code ?? 'pipeline failed')
        }
      } else if (done.state !== 'succeeded') {
        setError(done.error_code ?? done.state)
      }
    } catch (err) {
      if (pipelineOpActive(op)) {
        setError(err instanceof Error ? err.message : 'pipeline job retry failed')
      }
    } finally {
      if (pipelineOpActive(op)) {
        setPipelineRetryBusy(false)
      }
      if (pipelineRetryFlightRef.current?.attemptId === attemptId) {
        pipelineRetryFlightRef.current = null
      }
    }
  }

  async function onReleaseAndDeploy() {
    if (!selectedRepo || !mergedPr?.head_oid || !pipelineRun || pipelineRun.state !== 'succeeded') {
      return
    }
    const op = captureDeploymentOp()
    setError(null)
    try {
      const version = `0.0.${Date.now().toString(36)}`
      const rel = await createRelease(DEFAULT_ORG, selectedRepo, {
        version,
        tag_name: `v${version}`,
        commit_oid: mergedPr.head_oid,
        pipeline_run_id: pipelineRun.id,
        pr_id: mergedPr.id,
      })
      await publishRelease(DEFAULT_ORG, selectedRepo, rel.id)
      if (!deploymentOpActive(op)) {
        return
      }
      setPublishedReleaseId(rel.id)
      const dep = await startDeployment(DEFAULT_ORG, selectedRepo, rel.id)
      if (!deploymentOpActive(op)) {
        return
      }
      setDeploymentId(dep.id)
      setActiveDeployment(dep)
      const tv = await getDeploymentTrace(DEFAULT_ORG, selectedRepo, dep.id)
      if (!deploymentOpActive(op)) {
        return
      }
      setTrace(tv)
    } catch (err) {
      if (deploymentOpActive(op)) {
        setError(err instanceof Error ? err.message : 'release/deploy failed')
      }
    }
  }

  async function onDeployProduction() {
    if (!selectedRepo || !publishedReleaseId) return
    const op = captureDeploymentOp()
    setError(null)
    try {
      const dep = await startDeployment(
        DEFAULT_ORG,
        selectedRepo,
        publishedReleaseId,
        undefined,
        'production',
      )
      if (!deploymentOpActive(op)) {
        return
      }
      setDeploymentId(dep.id)
      setActiveDeployment(dep)
      if (dep.state !== 'awaiting_approval') {
        const tv = await getDeploymentTrace(DEFAULT_ORG, selectedRepo, dep.id)
        if (!deploymentOpActive(op)) {
          return
        }
        setTrace(tv)
      } else if (deploymentOpActive(op)) {
        setTrace(null)
      }
      refreshDeploymentListAfterOp(op)
    } catch (err) {
      if (deploymentOpActive(op)) {
        setError(err instanceof Error ? err.message : 'production deploy failed')
      }
    }
  }

  async function onRollbackToTarget(targetDeploymentId: string) {
    if (!selectedRepo || deploymentRollbackBusy) {
      return
    }
    const inFlight = deploymentRollbackFlightRef.current
    if (inFlight?.targetId === targetDeploymentId) {
      return
    }
    const attemptId = crypto.randomUUID()
    deploymentRollbackFlightRef.current = { targetId: targetDeploymentId, attemptId }
    const op = captureDeploymentOp()
    setDeploymentRollbackBusy(true)
    setError(null)
    try {
      const dep = await rollbackDeployment(
        DEFAULT_ORG,
        selectedRepo,
        targetDeploymentId,
        'console rollback',
        attemptId,
      )
      if (!deploymentOpActive(op)) {
        return
      }
      setDeploymentId(dep.id)
      setDeploymentLookupId(dep.id)
      setActiveDeployment(dep)
      if (dep.state !== 'awaiting_approval') {
        const tv = await getDeploymentTrace(DEFAULT_ORG, selectedRepo, dep.id)
        if (!deploymentOpActive(op)) {
          return
        }
        setTrace(tv)
      } else if (deploymentOpActive(op)) {
        setTrace(null)
      }
      refreshDeploymentListAfterOp(op)
    } catch (err) {
      if (deploymentOpActive(op)) {
        setError(err instanceof Error ? err.message : 'deployment rollback failed')
      }
    } finally {
      if (deploymentOpActive(op)) {
        setDeploymentRollbackBusy(false)
      }
      if (deploymentRollbackFlightRef.current?.attemptId === attemptId) {
        deploymentRollbackFlightRef.current = null
      }
    }
  }

  async function onDeploymentDecision(decision: 'approve' | 'reject') {
    if (!selectedRepo || !activeDeployment?.plan_hash) return
    const op = captureDeploymentOp()
    const deploymentIdLocked = activeDeployment.id
    const planHashLocked = activeDeployment.plan_hash
    setDeploymentApprovalBusy(true)
    setError(null)
    try {
      await approveDeployment(
        DEFAULT_ORG,
        selectedRepo,
        deploymentIdLocked,
        planHashLocked,
        decision,
        decision === 'approve' ? 'console approval' : 'console reject',
      )
      if (!deploymentOpActive(op)) {
        return
      }
      const refreshed = await getDeployment(
        DEFAULT_ORG,
        selectedRepo,
        deploymentIdLocked,
      )
      if (!deploymentOpActive(op)) {
        return
      }
      setActiveDeployment(refreshed)
      setDeploymentId(refreshed.id)
      if (refreshed.state !== 'awaiting_approval') {
        const tv = await getDeploymentTrace(
          DEFAULT_ORG,
          selectedRepo,
          refreshed.id,
        )
        if (!deploymentOpActive(op)) {
          return
        }
        setTrace(tv)
      } else if (deploymentOpActive(op)) {
        setTrace(null)
      }
      refreshDeploymentListAfterOp(op)
    } catch (err) {
      if (deploymentOpActive(op)) {
        setError(
          err instanceof Error ? err.message : 'deployment approval failed',
        )
      }
    } finally {
      if (deploymentOpActive(op)) {
        setDeploymentApprovalBusy(false)
      }
    }
  }

  async function onLoadTrace() {
    if (!selectedRepo || !deploymentId) return
    const op = captureDeploymentOp()
    const traceDeploymentId = deploymentId
    try {
      const tv = await getDeploymentTrace(
        DEFAULT_ORG,
        selectedRepo,
        traceDeploymentId,
      )
      if (deploymentOpActive(op)) {
        setTrace(tv)
      }
    } catch (err) {
      if (deploymentOpActive(op)) {
        setError(err instanceof Error ? err.message : 'trace load failed')
      }
    }
  }

  async function onLoadDeploymentById() {
    if (!selectedRepo || !deploymentLookupId.trim()) return
    const op = captureDeploymentOp()
    const lookupId = deploymentLookupId.trim()
    setError(null)
    try {
      const dep = await getDeployment(DEFAULT_ORG, selectedRepo, lookupId)
      if (!deploymentOpActive(op)) {
        return
      }
      setActiveDeployment(dep)
      setDeploymentId(dep.id)
      if (dep.state !== 'awaiting_approval') {
        const tv = await getDeploymentTrace(
          DEFAULT_ORG,
          selectedRepo,
          dep.id,
        )
        if (!deploymentOpActive(op)) {
          return
        }
        setTrace(tv)
      } else if (deploymentOpActive(op)) {
        setTrace(null)
      }
    } catch (err) {
      if (deploymentOpActive(op)) {
        setError(err instanceof Error ? err.message : 'deployment lookup failed')
      }
    }
  }

  return (
    <main
      style={{
        minHeight: '100vh',
        margin: 0,
        padding: '2rem',
        fontFamily:
          '-apple-system, BlinkMacSystemFont, "Segoe UI", "Noto Sans SC", Roboto, sans-serif',
        color: '#1f2328',
        background: '#f6f7f9',
      }}
    >
      <header style={{ marginBottom: '1.5rem' }}>
        <h1 style={{ margin: 0 }}>{t('app.name')}</h1>
        <p style={{ color: '#57606a' }}>
          {t('app.tagline', { version: appVersion })}
        </p>
        <button type="button" onClick={() => void i18n.changeLanguage(nextLanguage)}>
          {t('app.switchLanguage')}
        </button>
      </header>

      {session ? (
        <section>
          {error ? (
            <p role="alert" data-testid="app-alert">
              {error}
            </p>
          ) : null}
          <p>
            {t('app.signedInAs')}{' '}
            <strong data-testid="session-handle">{session}</strong>
            <button type="button" data-testid="logout" onClick={() => void onLogout()}>
              Log out
            </button>
          </p>
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: '1rem', marginBottom: '1rem' }}>
            <Panel style={{ flex: '1 1 18rem' }} data-testid="account-security">
              <h3>Account security</h3>
              <form onSubmit={(e) => void onChangePassword(e)}>
                <label>
                  Current password
                  <input
                    data-testid="password-current"
                    type="password"
                    autoComplete="current-password"
                    value={passwordChangeForm.current}
                    onChange={(e) =>
                      setPasswordChangeForm((f) => ({ ...f, current: e.target.value }))
                    }
                  />
                </label>
                <label>
                  New password
                  <input
                    data-testid="password-new"
                    type="password"
                    autoComplete="new-password"
                    value={passwordChangeForm.next}
                    onChange={(e) => setPasswordChangeForm((f) => ({ ...f, next: e.target.value }))}
                  />
                </label>
                <button type="submit" data-testid="change-password" disabled={passwordChangeBusy}>
                  Change password
                </button>
              </form>
            </Panel>
            <Panel style={{ flex: '1 1 18rem' }} data-testid="audit-panel">
              <h3>Audit (read-only)</h3>
              {auditEvents.length === 0 ? (
                <p>No audit events yet.</p>
              ) : (
                <ul style={{ maxHeight: '12rem', overflow: 'auto', margin: 0, paddingLeft: '1.2rem' }}>
                  {auditEvents.map((ev) => (
                    <li key={ev.id}>
                      <code>{ev.occurred_at.slice(0, 19)}</code> {ev.action}{' '}
                      <span>
                        {ev.resource_type}/{ev.resource_id.slice(0, 8)}…
                      </span>{' '}
                      · {ev.decision}
                    </li>
                  ))}
                </ul>
              )}
            </Panel>
            <Panel style={{ flex: '1 1 18rem' }} data-testid="org-members">
              <h3>Organization members</h3>
              {!membersAdmin ? (
                <p>Member admin requires org owner/admin.</p>
              ) : (
                <>
                  <ul data-testid="org-members-list" style={{ paddingLeft: '1.2rem' }}>
                    {orgMembers.map((m) => (
                      <li key={m.principal_id}>
                        {m.handle} · {m.org_role} · {m.principal_status}
                        {m.principal_status === 'active' && m.handle !== session ? (
                          <>
                            {' '}
                            <select
                              aria-label={`role-${m.handle}`}
                              value={m.org_role}
                              onChange={(e) => {
                                void (async () => {
                                  setError(null)
                                  try {
                                    await patchOrgMemberRole(
                                      DEFAULT_ORG,
                                      m.handle,
                                      e.target.value,
                                    )
                                    bumpIamPanelsGeneration()
                                    await refreshIamPanels()
                                  } catch (err) {
                                    setError(
                                      err instanceof Error
                                        ? err.message
                                        : 'role update failed',
                                    )
                                  }
                                })()
                              }}
                            >
                              <option value="member">member</option>
                              <option value="admin">admin</option>
                              <option value="owner">owner</option>
                            </select>
                            <button
                              type="button"
                              onClick={() => {
                                void (async () => {
                                  setError(null)
                                  try {
                                    await disableOrgMember(DEFAULT_ORG, m.handle)
                                    bumpIamPanelsGeneration()
                                    await refreshIamPanels()
                                  } catch (err) {
                                    setError(
                                      err instanceof Error ? err.message : 'disable failed',
                                    )
                                  }
                                })()
                              }}
                            >
                              Disable
                            </button>
                          </>
                        ) : null}
                      </li>
                    ))}
                  </ul>
                  <div>
                    <input
                      data-testid="member-handle"
                      placeholder="handle"
                      value={memberHandle}
                      onChange={(e) => setMemberHandle(e.target.value)}
                    />
                    <input
                      data-testid="member-password"
                      type="password"
                      placeholder="initial password"
                      value={memberPassword}
                      onChange={(e) => setMemberPassword(e.target.value)}
                    />
                    <button
                      type="button"
                      data-testid="invite-member"
                      disabled={iamBusy}
                      onClick={() => {
                        void (async () => {
                          const opEpoch = beginIamOperation()
                          const invitedAs = sessionHandleRef.current ?? session
                          setError(null)
                          try {
                            const invitedHandle = memberHandle.trim()
                            bumpOrgMembersRevision()
                            await inviteOrgMember(
                              DEFAULT_ORG,
                              invitedHandle,
                              memberPassword,
                              'member',
                            )
                            bumpIamPanelsGeneration()
                            const panelsGeneration = iamPanelsGenerationRef.current
                            const members = await listOrgMembers(DEFAULT_ORG)
                            if (
                              sessionEpochActive(opEpoch) &&
                              invitedAs &&
                              (sessionHandleRef.current ?? session) === invitedAs &&
                              iamPanelsGenerationActive(panelsGeneration)
                            ) {
                              bumpOrgMembersRevision()
                              setOrgMembers(members)
                              setMembersAdmin(true)
                            }
                            if (sessionEpochActive(opEpoch)) {
                              setMemberHandle('')
                              setMemberPassword('')
                            }
                          } catch (err) {
                            if (sessionEpochActive(opEpoch)) {
                              setError(
                                err instanceof Error ? err.message : 'member invite failed',
                              )
                            }
                          } finally {
                            endIamOperation(opEpoch)
                          }
                        })()
                      }}
                    >
                      Add member
                    </button>
                  </div>
                </>
              )}
            </Panel>
            <Panel style={{ flex: '1 1 18rem' }} data-testid="pat-management">
              <h3>Personal access tokens</h3>
              {mintedPatFull ? (
                <p data-testid="pat-minted-once">
                  Copy now (shown once): <code>{mintedPatFull}</code>
                </p>
              ) : (
                <p>Minted tokens are shown once at creation only.</p>
              )}
              <ul data-testid="pat-masked-list">
                {myTokens.map((t) => (
                  <li key={t.id}>
                    {t.name}: <code>{t.token_prefix}…</code> ({t.scope_mode}, exp{' '}
                    {t.expires_at.slice(0, 10)})
                    <button
                      type="button"
                      data-testid={`revoke-pat-${t.id}`}
                      onClick={() => {
                        void (async () => {
                          const epoch = sessionEpochRef.current
                          setError(null)
                          try {
                            await revokePat(t.id)
                            if (!sessionEpochActive(epoch)) {
                              return
                            }
                            setMintedPatFull(null)
                            bumpIamPanelsGeneration()
                            await refreshIamPanels()
                          } catch (err) {
                            if (sessionEpochActive(epoch)) {
                              setError(
                                err instanceof Error ? err.message : 'PAT revoke failed',
                              )
                            }
                          }
                        })()
                      }}
                    >
                      Revoke
                    </button>
                  </li>
                ))}
              </ul>
              <input
                data-testid="pat-name"
                placeholder="token name"
                value={patName}
                onChange={(e) => setPatName(e.target.value)}
              />
              <label>
                Scope mode{' '}
                <select
                  data-testid="pat-scope-mode"
                  value={patScopeMode}
                  onChange={(e) =>
                    setPatScopeMode(e.target.value as 'all_accessible' | 'selected')
                  }
                >
                  <option value="all_accessible">all_accessible</option>
                  <option value="selected">selected (one repo)</option>
                </select>
              </label>
              {patScopeMode === 'selected' ? (
                <label>
                  Repo{' '}
                  <select
                    data-testid="pat-repo-id"
                    value={patRepoId}
                    onChange={(e) => setPatRepoId(e.target.value)}
                  >
                    <option value="">— select —</option>
                    {repos.map((r) => (
                      <option key={r.id} value={r.id}>
                        {r.slug}
                      </option>
                    ))}
                  </select>
                </label>
              ) : null}
              <button
                type="button"
                data-testid="create-pat"
                disabled={iamBusy}
                onClick={() => {
                  void (async () => {
                    const opEpoch = beginIamOperation()
                    setError(null)
                    const name = patName.trim() || 'console-pat'
                    try {
                      let created
                      if (patScopeMode === 'selected') {
                        if (!patRepoId) {
                          if (sessionEpochActive(opEpoch)) {
                            setError('select a repo for selected-scope PAT')
                          }
                          return
                        }
                        created = await createScopedPat(DEFAULT_ORG, name, ['repo:read'], {
                          scope_mode: 'selected',
                          repo_ids: [patRepoId],
                        })
                      } else {
                        created = await createUserPat(DEFAULT_ORG, name)
                      }
                      if (!sessionEpochActive(opEpoch)) {
                        return
                      }
                      setMintedPatFull(created.token)
                      setPatName('')
                      bumpIamPanelsGeneration()
                      await refreshIamPanels()
                    } catch (err) {
                      if (sessionEpochActive(opEpoch)) {
                        setError(err instanceof Error ? err.message : 'PAT create failed')
                      }
                    } finally {
                      endIamOperation(opEpoch)
                    }
                  })()
                }}
              >
                Create PAT
              </button>
            </Panel>
            <Panel style={{ flex: '1 1 18rem' }} data-testid="runner-status-panel">
              <h3>Runner status</h3>
              {runnerCaps ? (
                <>
                  <p data-testid="runner-executors">
                    pipeline {runnerCaps.pipeline_executor} · agent {runnerCaps.agent_executor} ·
                    deploy {runnerCaps.deploy_executor}
                  </p>
                  <p>lease {runnerCaps.lease_seconds}s · scopes: {runnerCaps.scopes_accepted.join(', ')}</p>
                  <p style={{ color: '#57606a', fontSize: '0.9rem' }}>{runnerCaps.note}</p>
                </>
              ) : (
                <p>Unable to load runner capabilities.</p>
              )}
              {runnerTokens.length > 0 ? (
                <ul>
                  {runnerTokens.map((tok) => (
                    <li key={tok.id}>
                      {tok.name}: <code>{tok.token_prefix}…</code> ({tok.scope_mode})
                    </li>
                  ))}
                </ul>
              ) : (
                <p>No runner tokens (masked list only).</p>
              )}
            </Panel>
          </div>
          <button type="button" onClick={() => void onCreateRepo()}>
            {t('app.createRepo')}
          </button>
          <ul>
            {repos.map((repo) => (
              <li key={repo.id}>
                <button type="button" onClick={() => void loadIssues(repo.slug)}>
                  {repo.slug}
                </button>
                <code style={{ marginLeft: '0.5rem' }}>{repo.clone_url}</code>
              </li>
            ))}
          </ul>
          {selectedRepo ? (
            <Panel style={{ marginTop: '1rem' }}>
              <h2>{selectedRepo}</h2>
              {settings ? (
                <div data-testid="org-settings">
                  <label>
                    Org display name
                    <input
                      data-testid="org-display-name"
                      value={orgDisplayName}
                      onChange={(e) => setOrgDisplayName(e.target.value)}
                    />
                  </label>
                  <button
                    type="button"
                    data-testid="save-org-settings"
                    onClick={async () => {
                      const next = await patchOrgSettings(DEFAULT_ORG, {
                        display_name: orgDisplayName,
                        preferences: {
                          default_collaborator_role:
                            settings.preferences.default_collaborator_role || 'reader',
                        },
                      })
                      setSettings(next)
                    }}
                  >
                    Save org settings
                  </button>
                  <p>
                    pipeline:                     {settings.pipeline_executor} · agent:{' '}
                    {settings.agent_executor} · compose:{' '}
                    {settings.compose_adapter_mode} ({settings.product_profile})
                  </p>
                </div>
              ) : null}
              <div data-testid="repo-grants" style={{ marginBottom: '1rem' }}>
                <strong>Repo grants</strong>
                <ul>
                  {grants.map((g) => (
                    <li key={g.handle}>
                      {g.handle} ({g.role})
                    </li>
                  ))}
                </ul>
                <input
                  placeholder="handle"
                  value={grantHandle}
                  onChange={(e) => setGrantHandle(e.target.value)}
                />
                <button
                  type="button"
                  onClick={async () => {
                    if (!selectedRepo || !grantHandle.trim()) return
                    await grantRepoCollaborator(
                      DEFAULT_ORG,
                      selectedRepo,
                      grantHandle.trim(),
                      'reader',
                    )
                    setGrants(await listRepoGrants(DEFAULT_ORG, selectedRepo))
                    setGrantHandle('')
                  }}
                >
                  Grant reader
                </button>
              </div>
              <button
                type="button"
                data-testid="refresh-repo"
                onClick={() => void loadIssues(selectedRepo)}
              >
                {t('app.refreshRepo')}
              </button>
              <button type="button" onClick={() => void onCreateIssue()}>
                {t('app.createIssue')}
              </button>
              <ul>
                {issues.map((issue) => (
                  <li key={issue.id}>
                    #{issue.number} {issue.title}{' '}
                    <button type="button" onClick={() => void onAcceptPlan(issue)}>
                      {t('app.acceptPlan')}
                    </button>
                    <button type="button" onClick={() => void onRunAgent(issue)}>
                      {t('app.runReferenceAgent')}
                    </button>
                  </li>
                ))}
              </ul>
              {lastRun || agentRunStarting ? (
                <div data-testid="last-agent-run">
                  <p>
                    {agentRunStarting && !lastRun
                      ? t('app.agentStarting', { defaultValue: 'starting agent run…' })
                      : `${lastRun!.adapter_kind} / ${lastRun!.state}`}
                    {lastRun?.result_commit_oid
                      ? ` → ${lastRun.result_commit_oid.slice(0, 7)}`
                      : ''}
                    {lastRun?.budget_unknown ? ' (budget unknown)' : ''}
                  </p>
                  {lastRun &&
                  (lastRun.state === 'running' || lastRun.state === 'queued') ? (
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
              {pulls.length > 0 ? (
                <ul data-testid="pull-request-list">
                  {pulls.map((pr) => (
                    <li key={pr.id}>
                      #{pr.number} {pr.title} ({pr.state}) {pr.source_branch} →{' '}
                      {pr.target_branch}
                      {pr.state === 'open' ? (
                        <button type="button" onClick={() => void onApproveAndMerge(pr)}>
                          {t('app.mergeShortcut')}
                        </button>
                      ) : null}
                    </li>
                  ))}
                </ul>
              ) : null}
              <Panel style={{ marginTop: '1rem' }} data-testid="delivery-panel">
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
                    ['queued', 'running', 'cancel_requested', 'canceled'].includes(
                      pipelineRun.state,
                    ) ? (
                      <p data-testid="pipeline-cancel-honesty" style={{ fontSize: '0.9rem' }}>
                        {t('app.pipelineCancelHonesty')}
                      </p>
                    ) : null}
                    {pipelineRun && ['queued', 'running'].includes(pipelineRun.state) ? (
                      <button
                        type="button"
                        data-testid="pipeline-cancel"
                        disabled={pipelineCancelBusy || pipelineRetryBusy}
                        onClick={() => void onCancelPipelineRun()}
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
                        <p data-testid="pipeline-failure-code">
                          {pipelineRun.error_code ?? 'failed'}
                        </p>
                        {pipelineJobs.length > 0 ? (
                          <ul data-testid="pipeline-job-list">
                            {pipelineJobs.map((job) => (
                              <li key={job.id}>
                                {job.job_key} ({job.state})
                                {job.retry_allowed ? (
                                  <button
                                    type="button"
                                    data-testid={`pipeline-retry-${job.job_key}`}
                                    disabled={pipelineRetryBusy}
                                    onClick={() => void onRetryPipelineJob(job.id)}
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
                    <button type="button" onClick={() => void onRunDeliveryPipeline()}>
                      {t('app.runPipelineOnMain')}
                    </button>
                    <button type="button" onClick={() => void onReleaseAndDeploy()}>
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
                    onClick={() => void onDeployProduction()}
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
                      onClick={() => onDeploymentListModeChange('pending')}
                    >
                      {t('app.deploymentViewPending')}
                    </button>
                    <button
                      type="button"
                      data-testid="deployment-view-all"
                      aria-pressed={deploymentListMode === 'all'}
                      disabled={deploymentListLoading}
                      onClick={() => onDeploymentListModeChange('all')}
                    >
                      {t('app.deploymentViewAll')}
                    </button>
                  </div>
                  <button
                    type="button"
                    data-testid="refresh-deployments"
                    disabled={deploymentListLoading || deploymentListLoadingMore}
                    onClick={() => void refreshDeploymentList()}
                  >
                    {t('app.refreshDeployments')}
                  </button>
                  {deploymentListLoading ? (
                    <p role="status">{t('app.deploymentListLoading')}</p>
                  ) : null}
                  {deploymentListLoadingMore ? (
                    <p role="status">{t('app.deploymentListLoadingMore')}</p>
                  ) : null}
                  {deploymentListError ? (
                    <p role="alert">{deploymentListError}</p>
                  ) : null}
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
                          onClick={() => void onPickDeploymentFromList(d.id)}
                        >
                          <span data-testid={`deployment-row-env-${d.id}`}>
                            {d.environment_slug}
                          </span>
                          {' · '}
                          <span data-testid={`deployment-row-state-${d.id}`}>
                            {d.state}
                          </span>
                          {' · '}
                          <span data-testid={`deployment-row-updated-${d.id}`}>
                            {formatDeploymentWhen(d.updated_at)}
                          </span>
                          {d.plan_hash ? ` · ${d.plan_hash.slice(0, 8)}…` : ''}
                        </button>
                        {d.state === 'succeeded' ? (
                          <button
                            type="button"
                            data-testid={`deployment-rollback-${d.id}`}
                            disabled={deploymentRollbackBusy}
                            onClick={() => void onRollbackToTarget(d.id)}
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
                      onClick={() => onLoadMoreDeployments()}
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
                <button
                  type="button"
                  data-testid="load-deployment"
                  onClick={() => void onLoadDeploymentById()}
                >
                  {t('app.loadDeployment')}
                </button>
                {activeDeployment?.state === 'failed' ? (
                  <div data-testid="deployment-failure-panel">
                    <p>{t('app.deploymentFailedTitle')}</p>
                    {(() => {
                      const { code, detail } = deploymentHealthFailureText(
                        activeDeployment.health_summary,
                      )
                      return (
                        <>
                          <p data-testid="deployment-failure-code">
                            {code || t('app.deploymentFailureUnknown')}
                          </p>
                          {detail ? (
                            <p data-testid="deployment-failure-detail">{detail}</p>
                          ) : null}
                        </>
                      )
                    })()}
                    {(() => {
                      const target = latestSucceededRollbackTarget(
                        activeDeployment.environment_slug,
                        activeDeployment.id,
                      )
                      return target ? (
                        <button
                          type="button"
                          data-testid="deployment-rollback-known-good"
                          disabled={deploymentRollbackBusy}
                          onClick={() => void onRollbackToTarget(target.id)}
                        >
                          {t('app.rollbackToKnownGood', {
                            id: target.id.slice(0, 8),
                          })}
                        </button>
                      ) : (
                        <p data-testid="deployment-rollback-unavailable">
                          {t('app.rollbackNoKnownGood')}
                        </p>
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
                    <p data-testid="deployment-plan-hash">
                      {activeDeployment.plan_hash?.slice(0, 12)}…
                    </p>
                    <button
                      type="button"
                      data-testid="approve-deployment"
                      disabled={deploymentApprovalBusy}
                      onClick={() => void onDeploymentDecision('approve')}
                    >
                      {t('app.approveDeployment')}
                    </button>
                    <button
                      type="button"
                      data-testid="reject-deployment"
                      disabled={deploymentApprovalBusy}
                      onClick={() => void onDeploymentDecision('reject')}
                    >
                      {t('app.rejectDeployment')}
                    </button>
                  </div>
                ) : null}
                {deploymentId ? (
                  <button type="button" onClick={() => void onLoadTrace()}>
                    {t('app.viewTrace')}
                  </button>
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
                  <p role="status">{t('app.traceGaps')}: {trace.gaps.join(', ')}</p>
                ) : null}
              </Panel>
            </Panel>
          ) : null}
        </section>
      ) : (
        <form onSubmit={(e) => void onLogin(e)} style={{ maxWidth: '20rem' }}>
          <label>
            {t('app.loginHandle')}
            <input
              data-testid="login-handle-input"
              value={loginForm.handle}
              onChange={(e) =>
                setLoginForm({ ...loginForm, handle: e.target.value })
              }
            />
          </label>
          <label>
            {t('app.loginPassword')}
            <input
              type="password"
              value={loginForm.password}
              onChange={(e) =>
                setLoginForm({ ...loginForm, password: e.target.value })
              }
            />
          </label>
          {error ? <p role="alert">{error}</p> : null}
          <button type="submit">{t('app.login')}</button>
        </form>
      )}
    </main>
  )
}

function Panel({
  children,
  style,
  ...rest
}: React.HTMLAttributes<HTMLDivElement> & {
  children: React.ReactNode
  style?: React.CSSProperties
}) {
  return (
    <div
      style={{
        background: '#fff',
        border: '1px solid #d8dee4',
        borderRadius: '0.5rem',
        padding: '1rem',
        ...style,
      }}
      {...rest}
    >
      {children}
    </div>
  )
}
