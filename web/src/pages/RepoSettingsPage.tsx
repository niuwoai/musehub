import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  getOrgSettings,
  grantRepoCollaborator,
  listRepoEnvironments,
  listRepoGrants,
  revokeRepoGrant,
  type RepoEnvironment,
} from '../api'
import { Panel } from '../components/Panel'
import { useWorkspace } from '../app/WorkspaceProvider'

const GRANT_ROLES = ['reader', 'developer', 'maintainer'] as const

export function RepoSettingsPage() {
  const { t } = useTranslation()
  const {
    selectedRepo,
    grantHandle,
    setGrantHandle,
    setGrants,
    activeOrg,
    grants,
  } = useWorkspace()
  const [environments, setEnvironments] = useState<RepoEnvironment[]>([])
  const [grantRole, setGrantRole] = useState<string>('reader')
  const [grantError, setGrantError] = useState<string | null>(null)
  const [grantsBusy, setGrantsBusy] = useState(false)

  const reloadGrants = useCallback(async () => {
    if (!selectedRepo) return
    const rows = await listRepoGrants(activeOrg, selectedRepo)
    setGrants(rows)
  }, [activeOrg, selectedRepo, setGrants])

  useEffect(() => {
    if (!selectedRepo) return
    void listRepoEnvironments(activeOrg, selectedRepo)
      .then(setEnvironments)
      .catch(() => setEnvironments([]))
    void getOrgSettings(activeOrg)
      .then((s) => {
        const defaultRole = s.preferences?.default_collaborator_role
        if (defaultRole && GRANT_ROLES.includes(defaultRole as (typeof GRANT_ROLES)[number])) {
          setGrantRole(defaultRole)
        }
      })
      .catch(() => {})
    void reloadGrants().catch(() => {})
  }, [activeOrg, selectedRepo, reloadGrants])

  if (!selectedRepo) {
    return <p role="status">{t('repo.selectFirst')}</p>
  }
  const repoSlug = selectedRepo

  async function submitGrant() {
    const handle = grantHandle.trim()
    if (!handle) return
    setGrantError(null)
    setGrantsBusy(true)
    try {
      await grantRepoCollaborator(activeOrg, repoSlug, handle, grantRole)
      setGrantHandle('')
      await reloadGrants()
    } catch (e) {
      setGrantError(e instanceof Error ? e.message : t('repoSettings.grantFailed'))
    } finally {
      setGrantsBusy(false)
    }
  }

  async function removeGrant(principalId: string) {
    setGrantError(null)
    setGrantsBusy(true)
    try {
      await revokeRepoGrant(activeOrg, repoSlug, principalId)
      await reloadGrants()
    } catch (e) {
      setGrantError(e instanceof Error ? e.message : t('repoSettings.revokeFailed'))
    } finally {
      setGrantsBusy(false)
    }
  }

  return (
    <Panel data-testid="repo-settings-panel">
      <h2>{t('nav.repoSettings')}</h2>
      <div data-testid="repo-environments">
        <strong>{t('repoSettings.environmentsTitle')}</strong>
        <ul>
          {environments.map((e) => (
            <li key={e.id} data-testid={`env-${e.slug}`}>
              {e.slug} — {e.name}
              {e.requires_approval ? ` (${t('repoSettings.approvalRequired')})` : ''}
            </li>
          ))}
        </ul>
      </div>
      <div data-testid="repo-grants">
        <strong>{t('repoSettings.grantsTitle')}</strong>
        <ul data-testid="repo-grants-list">
          {grants.map((g) => (
            <li key={g.principal_id} data-testid="repo-grant-row">
              {g.handle} ({g.role}){' '}
              <button
                type="button"
                data-testid="repo-grant-revoke"
                disabled={grantsBusy}
                onClick={() => void removeGrant(g.principal_id)}
              >
                {t('repoSettings.revokeGrant')}
              </button>
            </li>
          ))}
        </ul>
        <input
          data-testid="repo-grant-handle"
          placeholder={t('repoSettings.grantHandlePlaceholder')}
          value={grantHandle}
          onChange={(e) => setGrantHandle(e.target.value)}
        />
        <label>
          {t('repoSettings.grantRole')}
          <select
            data-testid="repo-grant-role"
            value={grantRole}
            onChange={(e) => setGrantRole(e.target.value)}
          >
            {GRANT_ROLES.map((role) => (
              <option key={role} value={role}>
                {t(`orgSettings.repoRole.${role}`)}
              </option>
            ))}
          </select>
        </label>
        <button
          type="button"
          data-testid="repo-grant-submit"
          disabled={grantsBusy || !grantHandle.trim()}
          onClick={() => void submitGrant()}
        >
          {t('repoSettings.grantSubmit')}
        </button>
        {grantError ? <p role="alert">{grantError}</p> : null}
      </div>
    </Panel>
  )
}
