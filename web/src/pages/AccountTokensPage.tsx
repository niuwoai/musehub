import { createScopedPat, createUserPat, revokePat } from '../api'
import { Panel } from '../components/Panel'
import { useWorkspace } from '../app/WorkspaceProvider'

export function AccountTokensPage() {
  const ws = useWorkspace()
  const {
    t,
    repos,
    myTokens,
    mintedPatFull,
    patName,
    setPatName,
    patScopeMode,
    setPatScopeMode,
    patRepoId,
    setPatRepoId,
    iamBusy,
    DEFAULT_ORG,
    beginIamOperation,
    endIamOperation,
    sessionEpochActive,
    bumpIamPanelsGeneration,
    refreshIamPanels,
    setMintedPatFull,
    sessionEpochRef,
  } = ws

  return (
    <Panel data-testid="pat-management">
      <h2>{t('nav.accountTokens')}</h2>
      {mintedPatFull ? (
        <p data-testid="pat-minted-once">
          {t('pat.mintedOnce')} <code>{mintedPatFull}</code>
        </p>
      ) : (
        <p>{t('pat.mintedHint')}</p>
      )}
      <ul data-testid="pat-masked-list">
        {myTokens.map((tok) => (
          <li key={tok.id}>
            {tok.name}: <code>{tok.token_prefix}…</code> ({tok.scope_mode}, exp{' '}
            {tok.expires_at.slice(0, 10)})
            <button
              type="button"
              data-testid={`revoke-pat-${tok.id}`}
              onClick={() => {
                void (async () => {
                  const epoch = sessionEpochRef.current
                  ws.setError(null)
                  try {
                    await revokePat(tok.id)
                    if (!sessionEpochActive(epoch)) {
                      return
                    }
                    setMintedPatFull(null)
                    bumpIamPanelsGeneration()
                    await refreshIamPanels()
                  } catch (err) {
                    ws.setError(err instanceof Error ? err.message : 'PAT revoke failed')
                  }
                })()
              }}
            >
              {t('pat.revoke')}
            </button>
          </li>
        ))}
      </ul>
      <input
        data-testid="pat-name"
        placeholder="token name"
        value={patName}
        onChange={(e) => setPatName(e.target.value)}
      />
      <label>
        {t('pat.scopeMode')}{' '}
        <select
          data-testid="pat-scope-mode"
          value={patScopeMode}
          onChange={(e) =>
            setPatScopeMode(e.target.value as 'all_accessible' | 'selected')
          }
        >
          <option value="all_accessible">all_accessible</option>
          <option value="selected">selected (one repo)</option>
        </select>
      </label>
      {patScopeMode === 'selected' ? (
        <label>
          Repo{' '}
          <select
            data-testid="pat-repo-id"
            value={patRepoId}
            onChange={(e) => setPatRepoId(e.target.value)}
          >
            <option value="">— select —</option>
            {repos.map((r) => (
              <option key={r.id} value={r.id}>
                {r.slug}
              </option>
            ))}
          </select>
        </label>
      ) : null}
      <button
        type="button"
        data-testid="create-pat"
        disabled={iamBusy}
        onClick={() => {
          void (async () => {
            const opEpoch = beginIamOperation()
            ws.setError(null)
            const name = patName.trim() || 'console-pat'
            try {
              let created
              if (patScopeMode === 'selected') {
                if (!patRepoId) {
                  if (sessionEpochActive(opEpoch)) {
                    ws.setError('select a repo for selected-scope PAT')
                  }
                  return
                }
                created = await createScopedPat(DEFAULT_ORG, name, ['repo:read'], {
                  scope_mode: 'selected',
                  repo_ids: [patRepoId],
                })
              } else {
                created = await createUserPat(DEFAULT_ORG, name)
              }
              if (!sessionEpochActive(opEpoch)) {
                return
              }
              setMintedPatFull(created.token)
              setPatName('')
              bumpIamPanelsGeneration()
              await refreshIamPanels()
            } catch (err) {
              if (sessionEpochActive(opEpoch)) {
                ws.setError(err instanceof Error ? err.message : 'PAT create failed')
              }
            } finally {
              endIamOperation(opEpoch)
            }
          })()
        }}
      >
        {t('pat.create')}
      </button>
    </Panel>
  )
}
