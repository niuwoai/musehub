import {
  FormEvent,
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
} from 'react'
import { useLocation, useMatch, useNavigate } from 'react-router-dom'
import { resolveSafeRedirectPath } from './authRedirect'
import { useTranslation } from 'react-i18next'
import { DEFAULT_ORG, paths } from './paths'

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
  listRepoGrants,
  listRunnerTokens,
  listAuditEvents,
  listOrgMembers,
  listMyTokens,
  changePassword,
  logout,
  getRunnerCapabilities,
  type AuditEvent,
  type OrgMember,
  type PatMasked,
  type RunnerCapabilities,
  getAgentRun,
  getPrCiFailure,
  type PrCiFailure,
  startPrAgentFix,
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
} from '../api'

export const appVersion = import.meta.env.VITE_APP_VERSION

const WorkspaceContext = createContext<unknown>(null)

export function useWorkspace() {
  const ctx = useContext(WorkspaceContext)
  if (!ctx) {
    throw new Error('useWorkspace must be used within WorkspaceProvider')
  }
  return ctx as WorkspaceValue
}

export type SessionAuthState = 'pending' | 'authenticated' | 'anonymous'

export type WorkspaceValue = {
  t: ReturnType<typeof useTranslation>['t']
  i18n: ReturnType<typeof useTranslation>['i18n']
  sessionAuth: SessionAuthState
  session: string | null
  activeOrg: string
  /** @deprecated use activeOrg — kept for pages not yet migrated */
  DEFAULT_ORG: string
  repos: Repository[]
  selectedRepo: string | null
  issues: Issue[]
  pulls: PullRequest[]
  prCiFailure: PrCiFailure | null
  selectedPrNumber: number | null
  lastRun: AgentRun | null
  agentRunStarting: boolean
  error: string | null
  setError: (v: string | null) => void
  loginForm: { handle: string; password: string }
  setLoginForm: React.Dispatch<React.SetStateAction<{ handle: string; password: string }>>
  passwordChangeForm: { current: string; next: string }
  setPasswordChangeForm: React.Dispatch<
    React.SetStateAction<{ current: string; next: string }>
  >
  passwordChangeBusy: boolean
  pipelineRun: PipelineRun | null
  pipelineJobs: PipelineJob[]
  pipelineRetryBusy: boolean
  pipelineCancelBusy: boolean
  deploymentId: string | null
  activeDeployment: Deployment | null
  publishedReleaseId: string | null
  deploymentApprovalBusy: boolean
  deploymentRollbackBusy: boolean
  deploymentLookupId: string
  setDeploymentLookupId: React.Dispatch<React.SetStateAction<string>>
  deploymentSummaries: DeploymentSummary[]
  deploymentListMode: 'pending' | 'all'
  deploymentListLoading: boolean
  deploymentListLoadingMore: boolean
  deploymentListError: string | null
  deploymentListHasMore: boolean
  trace: DeploymentTrace | null
  settings: Awaited<ReturnType<typeof getOrgSettings>> | null
  orgDisplayName: string
  setOrgDisplayName: React.Dispatch<React.SetStateAction<string>>
  grants: { handle: string; role: string }[]
  grantHandle: string
  setGrantHandle: React.Dispatch<React.SetStateAction<string>>
  orgMembers: OrgMember[]
  membersAdmin: boolean
  memberHandle: string
  setMemberHandle: React.Dispatch<React.SetStateAction<string>>
  memberPassword: string
  setMemberPassword: React.Dispatch<React.SetStateAction<string>>
  myTokens: PatMasked[]
  patName: string
  setPatName: React.Dispatch<React.SetStateAction<string>>
  patScopeMode: 'all_accessible' | 'selected'
  setPatScopeMode: React.Dispatch<React.SetStateAction<'all_accessible' | 'selected'>>
  patRepoId: string
  setPatRepoId: React.Dispatch<React.SetStateAction<string>>
  iamBusy: boolean
  auditEvents: AuditEvent[]
  runnerCaps: RunnerCapabilities | null
  runnerTokens: Awaited<ReturnType<typeof listRunnerTokens>>
  mintedPatFull: string | null
  mergedPr: PullRequest | undefined
  onLogin: (event: FormEvent) => Promise<void>
  onLogout: () => Promise<void>
  onChangePassword: (event: FormEvent) => Promise<void>
  onCreateRepo: () => Promise<void>
  loadIssues: (slug: string, preservePrNumber?: number) => Promise<void>
  refreshRepos: () => Promise<void>
  selectRepo: (slug: string) => void
  onCreateIssue: () => Promise<void>
  onAcceptPlan: (issue: Issue) => Promise<void>
  onRunAgent: (issue: Issue) => Promise<void>
  onCancelRun: () => Promise<void>
  onInspectPrCi: (pr: PullRequest) => Promise<void>
  onStartPrAgentFix: () => Promise<void>
  onCreatePrFromRun: () => Promise<void>
  onApproveAndMerge: (pr: PullRequest) => Promise<void>
  onRunDeliveryPipeline: () => Promise<void>
  onCancelPipelineRun: () => Promise<void>
  onRetryPipelineJob: (jobId: string) => Promise<void>
  onReleaseAndDeploy: () => Promise<void>
  onDeployProduction: () => Promise<void>
  onRollbackToTarget: (targetDeploymentId: string) => Promise<void>
  onDeploymentDecision: (decision: 'approve' | 'reject') => Promise<void>
  onLoadTrace: () => Promise<void>
  onLoadDeploymentById: () => Promise<void>
  onPickDeploymentFromList: (id: string) => Promise<void>
  onDeploymentListModeChange: (mode: 'pending' | 'all') => void
  onLoadMoreDeployments: () => void
  refreshDeploymentList: () => Promise<void>
  deploymentHealthFailureText: (
    health: Deployment['health_summary'],
  ) => { code: string; detail: string }
  latestSucceededRollbackTarget: (
    environmentSlug: string,
    excludeId?: string,
  ) => DeploymentSummary | undefined
  formatDeploymentWhen: (iso: string) => string
  refreshIamPanels: () => Promise<void>
  beginIamOperation: () => number
  endIamOperation: (opEpoch: number) => void
  sessionEpochActive: (epoch: number) => boolean
  bumpIamPanelsGeneration: () => void
  iamPanelsGenerationActive: (generation: number) => boolean
  bumpOrgMembersRevision: () => void
  orgMembersRevisionRef: React.MutableRefObject<number>
  sessionHandleRef: React.MutableRefObject<string | null>
  sessionEpochRef: React.MutableRefObject<number>
  iamPanelsGenerationRef: React.MutableRefObject<number>
  setMintedPatFull: React.Dispatch<React.SetStateAction<string | null>>
  setSettings: React.Dispatch<
    React.SetStateAction<Awaited<ReturnType<typeof getOrgSettings>> | null>
  >
  setGrants: React.Dispatch<React.SetStateAction<{ handle: string; role: string }[]>>
  setOrgMembers: React.Dispatch<React.SetStateAction<OrgMember[]>>
  setMembersAdmin: React.Dispatch<React.SetStateAction<boolean>>
}

