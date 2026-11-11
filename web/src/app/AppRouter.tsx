import type { ReactNode } from 'react'
import { Navigate, Route, Routes, useLocation } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { appVersion, useWorkspace } from './WorkspaceProvider'
import { DEFAULT_ORG, paths } from './paths'
import { AppShell } from '../layout/AppShell'
import { LoginPage } from '../pages/LoginPage'
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

function ProtectedRoute({ children }: { children: ReactNode }) {
  const { session } = useWorkspace()
  const location = useLocation()
  if (!session) {
    return <Navigate to={paths.login()} replace state={{ from: location }} />
  }
  return <>{children}</>
}

function LoginRoute() {
  const { session } = useWorkspace()
  const { t } = useTranslation()
  if (session) {
    return <Navigate to={paths.repos(DEFAULT_ORG)} replace />
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
        <p style={{ color: '#57606a' }}>{t('app.tagline', { version: appVersion })}</p>
      </header>
      <LoginPage />
    </main>
  )
}

function RootRedirect() {
  const { session } = useWorkspace()
  if (session) {
    return <Navigate to={paths.repos(DEFAULT_ORG)} replace />
  }
  return <Navigate to={paths.login()} replace />
}

function UnknownProtectedRedirect() {
  return <Navigate to={paths.repos(DEFAULT_ORG)} replace />
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
        <Route path="/org/:org/repos" element={<RepoListPage />} />
        <Route path="/org/:org/settings" element={<OrgSettingsPage />} />
        <Route path="/org/:org/members" element={<OrgMembersPage />} />
        <Route path="/org/:org/audit" element={<AuditPage />} />
        <Route path="/org/:org/runner" element={<RunnerPage />} />
        <Route path="/account/security" element={<AccountSecurityPage />} />
        <Route path="/account/tokens" element={<AccountTokensPage />} />
        <Route path="/org/:org/repos/:slug/issues" element={<RepoIssuesPage />} />
        <Route path="/org/:org/repos/:slug/pulls" element={<RepoPullsPage />} />
        <Route path="/org/:org/repos/:slug/delivery" element={<RepoDeliveryPage />} />
        <Route path="/org/:org/repos/:slug/settings" element={<RepoSettingsPage />} />
        <Route path="*" element={<UnknownProtectedRedirect />} />
      </Route>
    </Routes>
  )
}
