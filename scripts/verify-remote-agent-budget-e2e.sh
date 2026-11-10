#!/usr/bin/env bash
# Remote agent: budget limit on worker; __budget_exceed__ message; no commit.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
# shellcheck source=lib/remote-runner-e2e-common.sh
source "$ROOT/scripts/lib/remote-runner-e2e-common.sh"
export MUSEHUB_AGENT_EXECUTOR=remote
export MUSEHUB_RUNNER_WORK_KINDS=agent

remote_runner_e2e_init
trap remote_runner_e2e_cleanup EXIT
remote_runner_start_server
COOKIE=$(remote_runner_login_cookie)
export COOKIE
RUNNER_MEMBER="agent-budget-$(e2e_uuid | cut -c1-8)"
remote_runner_bootstrap_repo "agent-budget-$(e2e_uuid | cut -c1-8)" "$RUNNER_MEMBER" "Agent-Budget-Member-1!"

PAT_JSON=$(curl -sf -X POST "http://127.0.0.1:${API_PORT}/api/v1/tokens" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d "{\"name\":\"agent\",\"org\":\"demo\",\"scopes\":[\"agent:execute\",\"repo:read\"],\"scope_mode\":\"selected\",\"repo_ids\":[\"$REPO_ID\"],\"runner_handle\":\"$RUNNER_MEMBER\"}")
RUNNER_PAT=$(echo "$PAT_JSON" | jq -r .token)

ISSUE_JSON=$(curl -sf -X POST "http://127.0.0.1:${API_PORT}/api/v1/orgs/demo/repos/$REPO_SLUG/issues" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d '{"title":"budget"}')
ISSUE_ID=$(echo "$ISSUE_JSON" | jq -r .id)
ISSUE_NUM=$(echo "$ISSUE_JSON" | jq -r .number)
curl -sf -X POST "http://127.0.0.1:${API_PORT}/api/v1/orgs/demo/repos/$REPO_SLUG/issues/$ISSUE_NUM/plans" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d '{"body":"p"}' >/dev/null
curl -sf -X PATCH "http://127.0.0.1:${API_PORT}/api/v1/orgs/demo/repos/$REPO_SLUG/issues/$ISSUE_NUM/plans" \
  -H "Cookie: musehub_session=$COOKIE" >/dev/null

AGENT_JSON=$(curl -sf -X POST "http://127.0.0.1:${API_PORT}/api/v1/orgs/demo/repos/$REPO_SLUG/agent-runs" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d "{\"issue_id\":\"$ISSUE_ID\",\"message\":\"__budget_exceed__\",\"budget_tokens\":1}")
RUN_ID=$(echo "$AGENT_JSON" | jq -r .id)

REF_BIN="${CARGO_TARGET_DIR:-$ROOT/target}/debug/musehub-agent-reference"
export RUNNER_PAT
remote_runner_start_runnerd "$REF_BIN"

for _ in $(seq 1 120); do
  st=$(psql "$DATABASE_URL" -Atc "SELECT state FROM agent_runs WHERE id='$RUN_ID'")
  [[ "$st" == "failed" || "$st" == "succeeded" ]] && break
  sleep 0.5
done
[[ "$st" == "failed" ]]
commit=$(psql "$DATABASE_URL" -Atc "SELECT coalesce(result_commit_oid,'') FROM agent_runs WHERE id='$RUN_ID'")
[[ -z "$commit" ]]
echo "remote_agent_budget_subprocess_e2e=ok"
