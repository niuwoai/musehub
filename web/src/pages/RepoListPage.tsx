import { useEffect } from 'react'
import { useWorkspace } from '../app/WorkspaceProvider'

export function RepoListPage() {
  const { t, repos, refreshRepos, onCreateRepo, selectRepo } = useWorkspace()

  useEffect(() => {
    void refreshRepos()
    // load list when entering repos route
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  return (
    <section>
      <button type="button" onClick={() => void onCreateRepo()}>
        {t('app.createRepo')}
      </button>
      <ul data-testid="repo-list">
        {repos.map((repo) => (
          <li key={repo.id}>
            <button type="button" onClick={() => selectRepo(repo.slug)}>
              {repo.slug}
            </button>
            <code style={{ marginLeft: '0.5rem' }}>{repo.clone_url}</code>
          </li>
        ))}
      </ul>
    </section>
  )
}
