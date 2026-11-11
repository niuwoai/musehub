import { NavLink, Outlet, useParams } from 'react-router-dom'
import { appVersion, useWorkspace } from '../app/WorkspaceProvider'
import { DEFAULT_ORG, paths } from '../app/paths'

export function AppShell() {
  const { t, i18n, session, onLogout, error } = useWorkspace()
  const { org = DEFAULT_ORG, slug } = useParams()

  const isChinese = i18n.language.startsWith('zh')
  const nextLanguage = isChinese ? 'en' : 'zh'

  return (
    <div className="app-shell">
      <header className="app-header">
        <div>
          <h1 className="app-title">{t('app.name')}</h1>
          <p className="app-tagline">{t('app.tagline', { version: appVersion })}</p>
          <p className="app-tagline">{t('nav.orgLabel', { org })}</p>
        </div>
        <div className="app-header-actions">
          <span>
            {t('app.signedInAs')}{' '}
            <strong data-testid="session-handle">{session}</strong>
          </span>
          <button type="button" data-testid="logout" onClick={() => void onLogout()}>
            {t('app.logout')}
          </button>
          <button type="button" onClick={() => void i18n.changeLanguage(nextLanguage)}>
            {t('app.switchLanguage')}
          </button>
        </div>
      </header>

      {error ? (
        <p role="alert" data-testid="app-alert" className="app-alert">
          {error}
        </p>
      ) : null}

      <div className="app-body">
        <nav className="app-sidebar" aria-label="Main">
          <NavLink to={paths.repos(org)} className="repo-tab" data-testid="nav-repos">
            {t('nav.repos')}
          </NavLink>
          <NavLink
            to={paths.orgSettings(org)}
            className="repo-tab"
            data-testid="nav-org-settings"
          >
            {t('nav.orgSettings')}
          </NavLink>
          <NavLink to={paths.members(org)} className="repo-tab" data-testid="nav-members">
            {t('nav.orgMembers')}
          </NavLink>
          <NavLink to={paths.audit(org)} className="repo-tab" data-testid="nav-audit">
            {t('nav.audit')}
          </NavLink>
          <NavLink to={paths.runner(org)} className="repo-tab" data-testid="nav-runner">
            {t('nav.runner')}
          </NavLink>
          <p className="nav-group-label">{t('nav.accountSecurity')}</p>
          <NavLink
            to={paths.accountSecurity()}
            className="repo-tab"
            data-testid="nav-account-security"
          >
            {t('nav.accountSecurity')}
          </NavLink>
          <NavLink
            to={paths.accountTokens()}
            className="repo-tab"
            data-testid="nav-account-tokens"
          >
            {t('nav.accountTokens')}
          </NavLink>
          {slug ? (
            <>
              <p className="nav-group-label" data-testid="repo-subnav-title">
                {slug}
              </p>
              <NavLink
                to={paths.repoIssues(org, slug)}
                className="repo-tab"
                data-testid="nav-repo-issues"
              >
                {t('nav.repoIssues')}
              </NavLink>
              <NavLink
                to={paths.repoPulls(org, slug)}
                className="repo-tab"
                data-testid="nav-repo-pulls"
              >
                {t('nav.repoPulls')}
              </NavLink>
              <NavLink
                to={paths.repoDelivery(org, slug)}
                className="repo-tab"
                data-testid="nav-repo-delivery"
              >
                {t('nav.repoDelivery')}
              </NavLink>
              <NavLink
                to={paths.repoSettings(org, slug)}
                className="repo-tab"
                data-testid="nav-repo-settings"
              >
                {t('nav.repoSettings')}
              </NavLink>
            </>
          ) : null}
        </nav>
        <div className="app-main">
          <Outlet />
        </div>
      </div>
    </div>
  )
}
