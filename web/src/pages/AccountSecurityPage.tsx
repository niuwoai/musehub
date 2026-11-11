import { Panel } from '../components/Panel'
import { useWorkspace } from '../app/WorkspaceProvider'

export function AccountSecurityPage() {
  const {
    t,
    passwordChangeForm,
    setPasswordChangeForm,
    onChangePassword,
    passwordChangeBusy,
  } = useWorkspace()

  return (
    <Panel data-testid="account-security">
      <h2>{t('nav.accountSecurity')}</h2>
      <form onSubmit={(e) => void onChangePassword(e)}>
        <label>
          {t('accountSecurity.currentPassword')}
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
          {t('accountSecurity.newPassword')}
          <input
            data-testid="password-new"
            type="password"
            autoComplete="new-password"
            value={passwordChangeForm.next}
            onChange={(e) => setPasswordChangeForm((f) => ({ ...f, next: e.target.value }))}
          />
        </label>
        <button type="submit" data-testid="change-password" disabled={passwordChangeBusy}>
          {t('accountSecurity.changePassword')}
        </button>
      </form>
    </Panel>
  )
}
