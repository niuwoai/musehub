export const STALE_DEPLOYMENT_ID = '11111111-1111-4111-8111-111111111111'
export const STALE_PLAN_HASH =
  'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
export const STALE_PLAN_PREFIX = STALE_PLAN_HASH.slice(0, 12)

export const STALE_DEPLOYMENT_BODY = {
  id: STALE_DEPLOYMENT_ID,
  environment_slug: 'production',
  release_id: '22222222-2222-4222-8222-222222222222',
  state: 'awaiting_approval',
  adapter_kind: 'compose-mock',
  plan_hash: STALE_PLAN_HASH,
  approval_required: true,
}

export const STALE_TRACE_BODY = {
  deployment_id: STALE_DEPLOYMENT_ID,
  release_id: '22222222-2222-4222-8222-222222222222',
  pipeline_run_id: '33333333-3333-4333-8333-333333333333',
  commit_oid: 'c'.repeat(40),
  deployment_adapter: 'compose-mock',
  adapter_is_mock: true,
  artifacts: [],
  gaps: [],
}
