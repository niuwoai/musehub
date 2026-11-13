import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useParams } from 'react-router-dom'
import { listAuditEvents, type AuditEvent } from '../api'
import { Panel } from '../components/Panel'
import { DEFAULT_ORG } from '../app/paths'

export function AuditPage() {
  const { t } = useTranslation()
  const { org = DEFAULT_ORG } = useParams<{ org: string }>()
  const [resourceType, setResourceType] = useState('')
  const [events, setEvents] = useState<AuditEvent[]>([])
  const [error, setError] = useState<string | null>(null)

  const reload = useCallback(async () => {
    setError(null)
    try {
      const items = await listAuditEvents(
        org,
        80,
        resourceType.trim() || undefined,
      )
      setEvents(items)
    } catch (e) {
      setError(e instanceof Error ? e.message : t('audit.loadFailed'))
      setEvents([])
    }
  }, [org, resourceType, t])

  useEffect(() => {
    void reload()
  }, [reload])

  return (
    <Panel data-testid="audit-panel">
      <h2>{t('nav.audit')}</h2>
      <label>
        {t('audit.resourceTypeFilter')}
        <select
          data-testid="audit-resource-type"
          value={resourceType}
          onChange={(e) => setResourceType(e.target.value)}
        >
          <option value="">{t('issues.filterAny')}</option>
          <option value="repository">{t('audit.resourceRepository')}</option>
          <option value="pull_request">{t('audit.resourcePullRequest')}</option>
          <option value="issue">{t('audit.resourceIssue')}</option>
          <option value="pipeline_run">{t('audit.resourcePipeline')}</option>
          <option value="deployment">{t('audit.resourceDeployment')}</option>
          <option value="principal">{t('audit.resourcePrincipal')}</option>
          <option value="repository_grant">{t('audit.resourceRepositoryGrant')}</option>
        </select>
      </label>
      <button type="button" data-testid="audit-apply-filter" onClick={() => void reload()}>
        {t('issues.applyFilters')}
      </button>
      {error ? <p role="alert">{error}</p> : null}
      {events.length === 0 ? (
        <p>{t('audit.empty')}</p>
      ) : (
        <ul style={{ maxHeight: '24rem', overflow: 'auto', margin: 0, paddingLeft: '1.2rem' }}>
          {events.map((ev) => (
            <li key={ev.id} data-testid="audit-event">
              <code>{ev.occurred_at.slice(0, 19)}</code> {ev.action}{' '}
              <span data-testid="audit-event-resource">
                {ev.resource_type}/{ev.resource_id.slice(0, 8)}…
              </span>{' '}
              · {ev.decision}
            </li>
          ))}
        </ul>
      )}
    </Panel>
  )
}
