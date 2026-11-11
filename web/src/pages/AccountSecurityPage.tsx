import { FormEvent, useCallback, useEffect, useState } from 'react'
import { Panel } from '../components/Panel'
import { useWorkspace } from '../app/WorkspaceProvider'
import { createSshKey, listSshKeys, revokeSshKey, type SshKey } from '../api'

export function AccountSecurityPage() {
  const {
    t,
    passwordChangeForm,
    setPasswordChangeForm,
    onChangePassword,
    passwordChangeBusy,
  } = useWorkspace()
  const [keys, setKeys] = useState<SshKey[]>([])
  const [keyName, setKeyName] = useState('laptop')
  const [publicKey, setPublicKey] = useState('')
  const [sshError, setSshError] = useState<string | null>(null)

  const reloadKeys = useCallback(async () => {
    setKeys(await listSshKeys())
  }, [])

  useEffect(() => {
    void reloadKeys().catch(() => {})
  }, [reloadKeys])

  async function onAddKey(e: FormEvent) {
    e.preventDefault()
    setSshError(null)
    try {
      await createSshKey(keyName, publicKey)
      setPublicKey('')
      await reloadKeys()
    } catch (err) {
      setSshError(err instanceof Error ? err.message : t('sshKeys.addFailed'))
    }
  }

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

      <section data-testid="ssh-keys-section" style={{ marginTop: '2rem' }}>
        <h3>{t('sshKeys.title')}</h3>
        <p role="note">{t('sshKeys.transportGap')}</p>
        <ul data-testid="ssh-key-list">
          {keys.length === 0 ? (
            <li role="status">{t('sshKeys.empty')}</li>
          ) : null}
          {keys.map((k) => (
            <li key={k.id} data-testid="ssh-key-row">
              {k.name} ({k.fingerprint})
              <button
                type="button"
                data-testid={`ssh-key-revoke-${k.id}`}
                onClick={() => void revokeSshKey(k.id).then(reloadKeys)}
              >
                {t('sshKeys.revoke')}
              </button>
            </li>
          ))}
        </ul>
        <form onSubmit={(e) => void onAddKey(e)}>
          <label>
            {t('sshKeys.name')}
            <input data-testid="ssh-key-name" value={keyName} onChange={(e) => setKeyName(e.target.value)} />
          </label>
          <label>
            {t('sshKeys.publicKey')}
            <textarea
              data-testid="ssh-key-public"
              value={publicKey}
              onChange={(e) => setPublicKey(e.target.value)}
              rows={3}
            />
          </label>
          {sshError ? <p role="alert">{sshError}</p> : null}
          <button type="submit" data-testid="ssh-key-add">
            {t('sshKeys.add')}
          </button>
        </form>
      </section>
    </Panel>
  )
}
