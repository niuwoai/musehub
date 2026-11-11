import { useCallback, useEffect, useState } from 'react'
import { useParams } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { Panel } from '../components/Panel'
import { DEFAULT_ORG } from '../app/paths'
import { listGitCommits, listGitTree, readGitBlob } from '../api'

export function RepoCodePage() {
  const { t } = useTranslation()
  const { org = DEFAULT_ORG, slug } = useParams<{ org: string; slug: string }>()
  const [gitRef, setGitRef] = useState('main')
  const [commit, setCommit] = useState('')
  const [path, setPath] = useState('')
  const [entries, setEntries] = useState<{ path: string; kind: string }[]>([])
  const [fileBody, setFileBody] = useState('')
  const [fileMeta, setFileMeta] = useState('')
  const [error, setError] = useState<string | null>(null)

  const loadTree = useCallback(async () => {
    if (!slug || !commit) return
    setError(null)
    const tree = await listGitTree(org, slug, commit, path)
    setEntries(tree.items)
    setFileBody('')
    setFileMeta('')
  }, [org, slug, commit, path])

  useEffect(() => {
    if (!slug) return
    void (async () => {
      try {
        const log = await listGitCommits(org, slug, gitRef)
        const head = log.items[0]?.oid
        if (head) {
          setCommit(head)
        }
      } catch (e) {
        setError(e instanceof Error ? e.message : t('code.loadFailed'))
      }
    })()
  }, [org, slug, gitRef, t])

  useEffect(() => {
    if (commit) void loadTree().catch((e) => setError(String(e)))
  }, [commit, loadTree])

  async function openEntry(entryPath: string, kind: string) {
    if (kind === 'tree') {
      setPath(entryPath)
      return
    }
    const blob = await readGitBlob(org, slug!, commit, entryPath)
    if (blob.binary) {
      setFileMeta(t('code.binaryFile', { path: entryPath }))
      setFileBody('')
    } else {
      setFileMeta(entryPath)
      setFileBody(blob.text ?? '')
    }
  }

  if (!slug) return <p role="status">{t('repo.selectFirst')}</p>

  return (
    <Panel data-testid="repo-code">
      <h2>{t('nav.repoCode')}</h2>
      <label>
        {t('code.refLabel')}{' '}
        <input
          data-testid="code-git-ref"
          value={gitRef}
          onChange={(e) => setGitRef(e.target.value)}
        />
      </label>
      <p data-testid="code-pinned-commit">{t('code.pinnedCommit', { oid: commit || '…' })}</p>
      {commit ? (
        <span data-testid="code-commit-oid" className="sr-only" aria-hidden>
          {commit}
        </span>
      ) : null}
      {error ? <p role="alert">{error}</p> : null}
      <ul data-testid="code-tree">
        {entries.length === 0 ? (
          <li role="status">{t('code.emptyTree')}</li>
        ) : (
          entries.map((e) => (
            <li key={e.path}>
              <button
                type="button"
                data-testid={`code-entry-${e.kind}`}
                onClick={() => void openEntry(e.path, e.kind)}
              >
                {e.kind === 'tree' ? '📁' : '📄'}{' '}
                {e.path.replace(`${path}${path ? '/' : ''}`, '') || e.path}
              </button>
            </li>
          ))
        )}
      </ul>
      {fileMeta ? (
        <pre data-testid="code-file-body" style={{ maxHeight: 360, overflow: 'auto' }}>
          {fileMeta}
          {'\n'}
          {fileBody}
        </pre>
      ) : null}
    </Panel>
  )
}
