import { useEffect, useState } from 'react'
import { useLocation } from 'react-router-dom'
import { getAuthProviders } from '../api'
import { resolveSafeRedirectPath } from '../app/authRedirect'
import { useWorkspace } from '../app/WorkspaceProvider'

export function LoginPage() {
  const { t, loginForm, setLoginForm, onLogin, error } = useWorkspace()
  const location = useLocation()
  const [oidcEnabled, setOidcEnabled] = useState(false)
  const [githubEnabled, setGithubEnabled] = useState(false)
  const [samlEnabled, setSamlEnabled] = useState(false)
  const from = (location.state as { from?: { pathname: string; search: string; hash: string } } | null)
    ?.from
  const returnTo = resolveSafeRedirectPath(from)
  const oidcUrl = `/api/v1/auth/oidc/start?return_to=${encodeURIComponent(returnTo)}`
  const githubUrl = `/api/v1/auth/github/start?return_to=${encodeURIComponent(returnTo)}`
  const samlUrl = `/api/v1/auth/saml/start?return_to=${encodeURIComponent(returnTo)}`

  useEffect(() => {
    let active = true
    void getAuthProviders()
      .then((providers) => {
        if (active) {
          setOidcEnabled(providers.oidc)
          setGithubEnabled(providers.github)
          setSamlEnabled(providers.saml)
        }
      })
      .catch(() => {
        if (active) {
          setOidcEnabled(false)
          setGithubEnabled(false)
          setSamlEnabled(false)
        }
      })
    return () => {
      active = false
    }
  }, [])

  return (
    <form onSubmit={(e) => void onLogin(e)} style={{ maxWidth: '20rem' }}>
      <label>
        {t('app.loginHandle')}
        <input
          data-testid="login-handle-input"
          value={loginForm.handle}
          onChange={(e) => setLoginForm({ ...loginForm, handle: e.target.value })}
        />
      </label>
      <label>
        {t('app.loginPassword')}
        <input
          type="password"
          value={loginForm.password}
          onChange={(e) => setLoginForm({ ...loginForm, password: e.target.value })}
        />
      </label>
      {error ? <p role="alert">{error}</p> : null}
      <button type="submit">{t('app.login')}</button>
      {oidcEnabled ? (
        <p>
          <a href={oidcUrl}>{t('app.loginWithOidc')}</a>
        </p>
      ) : null}
      {githubEnabled ? (
        <p>
          <a href={githubUrl}>{t('app.loginWithGithub')}</a>
        </p>
      ) : null}
      {samlEnabled ? (
        <p>
          <a href={samlUrl}>{t('app.loginWithSaml')}</a>
        </p>
      ) : null}
    </form>
  )
}
