import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  createRepoEnvironment,
  getOrgSettings,
  grantRepoCollaborator,
  listRepoEnvironments,
  listRepoGrants,
  patchRepoEnvironment,
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
  const [envSlug, setEnvSlug] = useState('')
  const [envName, setEnvName] = useState('')
  const [envApproval, setEnvApproval] = useState(false)
  const [envError, setEnvError] = useState<string | null>(null)
  const [envBusy, setEnvBusy] = useState(false)

  const reloadEnvironments = useCallback(async () => {
    if (!selectedRepo) return
    const rows = await listRepoEnvironments(activeOrg, selectedRepo)
    setEnvironments(rows)
  }, [activeOrg, selectedRepo])

  const reloadGrants = useCallback(async () => {
    if (!selectedRepo) return
    const rows = await listRepoGrants(activeOrg, selectedRepo)
    setGrants(rows)
  }, [activeOrg, selectedRepo, setGrants])

  useEffect(() => {
    if (!selectedRepo) return
    void reloadEnvironments().catch(() => setEnvironments([]))
    void getOrgSettings(activeOrg)
      .then((s) => {
        const defaultRole = s.preferences?.default_collaborator_role
        if (defaultRole && GRANT_ROLES.includes(defaultRole as (typeof GRANT_ROLES)[number])) {
          setGrantRole(defaultRole)
        }
      })
      .catch(() => {})
    void reloadGrants().catch(() => {})
  }, [activeOrg, selectedRepo, reloadEnvironments, reloadGrants])

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

  async function submitEnvironment() {
    const slug = envSlug.trim()
    const name = envName.trim()
    if (!slug || !name) return
    setEnvError(null)
    setEnvBusy(true)
    try {
      await createRepoEnvironment(activeOrg, repoSlug, {
        slug,
        name,
        requires_approval: envApproval,
      })
      setEnvSlug('')
      setEnvName('')
      setEnvApproval(false)
      await reloadEnvironments()
    } catch (e) {
      setEnvError(e instanceof Error ? e.message : t('repoSettings.envCreateFailed'))
    } finally {
      setEnvBusy(false)
    }
  }

  async function toggleEnvApproval(env: RepoEnvironment) {
    setEnvError(null)
    setEnvBusy(true)
    try {
      await patchRepoEnvironment(activeOrg, repoSlug, env.slug, {
        requires_approval: !env.requires_approval,
        expected_row_version: env.row_version,
      })
      await reloadEnvironments()
    } catch (e) {
      setEnvError(e instanceof Error ? e.message : t('repoSettings.envPatchFailed'))
    } finally {
      setEnvBusy(false)
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
              {e.requires_approval ? ` (${t('repoSettings.approvalRequired')})` : ''}{' '}
              <button
                type="button"
                data-testid={`env-toggle-approval-${e.slug}`}
                disabled={envBusy}
                onClick={() => void toggleEnvApproval(e)}
              >
                {t('repoSettings.envToggleApproval')}
              </button>
            </li>
          ))}
        </ul>
        <div data-testid="repo-env-create">
          <input
            data-testid="repo-env-slug"
            placeholder={t('repoSettings.envSlugPlaceholder')}
            value={envSlug}
            onChange={(ev) => setEnvSlug(ev.target.value)}
          />
          <input
            data-testid="repo-env-name"
            placeholder={t('repoSettings.envNamePlaceholder')}
            value={envName}
            onChange={(ev) => setEnvName(ev.target.value)}
          />
          <label>
            <input
              type="checkbox"
              data-testid="repo-env-approval"
              checked={envApproval}
              onChange={(ev) => setEnvApproval(ev.target.checked)}
            />{' '}
            {t('repoSettings.approvalRequired')}
          </label>
          <button
            type="button"
            data-testid="repo-env-submit"
            disabled={envBusy}
            onClick={() => void submitEnvironment()}
          >
            {t('repoSettings.envCreate')}
          </button>
        </div>
        {envError ? <p role="alert">{envError}</p> : null}
      </div>
      <div data-testid="repo-grants">
        <strong>{t('repoSettings.grantsTitle')}</strong>
        <ul data-testid="repo-grants-list">
          {grants.map((g) => (
            <li key={g.principal_id} data-testid="repo-grant-row">
              {g.handle} (
              {t(`repoSettings.grantRoles.${g.role as (typeof GRANT_ROLES)[number]}`, {
                defaultValue: g.role,
              })}
              ){' '}
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
        <select
          data-testid="repo-grant-role"
          value={grantRole}
          onChange={(e) => setGrantRole(e.target.value)}
        >
          {GRANT_ROLES.map((r) => (
            <option key={r} value={r}>
              {t(`repoSettings.grantRoles.${r}`)}
            </option>
          ))}
        </select>
        <button
          type="button"
          data-testid="repo-grant-submit"
          disabled={grantsBusy || grantHandle.trim().length === 0}
          onClick={() => void submitGrant()}
        >
          {t('repoSettings.grantSubmit')}
        </button>
        {grantError ? <p role="alert">{grantError}</p> : null}
      </div>
    </Panel>
  )
}
