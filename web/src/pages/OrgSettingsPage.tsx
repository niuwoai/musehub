import { useEffect } from 'react'
import { getOrgSettings, patchOrgSettings } from '../api'
import { useWorkspace } from '../app/WorkspaceProvider'

export function OrgSettingsPage() {
  const { t, settings, orgDisplayName, setOrgDisplayName, setSettings, activeOrg } = useWorkspace()

  useEffect(() => {
    void getOrgSettings(activeOrg).then((s) => {
      setSettings(s)
      setOrgDisplayName(s.display_name)
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
        <button
          type="button"
          data-testid="save-org-settings"
          onClick={async () => {
            const next = await patchOrgSettings(activeOrg, {
              display_name: orgDisplayName,
              preferences: {
                default_collaborator_role:
                  settings.preferences.default_collaborator_role || 'reader',
              },
            })
            setSettings(next)
          }}
        >
          {t('orgSettings.save')}
        </button>
        <p>
          pipeline: {settings.pipeline_executor} · agent: {settings.agent_executor} · compose:{' '}
          {settings.compose_adapter_mode} ({settings.product_profile})
        </p>
      </div>
    </section>
  )
}