function useWorkspaceState(): WorkspaceValue {
  const navigate = useNavigate()
  const location = useLocation()
  const orgRouteMatch = useMatch('/org/:org/*')
  const repoRouteMatch = useMatch('/org/:org/repos/:slug/*')
  const repoSlugFromUrl = repoRouteMatch?.params.slug
  const activeOrg = orgRouteMatch?.params.org ?? DEFAULT_ORG
  const { t, i18n } = useTranslation()

  const [sessionAuth, setSessionAuth] = useState<SessionAuthState>('pending')
  const [session, setSession] = useState<string | null>(null)
  const [repos, setRepos] = useState<Repository[]>([])
  const [selectedRepo, setSelectedRepo] = useState<string | null>(null)
  const [issues, setIssues] = useState<Issue[]>([])
  const [pulls, setPulls] = useState<PullRequest[]>([])
  const [prCiFailure, setPrCiFailure] = useState<PrCiFailure | null>(null)
  const [selectedPrNumber, setSelectedPrNumber] = useState<number | null>(null)
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
  const agentRunIdRef = useRef<string | null>(null)
  const agentStartOpRef = useRef<AgentOpContext | null>(null)
  const pipelineRunIdRef = useRef<string | null>(null)

  type DeploymentOpContext = {
    sessionEpoch: number
    repoGeneration: number
    repo: string
  }

  type AgentOpContext = DeploymentOpContext & {
    issueId: string | null
    agentRunId: string | null
    pollEpoch: number
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

  function captureAgentStartOp(issueId: string, repoSlug?: string): AgentOpContext {
    return {
      ...captureDeploymentOp(repoSlug),
      issueId,
      agentRunId: null,
      pollEpoch: agentRunPollEpochRef.current,
    }
  }

  function captureAgentCancelOp(runId: string, repoSlug?: string): AgentOpContext {
    return {
      ...captureDeploymentOp(repoSlug),
      issueId: null,
      agentRunId: runId,
      pollEpoch: agentRunPollEpochRef.current,
    }
  }

  function releaseAgentStartBusy(op: AgentOpContext) {
    if (agentStartOpRef.current && agentOpMatches(agentStartOpRef.current, op)) {
      agentStartOpRef.current = null
      setAgentRunStarting(false)
    }
  }

  function agentOpActive(ctx: AgentOpContext): boolean {
    return (
      deploymentOpActive(ctx) &&
      agentRunPollEpochActive(ctx.pollEpoch)
    )
  }

  function agentOpMatches(a: AgentOpContext, b: AgentOpContext): boolean {
    return (
      a.sessionEpoch === b.sessionEpoch &&
      a.repoGeneration === b.repoGeneration &&
      a.repo === b.repo &&
      a.issueId === b.issueId &&
      a.agentRunId === b.agentRunId &&
      a.pollEpoch === b.pollEpoch
    )
  }

  function applyAgentRun(run: AgentRun | null) {
    agentRunIdRef.current = run?.id ?? null
    setLastRun(run)
  }

  function resetAgentRunUi() {
    agentRunIdRef.current = null
    agentStartOpRef.current = null
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

  function clearWorkspaceState() {
    setError(null)
    setRepos([])
    setIssues([])
    setPulls([])
    setPrCiFailure(null)
    setSelectedPrNumber(null)
    resetAgentRunUi()
    invalidateDeploymentWork()
    setSettings(null)
    setOrgDisplayName('')
    setGrants([])
    setGrantHandle('')
    setMemberHandle('')
    setMemberPassword('')
    setPatName('')
    setPatScopeMode('all_accessible')
    setPatRepoId('')
    setLoginForm((prev) => ({ ...prev, password: '' }))
    setPasswordChangeForm({ current: '', next: '' })
    setPasswordChangeBusy(false)
    setMintedPatFull(null)
    selectedRepoRef.current = null
    setSelectedRepo(null)
    setAuditEvents([])
    setRunnerCaps(null)
    setRunnerTokens([])
    setMyTokens([])
    setOrgMembers([])
    setMembersAdmin(false)
    bumpOrgMembersRevision()
  }

  useEffect(() => {
    let cancelled = false
    void me()
      .then((principal) => {
        if (cancelled) {
          return
        }
        setSession(principal.handle)
        setSessionAuth('authenticated')
      })
      .catch(() => {
        if (cancelled) {
          return
        }
        setSession(null)
        setSessionAuth('anonymous')
      })
    return () => {
      cancelled = true
    }
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
    if (sessionAuth !== 'authenticated' || !session) {
      return
    }
    const prevHandle = sessionHandleRef.current
    sessionHandleRef.current = session
    const identityChanged =
      prevHandle !== null && session !== null && prevHandle !== session
    if (identityChanged) {
      clearWorkspaceState()
    }
    const epoch = bumpSessionEpoch()
    bumpIamPanelsGeneration()
    resetIamBusy()
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
      const audit = await listAuditEvents(activeOrg, 30).catch(() => [] as AuditEvent[])
      if (!sessionEpochActive(epoch) || !iamPanelsGenerationActive(panelsGeneration)) {
        return
      }
      setAuditEvents(audit)
      const runner = await listRunnerTokens(activeOrg).catch(
        () => [] as Awaited<ReturnType<typeof listRunnerTokens>>,
      )
      if (!sessionEpochActive(epoch) || !iamPanelsGenerationActive(panelsGeneration)) {
        return
      }
      setRunnerTokens(runner)
      try {
        const members = await listOrgMembers(activeOrg)
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
  }, [session, sessionAuth])

  useEffect(() => {
    if (sessionAuth !== 'anonymous') {
      return
    }
    sessionHandleRef.current = null
    clearWorkspaceState()
  }, [sessionAuth])

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
    const audit = await listAuditEvents(activeOrg, 40).catch(() => null)
    if (sessionEpochActive(epoch) && iamPanelsGenerationActive(panelsGeneration) && audit) {
      setAuditEvents(audit)
    }
    const members = await listOrgMembers(activeOrg).catch(() => null)
    if (sessionEpochActive(epoch) && iamPanelsGenerationActive(panelsGeneration) && members) {
      bumpOrgMembersRevision()
      setOrgMembers(members)
      setMembersAdmin(true)
    }
  }

  async function onLogout() {
    bumpSessionEpoch()
    bumpIamPanelsGeneration()
    try {
      await logout()
    } catch {
      /* cookie may already be cleared */
    }
    clearWorkspaceState()
    resetIamBusy()
    setSession(null)
    setSessionAuth('anonymous')
    navigate(paths.login(), { replace: true })
  }

  const selectRepo = useCallback(
    (slug: string) => {
      navigate(paths.repoIssues(activeOrg, slug))
      void loadIssues(slug)
    },
    [navigate, activeOrg],
  )

  useEffect(() => {
    if (!session || !repoSlugFromUrl) {
      return
    }
    if (selectedRepoRef.current !== repoSlugFromUrl) {
      void loadIssues(repoSlugFromUrl)
    }
  }, [session, repoSlugFromUrl])

  async function refreshRepos() {
    const data = await listRepos(activeOrg)
    setRepos(data)
  }

  async function onLogin(event: FormEvent) {
    event.preventDefault()
    setError(null)
    const passwordUsed = loginForm.password
    try {
      await login(loginForm.handle, passwordUsed)
      const principal = await me()
      clearWorkspaceState()
      resetIamBusy()
      bumpSessionEpoch()
      bumpIamPanelsGeneration()
      setSession(principal.handle)
      setSessionAuth('authenticated')
      setLoginForm((prev) => ({ ...prev, password: '' }))
      await Promise.all([refreshRepos(), refreshIamPanels()])
      const from = (location.state as { from?: { pathname: string; search: string; hash: string } } | null)
        ?.from
      navigate(resolveSafeRedirectPath(from ?? null), { replace: true })
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
      const audit = await listAuditEvents(activeOrg, 30).catch(() => null)
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
    await createRepo(activeOrg, slug)
    await refreshRepos()
    navigate(paths.repoIssues(activeOrg, slug))
    await loadIssues(slug)
  }

  async function loadIssues(slug: string, preservePrNumber?: number) {
    if (selectedRepoRef.current !== slug) {
      invalidateAgentRunWork()
    }
    invalidateDeploymentWork()
    selectedRepoRef.current = slug
    setSelectedRepo(slug)
    if (preservePrNumber == null) {
      setPrCiFailure(null)
      setSelectedPrNumber(null)
    }
    const op = captureDeploymentOp(slug)
    setError(null)
    try {
      const prs = await listPullRequests(activeOrg, slug)
      const data = await listIssues(activeOrg, slug)
      const s = await getOrgSettings(activeOrg)
      const g = await listRepoGrants(activeOrg, slug)
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
      const list = await listDeployments(activeOrg, ctx.repo, {
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
      const dep = await getDeployment(activeOrg, selectedRepo, id)
      if (!deploymentOpActive(op)) {
        return
      }
      setActiveDeployment(dep)
      setDeploymentId(dep.id)
      if (dep.state !== 'awaiting_approval') {
        const tv = await getDeploymentTrace(activeOrg, selectedRepo, dep.id)
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
    await createIssue(activeOrg, selectedRepo, 'Slice issue')
    await loadIssues(selectedRepo)
  }

  async function onAcceptPlan(issue: Issue) {
    if (!selectedRepo) return
    await createPlan(activeOrg, selectedRepo, issue.number, 'UI plan draft')
    await acceptPlan(activeOrg, selectedRepo, issue.number)
  }

  async function pollRun(runId: string, op: AgentOpContext) {
    const remoteAgent = runnerCaps?.agent_executor === 'remote'
    const maxIter = remoteAgent ? 600 : 360
    for (let i = 0; i < maxIter; i++) {
      if (!agentOpActive(op)) {
        return null
      }
      const run = await getAgentRun(runId)
      if (!agentOpActive(op)) {
        return null
      }
      if (agentRunIdRef.current !== null && agentRunIdRef.current !== run.id) {
        return null
      }
      applyAgentRun(run)
      if (['succeeded', 'failed', 'canceled'].includes(run.state)) {
        return run
      }
      await new Promise((resolve) => setTimeout(resolve, 500))
    }
    throw new Error('agent run timeout')
  }

  async function onRunAgent(issue: Issue) {
    if (!selectedRepo || agentRunStarting) return
    bumpAgentRunPollEpoch()
    const op = captureAgentStartOp(issue.id)
    agentStartOpRef.current = op
    setAgentRunStarting(true)
    setError(null)
    try {
      const run = await startAgentRun(
        activeOrg,
        selectedRepo,
        issue.id,
        `Implement: ${issue.title}`,
      )
      if (!agentOpActive(op)) {
        return
      }
      applyAgentRun(run)
      const finished = await pollRun(run.id, op)
      if (!finished || !agentOpActive(op)) {
        return
      }
      applyAgentRun(finished)
      await loadIssues(selectedRepo)
    } catch (err) {
      if (agentOpActive(op)) {
        setError(err instanceof Error ? err.message : 'agent run failed')
      }
    } finally {
      releaseAgentStartBusy(op)
    }
  }

  async function onCancelRun() {
    if (!selectedRepo || !lastRun) return
    const heldStart = agentStartOpRef.current
    const runId = lastRun.id
    bumpAgentRunPollEpoch()
    if (heldStart) {
      releaseAgentStartBusy(heldStart)
    }
    const op = captureAgentCancelOp(runId)
    setError(null)
    try {
      await cancelAgentRun(runId)
      if (!agentOpActive(op)) {
        return
      }
      const finished = await pollRun(runId, op)
      if (finished && agentOpActive(op)) {
        applyAgentRun(finished)
      }
    } catch (err) {
      if (agentOpActive(op)) {
        setError(err instanceof Error ? err.message : 'agent cancel failed')
      }
    } finally {
      if (heldStart) {
        releaseAgentStartBusy(heldStart)
      }
    }
  }

  async function onInspectPrCi(pr: PullRequest) {
    if (!selectedRepo) return
    setSelectedPrNumber(pr.number)
    setError(null)
    try {
      const ctx = await getPrCiFailure(activeOrg, selectedRepo, pr.number)
      setPrCiFailure(ctx)
    } catch (err) {
      setPrCiFailure(null)
      setError(err instanceof Error ? err.message : 'CI failure lookup failed')
    }
  }

  async function onStartPrAgentFix() {
    if (!selectedRepo || !prCiFailure?.agent_fix_allowed || !prCiFailure.issue_id) return
    if (agentRunStarting) return
    bumpAgentRunPollEpoch()
    const op = captureAgentStartOp(prCiFailure.issue_id)
    agentStartOpRef.current = op
    setAgentRunStarting(true)
    setError(null)
    const prNumber = prCiFailure.pr_number
    try {
      const run = await startPrAgentFix(activeOrg, selectedRepo, prNumber)
      if (!agentOpActive(op)) {
        return
      }
      applyAgentRun({
        id: run.id,
        adapter_kind: run.adapter_kind,
        state: run.state,
        result_commit_oid: null,
        usage_tokens: run.usage_tokens,
        budget_unknown: run.budget_unknown,
      })
      const finished = await pollRun(run.id, op)
      if (!finished || !agentOpActive(op)) {
        return
      }
      applyAgentRun(finished)
      const ctx = await getPrCiFailure(activeOrg, selectedRepo, prNumber)
      if (agentOpActive(op)) {
        setPrCiFailure(ctx)
      }
      await loadIssues(selectedRepo, prNumber)
    } catch (err) {
      if (agentOpActive(op)) {
        setError(err instanceof Error ? err.message : 'agent fix failed')
      }
    } finally {
      releaseAgentStartBusy(op)
    }
  }

  async function onCreatePrFromRun() {
    if (!selectedRepo || !lastRun?.result_commit_oid) return
    const branch = `agent/${lastRun.id}`
    await createPullRequest(activeOrg, selectedRepo, `PR for ${branch}`, branch)
    await loadIssues(selectedRepo)
    navigate(paths.repoPulls(activeOrg, selectedRepo))
  }

  const mergedPr = pulls.find((p) => p.state === 'merged' && p.head_oid)

  async function onApproveAndMerge(pr: PullRequest) {
    if (!selectedRepo) return
    setError(null)
    try {
      await submitReview(activeOrg, selectedRepo, pr.number, 'approve')
      const attempt = crypto.randomUUID()
      await setPullRequestCheck(
        activeOrg,
        selectedRepo,
        pr.number,
        'product-ci',
        'success',
        attempt,
      )
      await mergePullRequest(activeOrg, selectedRepo, pr.number)
      await loadIssues(selectedRepo)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'merge failed')
    }
  }

  async function loadPipelineJobsForRun(runId: string) {
    if (!selectedRepo) return
    const op = capturePipelineOp(runId)
    const jobs = await listPipelineJobs(activeOrg, selectedRepo, runId)
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
      const run = await getPipeline(activeOrg, repo, runId)
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
      const run = await startPipeline(activeOrg, selectedRepo, mergedPr.head_oid)
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
      const requested = await cancelPipelineRun(activeOrg, selectedRepo, runId)
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
      await retryPipelineJob(activeOrg, selectedRepo, jobId, attemptId)
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
      const rel = await createRelease(activeOrg, selectedRepo, {
        version,
        tag_name: `v${version}`,
        commit_oid: mergedPr.head_oid,
        pipeline_run_id: pipelineRun.id,
        pr_id: mergedPr.id,
      })
      await publishRelease(activeOrg, selectedRepo, rel.id)
      if (!deploymentOpActive(op)) {
        return
      }
      setPublishedReleaseId(rel.id)
      const dep = await startDeployment(activeOrg, selectedRepo, rel.id)
      if (!deploymentOpActive(op)) {
        return
      }
      setDeploymentId(dep.id)
      setActiveDeployment(dep)
      const tv = await getDeploymentTrace(activeOrg, selectedRepo, dep.id)
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
        activeOrg,
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
        const tv = await getDeploymentTrace(activeOrg, selectedRepo, dep.id)
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
        activeOrg,
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
        const tv = await getDeploymentTrace(activeOrg, selectedRepo, dep.id)
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
        activeOrg,
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
        activeOrg,
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
          activeOrg,
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
        activeOrg,
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
      const dep = await getDeployment(activeOrg, selectedRepo, lookupId)
      if (!deploymentOpActive(op)) {
        return
      }
      setActiveDeployment(dep)
      setDeploymentId(dep.id)
      if (dep.state !== 'awaiting_approval') {
        const tv = await getDeploymentTrace(
          activeOrg,
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

  const refreshDeploymentListForUi = useCallback(async () => {
    await refreshDeploymentList()
  }, [])

  return {
    t,
    i18n,
    sessionAuth,
    session,
    activeOrg,
    repos,
    selectedRepo,
    issues,
    pulls,
    prCiFailure,
    selectedPrNumber,
    lastRun,
    agentRunStarting,
    error,
    setError,
    loginForm,
    setLoginForm,
    passwordChangeForm,
    setPasswordChangeForm,
    passwordChangeBusy,
    pipelineRun,
    pipelineJobs,
    pipelineRetryBusy,
    pipelineCancelBusy,
    deploymentId,
    activeDeployment,
    publishedReleaseId,
    deploymentApprovalBusy,
    deploymentRollbackBusy,
    deploymentLookupId,
    setDeploymentLookupId,
    deploymentSummaries,
    deploymentListMode,
    deploymentListLoading,
    deploymentListLoadingMore,
    deploymentListError,
    deploymentListHasMore,
    trace,
    settings,
    orgDisplayName,
    setOrgDisplayName,
    grants,
    grantHandle,
    setGrantHandle,
    orgMembers,
    membersAdmin,
    memberHandle,
    setMemberHandle,
    memberPassword,
    setMemberPassword,
    myTokens,
    patName,
    setPatName,
    patScopeMode,
    setPatScopeMode,
    patRepoId,
    setPatRepoId,
    iamBusy,
    auditEvents,
    runnerCaps,
    runnerTokens,
    mintedPatFull,
    mergedPr,
    DEFAULT_ORG: activeOrg,
    onLogin,
    onLogout,
    onChangePassword,
    onCreateRepo,
    loadIssues,
    refreshRepos,
    selectRepo,
    onCreateIssue,
    onAcceptPlan,
    onRunAgent,
    onCancelRun,
    onInspectPrCi,
    onStartPrAgentFix,
    onCreatePrFromRun,
    onApproveAndMerge,
    onRunDeliveryPipeline,
    onCancelPipelineRun,
    onRetryPipelineJob,
    onReleaseAndDeploy,
    onDeployProduction,
    onRollbackToTarget,
    onDeploymentDecision,
    onLoadTrace,
    onLoadDeploymentById,
    onPickDeploymentFromList,
    onDeploymentListModeChange,
    onLoadMoreDeployments,
    refreshDeploymentList: refreshDeploymentListForUi,
    deploymentHealthFailureText,
    latestSucceededRollbackTarget,
    formatDeploymentWhen,
    refreshIamPanels,
    beginIamOperation,
    endIamOperation,
    sessionEpochActive,
    bumpIamPanelsGeneration,
    iamPanelsGenerationActive,
    bumpOrgMembersRevision,
    orgMembersRevisionRef,
    sessionHandleRef,
    sessionEpochRef,
    iamPanelsGenerationRef,
    setMintedPatFull,
    setSettings,
    setGrants,
    setOrgMembers,
    setMembersAdmin,
  }
}

export function WorkspaceProvider({ children }: { children: React.ReactNode }) {
  const value = useWorkspaceState()
  return <WorkspaceContext.Provider value={value}>{children}</WorkspaceContext.Provider>
}
