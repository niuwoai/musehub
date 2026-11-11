import type { ReactNode } from 'react'
import type { Location } from 'react-router-dom'
import { Navigate, Route, Routes, useLocation, useParams } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { resolveSafeRedirectPath } from './authRedirect'
import { appVersion, useWorkspace } from './WorkspaceProvider'
import { DEFAULT_ORG, paths } from './paths'
import { OrgScopeGuard } from '../components/OrgScopeGuard'
import { AppShell } from '../layout/AppShell'
import { LoginPage } from '../pages/LoginPage'
import { NotFoundPage } from '../pages/NotFoundPage'
import { RepoListPage } from '../pages/RepoListPage'
import { OrgSettingsPage } from '../pages/OrgSettingsPage'
import { OrgMembersPage } from '../pages/OrgMembersPage'
import { AuditPage } from '../pages/AuditPage'
import { RunnerPage } from '../pages/RunnerPage'
import { AccountSecurityPage } from '../pages/AccountSecurityPage'
import { AccountTokensPage } from '../pages/AccountTokensPage'
import { RepoIssuesPage } from '../pages/RepoIssuesPage'
import { RepoPullsPage } from '../pages/RepoPullsPage'
import { RepoDeliveryPage } from '../pages/RepoDeliveryPage'
import { RepoSettingsPage } from '../pages/RepoSettingsPage'
import { SessionBootScreen } from '../pages/SessionBootScreen'

function ProtectedRoute({ children }: { children: ReactNode }) {
  const { sessionAuth, session } = useWorkspace()
  const location = useLocation()
  if (sessionAuth === 'pending') {
    return <SessionBootScreen />
  }
  if (!session) {
    return <Navigate to={paths.login()} replace state={{ from: location }} />
  }
  return <>{children}</>
}

function LoginRoute() {
  const { sessionAuth, session } = useWorkspace()
  const location = useLocation()
  const { t } = useTranslation()
  if (sessionAuth === 'pending') {
    return <SessionBootScreen />
  }
  if (session) {
    const target = resolveSafeRedirectPath(
      (location.state as { from?: Location } | null)?.from ?? null,
    )
    return <Navigate to={target} replace />
  }
  return (
    <main
      style={{
        minHeight: '100vh',
        margin: 0,
        padding: '2rem',
        fontFamily:
          "-apple-system, BlinkMacSystemFont, 'Segoe UI', 'Noto Sans SC', Roboto, sans-serif",
        color: '#1f2328',
        background: '#f6f7f9',
      }}
    >
      <header style={{ marginBottom: '1.5rem' }}>
        <h1 style={{ margin: 0 }}>{t('app.name')}</h1>
        <p style={{ color: '#57606a' }}>{t('app.tagline', { version: appVersion })}</p>
      </header>
      <LoginPage />
    </main>
  )
}

function RootRedirect() {
  const { sessionAuth, session } = useWorkspace()
  if (sessionAuth === 'pending') {
    return <SessionBootScreen />
  }
  if (session) {
    return <Navigate to={paths.repos(DEFAULT_ORG)} replace />
  }
  return <Navigate to={paths.login()} replace />
}

function RepoTabRedirect() {
  const { org, slug } = useParams()
  if (!org || !slug) {
    return <NotFoundPage />
  }
  return <Navigate to={paths.repoIssues(org, slug)} replace />
}

export function AppRouter() {
  return (
    <Routes>
      <Route path="/" element={<RootRedirect />} />
      <Route path="/login" element={<LoginRoute />} />
      <Route
        element={
          <ProtectedRoute>
            <AppShell />
          </ProtectedRoute>
        }
      >
        <Route
          path="/org/:org/repos"
          element={
            <OrgScopeGuard>
              <RepoListPage />
            </OrgScopeGuard>
          }
        />
        <Route
          path="/org/:org/settings"
          element={
            <OrgScopeGuard>
              <OrgSettingsPage />
            </OrgScopeGuard>
          }
        />
        <Route
          path="/org/:org/members"
          element={
            <OrgScopeGuard>
              <OrgMembersPage />
            </OrgScopeGuard>
          }
        />
        <Route
          path="/org/:org/audit"
          element={
            <OrgScopeGuard>
              <AuditPage />
            </OrgScopeGuard>
          }
        />
        <Route
          path="/org/:org/runner"
          element={
            <OrgScopeGuard>
              <RunnerPage />
            </OrgScopeGuard>
          }
        />
        <Route path="/account/security" element={<AccountSecurityPage />} />
        <Route path="/account/tokens" element={<AccountTokensPage />} />
        <Route
          path="/org/:org/repos/:slug"
          element={
            <OrgScopeGuard>
              <RepoTabRedirect />
            </OrgScopeGuard>
          }
        />
        <Route
          path="/org/:org/repos/:slug/issues"
          element={
            <OrgScopeGuard>
              <RepoIssuesPage />
            </OrgScopeGuard>
          }
        />
        <Route
          path="/org/:org/repos/:slug/pulls"
          element={
            <OrgScopeGuard>
              <RepoPullsPage />
            </OrgScopeGuard>
          }
        />
        <Route
          path="/org/:org/repos/:slug/delivery"
          element={
            <OrgScopeGuard>
              <RepoDeliveryPage />
            </OrgScopeGuard>
          }
        />
        <Route
          path="/org/:org/repos/:slug/settings"
          element={
            <OrgScopeGuard>
              <RepoSettingsPage />
            </OrgScopeGuard>
          }
        />
        <Route path="*" element={<NotFoundPage />} />
      </Route>
    </Routes>
  )
}
