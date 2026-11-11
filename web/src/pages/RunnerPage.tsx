import { Panel } from '../components/Panel'
import { useWorkspace } from '../app/WorkspaceProvider'

export function RunnerPage() {
  const { t, runnerCaps, runnerTokens } = useWorkspace()

  return (
    <Panel data-testid="runner-status-panel">
      <h2>{t('nav.runner')}</h2>
      {runnerCaps ? (
        <>
          <p data-testid="runner-executors">
            pipeline {runnerCaps.pipeline_executor} · agent {runnerCaps.agent_executor} · deploy{' '}
            {runnerCaps.deploy_executor}
          </p>
          <p>
            lease {runnerCaps.lease_seconds}s · scopes: {runnerCaps.scopes_accepted.join(', ')}
          </p>
          <p style={{ color: '#57606a', fontSize: '0.9rem' }}>{runnerCaps.note}</p>
        </>
      ) : (
        <p>{t('runner.loadFailed')}</p>
      )}
      {runnerTokens.length > 0 ? (
        <ul>
          {runnerTokens.map((tok) => (
            <li key={tok.id}>
              {tok.name}: <code>{tok.token_prefix}…</code> ({tok.scope_mode})
            </li>
          ))}
        </ul>
      ) : (
        <p>{t('runner.noTokens')}</p>
      )}
    </Panel>
  )
}
