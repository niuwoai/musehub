import {
  disableOrgMember,
  inviteOrgMember,
  listOrgMembers,
  patchOrgMemberRole,
} from '../api'
import { Panel } from '../components/Panel'
import { useWorkspace } from '../app/WorkspaceProvider'

export function OrgMembersPage() {
  const ws = useWorkspace()
  const {
    t,
    session,
    orgMembers,
    membersAdmin,
    memberHandle,
    setMemberHandle,
    memberPassword,
    setMemberPassword,
    iamBusy,
    DEFAULT_ORG,
    beginIamOperation,
    endIamOperation,
    sessionEpochActive,
    bumpIamPanelsGeneration,
    refreshIamPanels,
    bumpOrgMembersRevision,
    orgMembersRevisionRef,
    iamPanelsGenerationRef,
    iamPanelsGenerationActive,
    sessionHandleRef,
  } = ws

  return (
    <Panel data-testid="org-members">
      <h2>{t('nav.orgMembers')}</h2>
      {!membersAdmin ? (
        <p>{t('orgMembers.adminRequired')}</p>
      ) : (
        <>
          <ul data-testid="org-members-list" style={{ paddingLeft: '1.2rem' }}>
            {orgMembers.map((m) => (
              <li key={m.principal_id}>
                {m.handle} · {m.org_role} · {m.principal_status}
                {m.principal_status === 'active' && m.handle !== session ? (
                  <>
                    {' '}
                    <select
                      aria-label={`role-${m.handle}`}
                      value={m.org_role}
                      onChange={(e) => {
                        void (async () => {
                          ws.setError(null)
                          try {
                            await patchOrgMemberRole(DEFAULT_ORG, m.handle, e.target.value)
                            bumpIamPanelsGeneration()
                            await refreshIamPanels()
                          } catch (err) {
                            ws.setError(
                              err instanceof Error ? err.message : 'role update failed',
                            )
                          }
                        })()
                      }}
                    >
                      <option value="member">member</option>
                      <option value="admin">admin</option>
                      <option value="owner">owner</option>
                    </select>
                    <button
                      type="button"
                      onClick={() => {
                        void (async () => {
                          ws.setError(null)
                          try {
                            await disableOrgMember(DEFAULT_ORG, m.handle)
                            bumpIamPanelsGeneration()
                            await refreshIamPanels()
                          } catch (err) {
                            ws.setError(
                              err instanceof Error ? err.message : 'disable failed',
                            )
                          }
                        })()
                      }}
                    >
                      {t('orgMembers.disable')}
                    </button>
                  </>
                ) : null}
              </li>
            ))}
          </ul>
          <div>
            <input
              data-testid="member-handle"
              placeholder="handle"
              value={memberHandle}
              onChange={(e) => setMemberHandle(e.target.value)}
            />
            <input
              data-testid="member-password"
              type="password"
              placeholder="initial password"
              value={memberPassword}
              onChange={(e) => setMemberPassword(e.target.value)}
            />
            <button
              type="button"
              data-testid="invite-member"
              disabled={iamBusy}
              onClick={() => {
                void (async () => {
                  const opEpoch = beginIamOperation()
                  const invitedAs = sessionHandleRef.current ?? session
                  ws.setError(null)
                  try {
                    const invitedHandle = memberHandle.trim()
                    bumpOrgMembersRevision()
                    await inviteOrgMember(
                      DEFAULT_ORG,
                      invitedHandle,
                      memberPassword,
                      'member',
                    )
                    bumpIamPanelsGeneration()
                    const panelsGeneration = iamPanelsGenerationRef.current
                    await listOrgMembers(DEFAULT_ORG)
                    if (
                      sessionEpochActive(opEpoch) &&
                      invitedAs &&
                      (sessionHandleRef.current ?? session) === invitedAs &&
                      iamPanelsGenerationActive(panelsGeneration) &&
                      orgMembersRevisionRef.current !== undefined
                    ) {
                      bumpOrgMembersRevision()
                      await refreshIamPanels()
                    }
                    if (sessionEpochActive(opEpoch)) {
                      setMemberHandle('')
                      setMemberPassword('')
                    }
                  } catch (err) {
                    if (sessionEpochActive(opEpoch)) {
                      ws.setError(
                        err instanceof Error ? err.message : 'member invite failed',
                      )
                    }
                  } finally {
                    endIamOperation(opEpoch)
                  }
                })()
              }}
            >
              {t('orgMembers.addMember')}
            </button>
          </div>
        </>
      )}
    </Panel>
  )
}
