import { useCallback, useEffect, useRef, useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { Panel } from '../components/Panel'
import { DEFAULT_ORG, paths } from '../app/paths'
import {
  listGitCommitDiff,
  listGitCommits,
  listGitTree,
  listRepos,
  readGitBlob,
  readGitCommitDiff,
} from '../api'

const RECENT_COMMIT_LIMIT = 20

type GitCommit = { oid: string; parents: string[]; summary: string; author: string }
type GitDiffFile = { old_path: string; new_path: string; change_type: string; truncated: boolean }
type GitDiffDocument = { path: string; binary: boolean; unified?: string | null; truncated: boolean }

export function RepoCodePage() {
  const { t } = useTranslation()
  const { org = DEFAULT_ORG, slug } = useParams<{ org: string; slug: string }>()
  const [gitRef, setGitRef] = useState('main')
  const [commit, setCommit] = useState('')
  const [history, setHistory] = useState<GitCommit[]>([])
  const [historyHasMore, setHistoryHasMore] = useState(false)
  const [historyLoadingMore, setHistoryLoadingMore] = useState(false)
  const historyRequestId = useRef(0)
  const historyMoreInFlightFor = useRef<number | null>(null)
  const [path, setPath] = useState('')
  const [entries, setEntries] = useState<{ path: string; kind: string }[]>([])
  const [fileBody, setFileBody] = useState('')
  const [fileMeta, setFileMeta] = useState('')
  const [diffFiles, setDiffFiles] = useState<GitDiffFile[]>([])
  const [diffHasMore, setDiffHasMore] = useState(false)
  const [diffDocument, setDiffDocument] = useState<GitDiffDocument | null>(null)
  const [diffLoading, setDiffLoading] = useState(false)
  const [diffError, setDiffError] = useState<string | null>(null)
  const diffRequestId = useRef(0)
  const [error, setError] = useState<string | null>(null)
  const [sshCloneUrl, setSshCloneUrl] = useState<string | null>(null)
  const [httpCloneUrl, setHttpCloneUrl] = useState<string | null>(null)

  const loadTree = useCallback(async () => {
    if (!slug || !commit) return
    setError(null)
    const tree = await listGitTree(org, slug, commit, path)
    setEntries(tree.items)
    setFileBody('')
    setFileMeta('')
  }, [org, slug, commit, path])

  const selectedCommit = history.find((item) => item.oid === commit)
  const diffBase = selectedCommit?.parents[0] ?? ''

  useEffect(() => {
    if (!slug) return
    void listRepos(org)
      .then((repos) => {
        const match = repos.find((r) => r.slug === slug)
        setSshCloneUrl(match?.ssh_clone_url ?? null)
        setHttpCloneUrl(match?.clone_url ?? null)
      })
      .catch(() => {
        setSshCloneUrl(null)
        setHttpCloneUrl(null)
      })
  }, [org, slug])

  useEffect(() => {
    if (!slug) return
    let active = true
    historyRequestId.current += 1
    setError(null)
    setHistory([])
    setHistoryHasMore(false)
    setHistoryLoadingMore(false)
    historyMoreInFlightFor.current = null
    setCommit('')
    setPath('')
    setEntries([])
    setFileBody('')
    setFileMeta('')
    void (async () => {
      try {
        const log = await listGitCommits(org, slug, gitRef, RECENT_COMMIT_LIMIT)
        if (!active) return
        setHistory(log.items)
        setHistoryHasMore(log.has_more)
        setCommit(log.items[0]?.oid ?? '')
        setPath('')
      } catch (e) {
        if (!active) return
        setHistory([])
        setCommit('')
        setError(e instanceof Error ? e.message : t('code.loadFailed'))
      }
    })()
    return () => {
      active = false
    }
  }, [org, slug, gitRef, t])

  async function loadMoreCommits() {
    if (!slug || !historyHasMore) return
    const requestId = historyRequestId.current
    if (historyMoreInFlightFor.current === requestId) return
    historyMoreInFlightFor.current = requestId
    setHistoryLoadingMore(true)
    try {
      const page = await listGitCommits(org, slug, gitRef, RECENT_COMMIT_LIMIT, history.length)
      if (requestId !== historyRequestId.current) return
      setHistory((current) => [...current, ...page.items])
      setHistoryHasMore(page.has_more)
    } catch (e) {
      if (requestId === historyRequestId.current) {
        setError(e instanceof Error ? e.message : t('code.loadFailed'))
      }
    } finally {
      if (historyMoreInFlightFor.current === requestId) {
        historyMoreInFlightFor.current = null
      }
      if (requestId === historyRequestId.current) setHistoryLoadingMore(false)
    }
  }

  useEffect(() => {
    if (commit) void loadTree().catch((e) => setError(String(e)))
  }, [commit, loadTree])

  useEffect(() => {
    let active = true
    diffRequestId.current += 1
    setDiffFiles([])
    setDiffHasMore(false)
    setDiffDocument(null)
    setDiffError(null)
    if (!slug || !commit || !diffBase) {
      setDiffLoading(false)
      return () => {
        active = false
      }
    }
    setDiffLoading(true)
    void listGitCommitDiff(org, slug, diffBase, commit)
      .then((result) => {
        if (!active) return
        setDiffFiles(result.files)
        setDiffHasMore(result.has_more)
      })
      .catch((e) => {
        if (active) setDiffError(e instanceof Error ? e.message : t('code.diffLoadFailed'))
      })
      .finally(() => {
        if (active) setDiffLoading(false)
      })
    return () => {
      active = false
      diffRequestId.current += 1
    }
  }, [org, slug, commit, diffBase, t])

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
      const meta = blob.truncated
        ? t('code.truncatedFile', { path: entryPath })
        : entryPath
      setFileMeta(meta)
      setFileBody(blob.text ?? '')
    }
  }

  async function openDiffFile(filePath: string) {
    const requestId = ++diffRequestId.current
    setDiffError(null)
    setDiffDocument(null)
    try {
      const diff = await readGitCommitDiff(org, slug!, diffBase, commit, filePath)
      if (requestId === diffRequestId.current) setDiffDocument(diff)
    } catch (e) {
      if (requestId === diffRequestId.current) {
        setDiffError(e instanceof Error ? e.message : t('code.diffLoadFailed'))
      }
    }
  }

  if (!slug) return <p role="status">{t('repo.selectFirst')}</p>

  const pathParts = path ? path.split('/') : []

  return (
    <Panel data-testid="repo-code">
      <h2>{t('nav.repoCode')}</h2>
      {httpCloneUrl ? (
        <p data-testid="code-http-clone-url">
          <strong>{t('code.httpCloneLabel')}</strong>{' '}
          <code>{httpCloneUrl}</code>
        </p>
      ) : null}
      {sshCloneUrl ? (
        <p data-testid="code-ssh-clone-url">
          <strong>{t('code.sshCloneLabel')}</strong>{' '}
          <code>{sshCloneUrl}</code>
        </p>
      ) : null}
      <p>
        <Link to={paths.accountSecurity()} data-testid="code-ssh-keys-link">
          {t('code.sshKeysLink')}
        </Link>
      </p>
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
      <section aria-labelledby="code-history-heading">
        <h3 id="code-history-heading">{t('code.historyLabel')}</h3>
        {history.length === 0 ? (
          <p role="status">{t('code.emptyHistory')}</p>
        ) : (
          <ol data-testid="code-history">
            {history.map((item) => (
              <li key={item.oid}>
                <button
                  type="button"
                  data-testid="code-history-commit"
                  aria-pressed={item.oid === commit}
                  onClick={() => {
                    setPath('')
                    setFileBody('')
                    setFileMeta('')
                    setCommit(item.oid)
                  }}
                >
                  <code>{item.oid.slice(0, 8)}</code>{' '}
                  {item.summary || t('code.commitWithoutSummary')}
                </button>
                <span> — {item.author}</span>
              </li>
            ))}
          </ol>
        )}
        {historyHasMore || historyLoadingMore ? (
          <button
            type="button"
            data-testid="code-history-load-more"
            disabled={historyLoadingMore}
            onClick={() => void loadMoreCommits()}
          >
            {historyLoadingMore ? t('code.loadingMoreHistory') : t('code.loadMoreHistory')}
          </button>
        ) : null}
      </section>
      {selectedCommit ? (
        <section aria-labelledby="code-diff-heading" data-testid="code-commit-diff">
          <h3 id="code-diff-heading">{t('code.diffHeading', { oid: commit.slice(0, 8) })}</h3>
          {!diffBase ? (
            <p role="status">{t('code.rootCommitNoDiff')}</p>
          ) : (
            <>
              <p>{t('code.diffBase', { oid: diffBase.slice(0, 8) })}</p>
              {diffLoading ? <p role="status">{t('code.diffLoading')}</p> : null}
              {diffError ? <p role="alert">{diffError}</p> : null}
              {!diffLoading && diffFiles.length === 0 && !diffError ? (
                <p role="status">{t('code.noDiffFiles')}</p>
              ) : null}
              <ul data-testid="code-commit-diff-files">
                {diffFiles.map((file) => {
                  const pathLabel = file.old_path === file.new_path
                    ? file.new_path
                    : `${file.old_path} → ${file.new_path}`
                  return (
                    <li key={`${file.old_path}:${file.new_path}`}>
                      <button type="button" onClick={() => void openDiffFile(file.new_path || file.old_path)}>
                        {pathLabel}
                      </button>
                    </li>
                  )
                })}
              </ul>
              {diffHasMore ? <p role="status">{t('code.diffFilesTruncated')}</p> : null}
              {diffDocument ? (
                <div data-testid="code-commit-diff-document">
                  <p>{diffDocument.path}</p>
                  {diffDocument.binary ? <p role="status">{t('code.binaryDiff')}</p> : null}
                  {diffDocument.truncated ? <p role="status">{t('code.diffTruncated')}</p> : null}
                  {!diffDocument.binary ? (
                    <pre style={{ maxHeight: 480, overflow: 'auto' }}>
                      {diffDocument.unified ?? ''}
                    </pre>
                  ) : null}
                </div>
              ) : null}
            </>
          )}
        </section>
      ) : null}
      <nav aria-label={t('code.breadcrumbLabel')} data-testid="code-breadcrumbs">
        <ol>
          <li>
            <button type="button" aria-current={path ? undefined : 'page'} onClick={() => setPath('')}>
              {t('code.repositoryRoot')}
            </button>
          </li>
          {pathParts.map((part, index) => {
            const itemPath = pathParts.slice(0, index + 1).join('/')
            const current = index === pathParts.length - 1
            return (
              <li key={itemPath}>
                <span aria-hidden="true"> / </span>
                <button
                  type="button"
                  aria-current={current ? 'page' : undefined}
                  onClick={() => setPath(itemPath)}
                >
                  {part}
                </button>
              </li>
            )
          })}
        </ol>
      </nav>
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
