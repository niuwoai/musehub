import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { getOrgSettings, patchOrgSettings } from '../api'
import { useWorkspace } from '../app/WorkspaceProvider'

const COLLAB_ROLES = ['reader', 'developer', 'maintainer'] as const

export function OrgSettingsPage() {
  const { t } = useTranslation()
  const { settings, orgDisplayName, setOrgDisplayName, setSettings, activeOrg } = useWorkspace()
  const [defaultCollabRole, setDefaultCollabRole] = useState('reader')
  const [loadedOrg, setLoadedOrg] = useState<string | null>(null)
  const [settingsError, setSettingsError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  const [saved, setSaved] = useState(false)
  const activeOrgRef = useRef(activeOrg)
  activeOrgRef.current = activeOrg

  useEffect(() => {
    let cancelled = false
    setLoadedOrg(null)
    setSettingsError(null)
    setSaved(false)
    setSaving(false)
    void getOrgSettings(activeOrg)
      .then((orgSettings) => {
        if (cancelled) return
        setSettings(orgSettings)
        setOrgDisplayName(orgSettings.display_name)
        setDefaultCollabRole(orgSettings.preferences?.default_collaborator_role || 'reader')
        setLoadedOrg(activeOrg)
      })
      .catch(() => {
        if (!cancelled) setSettingsError(t('orgSettings.loadFailed'))
      })
    return () => {
      cancelled = true
    }
  }, [activeOrg, setOrgDisplayName, setSettings, t])

  if (loadedOrg !== activeOrg || !settings) {
    if (settingsError) return <p role="alert">{settingsError}</p>
    return <p role="status">{t('common.loading')}</p>
  }

  async function saveSettings() {
    const org = activeOrg
    setSettingsError(null)
    setSaved(false)
    setSaving(true)
    try {
      const next = await patchOrgSettings(org, {
        display_name: orgDisplayName,
        preferences: { default_collaborator_role: defaultCollabRole },
      })
      if (activeOrgRef.current !== org) return
      setSettings(next)
      setOrgDisplayName(next.display_name)
      setDefaultCollabRole(next.preferences?.default_collaborator_role || defaultCollabRole)
      setSaved(true)
    } catch {
      if (activeOrgRef.current === org) setSettingsError(t('orgSettings.saveFailed'))
    } finally {
      if (activeOrgRef.current === org) setSaving(false)
    }
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
            onChange={(e) => {
              setOrgDisplayName(e.target.value)
              setSaved(false)
            }}
          />
        </label>
        <label>
          {t('orgSettings.defaultCollaboratorRole')}
          <select
            data-testid="org-default-collab-role"
            value={defaultCollabRole}
            onChange={(e) => {
              setDefaultCollabRole(e.target.value)
              setSaved(false)
            }}
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
          disabled={saving}
          onClick={() => void saveSettings()}
        >
          {saving ? t('orgSettings.saving') : t('orgSettings.save')}
        </button>
        {settingsError ? <p role="alert">{settingsError}</p> : null}
        {saved ? (
          <p role="status" data-testid="org-settings-saved">
            {t('orgSettings.saved')}
          </p>
        ) : null}
        <p data-testid="org-settings-executors">
          {t('orgSettings.runtime', {
            pipeline: settings.pipeline_executor,
            agent: settings.agent_executor,
            compose: settings.compose_adapter_mode,
            profile: settings.product_profile,
          })}
        </p>
      </div>
    </section>
  )
}
