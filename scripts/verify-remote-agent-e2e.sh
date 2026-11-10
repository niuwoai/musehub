#!/usr/bin/env bash
# Subprocess acceptance: API (remote agent executor) + musehub-runnerd agent claim.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export DATABASE_URL="${MUSEHUB_TEST_DATABASE_URL:-${DATABASE_URL:-}}"
test -n "$DATABASE_URL" || { echo "MUSEHUB_TEST_DATABASE_URL or DATABASE_URL required" >&2; exit 1; }

# shellcheck source=lib/remote-runner-e2e-common.sh
source "$ROOT/scripts/lib/remote-runner-e2e-common.sh"
export MUSEHUB_AGENT_EXECUTOR=remote
unset MUSEHUB_PIPELINE_EXECUTOR
export MUSEHUB_RUNNER_WORK_KINDS=agent

remote_runner_e2e_init
trap 'remote_runner_e2e_cleanup' EXIT

remote_runner_start_server
COOKIE=$(remote_runner_login_cookie)
export COOKIE
RUNNER_MEMBER="agent-runner-$(e2e_uuid | cut -c1-8)"
remote_runner_bootstrap_repo "agent-pipe-$(e2e_uuid | cut -c1-8)" "$RUNNER_MEMBER" "Agent-Runner-Member-1!"

PAT_JSON=$(curl -sf -X POST "http://127.0.0.1:${API_PORT}/api/v1/tokens" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d "{\"name\":\"agent\",\"org\":\"demo\",\"scopes\":[\"agent:execute\",\"repo:read\"],\"scope_mode\":\"selected\",\"repo_ids\":[\"$REPO_ID\"],\"runner_handle\":\"$RUNNER_MEMBER\"}")
RUNNER_PAT=$(echo "$PAT_JSON" | jq -r .token)

ISSUE_JSON=$(curl -sf -X POST "http://127.0.0.1:${API_PORT}/api/v1/orgs/demo/repos/$REPO_SLUG/issues" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d '{"title":"agent e2e"}')
ISSUE_ID=$(echo "$ISSUE_JSON" | jq -r .id)
ISSUE_NUM=$(echo "$ISSUE_JSON" | jq -r .number)

curl -sf -X POST "http://127.0.0.1:${API_PORT}/api/v1/orgs/demo/repos/$REPO_SLUG/issues/$ISSUE_NUM/plans" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d '{"body":"plan body"}' >/dev/null
curl -sf -X PATCH "http://127.0.0.1:${API_PORT}/api/v1/orgs/demo/repos/$REPO_SLUG/issues/$ISSUE_NUM/plans" \
  -H "Cookie: musehub_session=$COOKIE" >/dev/null

AGENT_JSON=$(curl -sf -X POST "http://127.0.0.1:${API_PORT}/api/v1/orgs/demo/repos/$REPO_SLUG/agent-runs" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d "{\"issue_id\":\"$ISSUE_ID\",\"message\":\"implement via remote runner\"}")
RUN_ID=$(echo "$AGENT_JSON" | jq -r .id)

REF_BIN="${CARGO_TARGET_DIR:-$ROOT/target}/debug/musehub-agent-reference"
remote_runner_start_runnerd "$REF_BIN"

for _ in $(seq 1 180); do
  st=$(psql "$DATABASE_URL" -Atc "SELECT state FROM agent_runs WHERE id='$RUN_ID'")
  if [[ "$st" == "succeeded" ]]; then
    curl -sf "http://127.0.0.1:${API_PORT}/api/v1/runner/capabilities" | jq -e '.agent_executor == "remote"' >/dev/null
    echo "remote_agent_subprocess_e2e=ok"
    exit 0
  fi
  [[ "$st" == "failed" || "$st" == "canceled" ]] && break
  sleep 1
done
echo "agent run ended: $st" >&2
exit 1
