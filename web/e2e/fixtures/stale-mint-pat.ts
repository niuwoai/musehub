/** Playwright-only fixture token; never a real credential. */
export const STALE_MINT_FIXTURE_TOKEN =
  'mhp_TEST_FIXTURE_STALE_MINT_EXAMPLE_abcdefghijklmnopqrstuvwxyz0123456789'

export const STALE_MINT_FIXTURE_BODY = {
  id: '00000000-0000-0000-0000-0000000000e2',
  token: STALE_MINT_FIXTURE_TOKEN,
  note: 'shown once (test fixture — example only)',
}
