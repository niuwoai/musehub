#!/usr/bin/env bash
# Remote agent: worker runs __slow__ reference; HTTP cancel kills work; no commit.
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
RUNNER_MEMBER="agent-cancel-$(e2e_uuid | cut -c1-8)"
remote_runner_bootstrap_repo "agent-cancel-$(e2e_uuid | cut -c1-8)" "$RUNNER_MEMBER" "Agent-Cancel-Member-1!"

PAT_JSON=$(curl -sf -X POST "http://127.0.0.1:${API_PORT}/api/v1/tokens" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d "{\"name\":\"agent\",\"org\":\"demo\",\"scopes\":[\"agent:execute\",\"repo:read\"],\"scope_mode\":\"selected\",\"repo_ids\":[\"$REPO_ID\"],\"runner_handle\":\"$RUNNER_MEMBER\"}")
RUNNER_PAT=$(echo "$PAT_JSON" | jq -r .token)

ISSUE_JSON=$(curl -sf -X POST "http://127.0.0.1:${API_PORT}/api/v1/orgs/demo/repos/$REPO_SLUG/issues" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d '{"title":"cancel"}')
ISSUE_ID=$(echo "$ISSUE_JSON" | jq -r .id)
ISSUE_NUM=$(echo "$ISSUE_JSON" | jq -r .number)
curl -sf -X POST "http://127.0.0.1:${API_PORT}/api/v1/orgs/demo/repos/$REPO_SLUG/issues/$ISSUE_NUM/plans" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d '{"body":"p"}' >/dev/null
curl -sf -X PATCH "http://127.0.0.1:${API_PORT}/api/v1/orgs/demo/repos/$REPO_SLUG/issues/$ISSUE_NUM/plans" \
  -H "Cookie: musehub_session=$COOKIE" >/dev/null

AGENT_JSON=$(curl -sf -X POST "http://127.0.0.1:${API_PORT}/api/v1/orgs/demo/repos/$REPO_SLUG/agent-runs" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d "{\"issue_id\":\"$ISSUE_ID\",\"message\":\"__slow__\"}")
RUN_ID=$(echo "$AGENT_JSON" | jq -r .id)

REF_BIN="${CARGO_TARGET_DIR:-$ROOT/target}/debug/musehub-agent-reference"
export RUNNER_PAT
remote_runner_start_runnerd "$REF_BIN"
sleep 1

for _ in $(seq 1 120); do
  st=$(psql "$DATABASE_URL" -Atc "SELECT state FROM agent_runs WHERE id='$RUN_ID'")
  [[ "$st" == "running" ]] && break
  [[ "$st" == "succeeded" || "$st" == "failed" ]] && { echo "unexpected early state=$st" >&2; exit 1; }
  sleep 0.5
done
[[ "$(psql "$DATABASE_URL" -Atc "SELECT state FROM agent_runs WHERE id='$RUN_ID'")" == "running" ]]

cancel_code=$(curl -sS -X POST -o /tmp/agent-cancel.json -w "%{http_code}" \
  -H "Cookie: musehub_session=$COOKIE" \
  "http://127.0.0.1:${API_PORT}/api/v1/agent-runs/${RUN_ID}/cancel")
[[ "$cancel_code" =~ ^2 ]]

for _ in $(seq 1 120); do
  st=$(psql "$DATABASE_URL" -Atc "SELECT state FROM agent_runs WHERE id='$RUN_ID'")
  [[ "$st" == "canceled" ]] && break
  sleep 0.5
done
[[ "$st" == "canceled" ]]
commit=$(psql "$DATABASE_URL" -Atc "SELECT coalesce(result_commit_oid,'') FROM agent_runs WHERE id='$RUN_ID'")
[[ -z "$commit" ]]

BARE="$GIT_ROOT/${STORAGE_KEY}.git"
branch=$(psql "$DATABASE_URL" -Atc "SELECT work_branch FROM agent_runs WHERE id='$RUN_ID'")
! git --git-dir="$BARE" rev-parse "$branch" >/dev/null 2>&1

echo "remote_agent_cancel_subprocess_e2e=ok"
