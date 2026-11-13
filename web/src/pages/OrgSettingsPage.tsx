import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { getOrgSettings, patchOrgSettings } from '../api'
import { useWorkspace } from '../app/WorkspaceProvider'

const COLLAB_ROLES = ['reader', 'developer', 'maintainer'] as const

export function OrgSettingsPage() {
  const { t } = useTranslation()
  const { settings, orgDisplayName, setOrgDisplayName, setSettings, activeOrg } = useWorkspace()
  const [defaultCollabRole, setDefaultCollabRole] = useState('reader')
  const [settingsError, setSettingsError] = useState<string | null>(null)

  useEffect(() => {
    void getOrgSettings(activeOrg).then((s) => {
      setSettings(s)
      setOrgDisplayName(s.display_name)
      setDefaultCollabRole(s.preferences?.default_collaborator_role || 'reader')
    })
  }, [activeOrg, setOrgDisplayName, setSettings])

  if (!settings) {
    return <p role="status">{t('common.loading')}</p>
  }

  return (
    <section>
      <h2>{t('nav.orgSettings')}</h2>
      <div data-testid="org-settings">
        <label>
          {t('orgSettings.displayName')}
          <input
            data-testid="org-display-name"
            value={orgDisplayName}
            onChange={(e) => setOrgDisplayName(e.target.value)}
          />
        </label>
        <label>
          {t('orgSettings.defaultCollaboratorRole')}
          <select
            data-testid="org-default-collab-role"
            value={defaultCollabRole}
            onChange={(e) => setDefaultCollabRole(e.target.value)}
          >
            {COLLAB_ROLES.map((role) => (
              <option key={role} value={role}>
                {t(`orgSettings.repoRole.${role}`)}
              </option>
            ))}
          </select>
        </label>
        <button
          type="button"
          data-testid="save-org-settings"
          onClick={() => {
            setSettingsError(null)
            void patchOrgSettings(activeOrg, {
              display_name: orgDisplayName,
              preferences: { default_collaborator_role: defaultCollabRole },
            })
              .then((next) => {
                setSettings(next)
                setDefaultCollabRole(
                  next.preferences?.default_collaborator_role || defaultCollabRole,
                )
              })
              .catch((e) =>
                setSettingsError(
                  e instanceof Error ? e.message : t('orgSettings.saveFailed'),
                ),
              )
          }}
        >
          {t('orgSettings.save')}
        </button>
        {settingsError ? <p role="alert">{settingsError}</p> : null}
        <p data-testid="org-settings-executors">
          pipeline: {settings.pipeline_executor} · agent: {settings.agent_executor} · compose:{' '}
          {settings.compose_adapter_mode} ({settings.product_profile})
        </p>
      </div>
    </section>
  )
}
