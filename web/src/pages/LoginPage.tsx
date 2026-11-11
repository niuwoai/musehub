import { useWorkspace } from '../app/WorkspaceProvider'

export function LoginPage() {
  const { t, loginForm, setLoginForm, onLogin, error } = useWorkspace()

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
    </form>
  )
}
