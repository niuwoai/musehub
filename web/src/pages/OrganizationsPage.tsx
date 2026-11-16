import { FormEvent, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useWorkspace } from '../app/WorkspaceProvider'
import { paths } from '../app/paths'
import { createOrganization } from '../api'

export function OrganizationsPage() {
  const navigate = useNavigate()
  const { t, organizations, organizationsLoading, refreshOrganizations } = useWorkspace()
  const [slug, setSlug] = useState('')
  const [displayName, setDisplayName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function onCreate(event: FormEvent) {
    event.preventDefault()
    if (busy) return
    setBusy(true)
    setError(null)
    try {
      const organization = await createOrganization(slug, displayName)
      await refreshOrganizations()
      navigate(paths.repos(organization.slug))
    } catch (err) {
      setError(err instanceof Error ? err.message : t('organizations.createFailed'))
    } finally {
      setBusy(false)
    }
  }

  return (
    <section>
      <h2>{t('organizations.title')}</h2>
      <ul data-testid="organization-list">
        {organizations.map((organization) => (
          <li key={organization.id}>
            <button
              type="button"
              onClick={() => navigate(paths.repos(organization.slug))}
            >
              {organization.display_name} ({organization.slug})
            </button>{' '}
            <span>{t('organizations.role', { role: organization.role })}</span>
          </li>
        ))}
      </ul>
      {organizationsLoading ? <p role="status">{t('common.loading')}</p> : null}

      <h3>{t('organizations.createTitle')}</h3>
      <form onSubmit={(event) => void onCreate(event)}>
        <label htmlFor="organization-display-name">{t('organizations.displayName')}</label>
        <input
          id="organization-display-name"
          value={displayName}
          onChange={(event) => setDisplayName(event.target.value)}
          maxLength={120}
          required
        />
        <label htmlFor="organization-slug">{t('organizations.slug')}</label>
        <input
          id="organization-slug"
          value={slug}
          onChange={(event) => setSlug(event.target.value)}
          minLength={3}
          maxLength={48}
          pattern="[A-Za-z0-9]+(?:-[A-Za-z0-9]+)*"
          required
        />
        {error ? <p role="alert">{error}</p> : null}
        <button type="submit" disabled={busy}>
          {busy ? t('common.loading') : t('organizations.create')}
        </button>
      </form>
    </section>
  )
}
