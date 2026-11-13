import { useEffect, useState } from 'react'
import { grantRepoCollaborator, listRepoEnvironments, listRepoGrants, type RepoEnvironment } from '../api'
import { Panel } from '../components/Panel'
import { useWorkspace } from '../app/WorkspaceProvider'

export function RepoSettingsPage() {
  const {
    t,
    selectedRepo,
    grants,
    grantHandle,
    setGrantHandle,
    setGrants,
    activeOrg,
  } = useWorkspace()
  const [environments, setEnvironments] = useState<RepoEnvironment[]>([])

  useEffect(() => {
    if (!selectedRepo) return
    void listRepoEnvironments(activeOrg, selectedRepo)
      .then(setEnvironments)
      .catch(() => setEnvironments([]))
  }, [activeOrg, selectedRepo])

  if (!selectedRepo) {
    return <p role="status">{t('repo.selectFirst')}</p>
  }

  return (
    <Panel>
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
        <strong>{t('repo.grantsTitle')}</strong>
        <ul>
          {grants.map((g) => (
            <li key={g.handle}>
              {g.handle} ({g.role})
            </li>
          ))}
        </ul>
        <input
          placeholder={t('repo.grantHandlePlaceholder')}
          value={grantHandle}
          onChange={(e) => setGrantHandle(e.target.value)}
        />
        <button
          type="button"
          onClick={async () => {
            if (!grantHandle.trim()) return
            await grantRepoCollaborator(activeOrg, selectedRepo, grantHandle.trim(), 'reader')
            setGrants(await listRepoGrants(activeOrg, selectedRepo))
            setGrantHandle('')
          }}
        >
          {t('repo.grantReader')}
        </button>
      </div>
    </Panel>
  )
}
