import { Panel } from '../components/Panel'
import { useWorkspace } from '../app/WorkspaceProvider'

export function AuditPage() {
  const { t, auditEvents } = useWorkspace()

  return (
    <Panel data-testid="audit-panel">
      <h2>{t('nav.audit')}</h2>
      {auditEvents.length === 0 ? (
        <p>{t('audit.empty')}</p>
      ) : (
        <ul style={{ maxHeight: '24rem', overflow: 'auto', margin: 0, paddingLeft: '1.2rem' }}>
          {auditEvents.map((ev) => (
            <li key={ev.id}>
              <code>{ev.occurred_at.slice(0, 19)}</code> {ev.action}{' '}
              <span>
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
