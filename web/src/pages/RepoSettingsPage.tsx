import { grantRepoCollaborator, listRepoGrants } from '../api'
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
    DEFAULT_ORG,
  } = useWorkspace()

  if (!selectedRepo) {
    return <p role="status">{t('repo.selectFirst')}</p>
  }

  return (
    <Panel>
      <h2>{t('nav.repoSettings')}</h2>
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
            await grantRepoCollaborator(DEFAULT_ORG, selectedRepo, grantHandle.trim(), 'reader')
            setGrants(await listRepoGrants(DEFAULT_ORG, selectedRepo))
            setGrantHandle('')
          }}
        >
          {t('repo.grantReader')}
        </button>
      </div>
    </Panel>
  )
}
