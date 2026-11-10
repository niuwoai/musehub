/** Playwright-only fixture agent run; never a real execution. */
export const STALE_AGENT_FIXTURE_ID = '00000000-0000-0000-0000-0000000000a1'

export const STALE_AGENT_FIXTURE_COMMIT =
  'cafebabecafebabecafebabecafebabecafebabe'

export const STALE_AGENT_FIXTURE_BODY = {
  id: STALE_AGENT_FIXTURE_ID,
  adapter_kind: 'reference',
  state: 'succeeded',
  result_commit_oid: STALE_AGENT_FIXTURE_COMMIT,
  usage_tokens: 0,
  budget_unknown: true,
  error_code: null,
}
