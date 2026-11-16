#!/usr/bin/env bash
# Explicitly billed acceptance: real Codex CLI through the remote AgentRun worker.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [[ "${MUSEHUB_LIVE_CODEX:-}" != "1" ]]; then
  echo "Live Codex execution is opt-in; set MUSEHUB_LIVE_CODEX=1 to continue." >&2
  exit 2
fi
: "${MUSEHUB_AGENT_CODEX_BIN:?set MUSEHUB_AGENT_CODEX_BIN to the Codex CLI executable}"
: "${MUSEHUB_LIVE_CODEX_BUDGET_TOKENS:?set an explicit MUSEHUB_LIVE_CODEX_BUDGET_TOKENS limit}"
[[ -x "$MUSEHUB_AGENT_CODEX_BIN" ]] || { echo "Codex CLI is not executable: $MUSEHUB_AGENT_CODEX_BIN" >&2; exit 2; }
[[ "$MUSEHUB_LIVE_CODEX_BUDGET_TOKENS" =~ ^[1-9][0-9]*$ ]] || {
  echo "MUSEHUB_LIVE_CODEX_BUDGET_TOKENS must be a positive integer" >&2
  exit 2
}
(( MUSEHUB_LIVE_CODEX_BUDGET_TOKENS <= 100000 )) || {
  echo "token budget must not exceed 100000 for this acceptance task" >&2
  exit 2
}

# shellcheck source=lib/remote-runner-e2e-common.sh
source "$ROOT/scripts/lib/remote-runner-e2e-common.sh"
export MUSEHUB_AGENT_EXECUTOR=remote
export MUSEHUB_RUNNER_WORK_KINDS=agent
export MUSEHUB_AGENT_CODEX_BIN

RUN_ID=""
RUNNER_TOKEN_ID=""
CANCEL_CONFIRM_TIMEOUT_SECS="${MUSEHUB_LIVE_CODEX_CANCEL_CONFIRM_TIMEOUT_SECS:-30}"
[[ "$CANCEL_CONFIRM_TIMEOUT_SECS" =~ ^[1-9][0-9]*$ ]] || {
  echo "MUSEHUB_LIVE_CODEX_CANCEL_CONFIRM_TIMEOUT_SECS must be a positive integer" >&2
  exit 2
}
is_terminal_agent_run_state() {
  case "${1:-}" in
    succeeded|failed|canceled) return 0 ;;
    *) return 1 ;;
  esac
}
revoke_live_codex_token() {
  [[ -n "${RUNNER_TOKEN_ID:-}" && -n "${DATABASE_URL:-}" ]] || return 0
  if ! psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -v token_id="$RUNNER_TOKEN_ID" \
    -c "UPDATE access_tokens SET revoked_at = CURRENT_TIMESTAMP WHERE id = :'token_id'::uuid AND revoked_at IS NULL" \
    >/dev/null; then
    return 1
  fi
  RUNNER_TOKEN_ID=""
}
live_codex_cleanup() {
  local status=$?
  local state=""
  local elapsed
  if [[ -n "${RUN_ID:-}" && -n "${COOKIE:-}" && -n "${API_PORT:-}" ]]; then
    curl --max-time 5 -sf -X POST \
      -H "Cookie: musehub_session=$COOKIE" \
      "http://127.0.0.1:${API_PORT}/api/v1/agent-runs/${RUN_ID}/cancel" >/dev/null || true
    for ((elapsed=0; elapsed<CANCEL_CONFIRM_TIMEOUT_SECS; elapsed++)); do
      state=$(psql "$DATABASE_URL" -Atc "SELECT state FROM agent_runs WHERE id='$RUN_ID'" 2>/dev/null || true)
      is_terminal_agent_run_state "$state" && break
      sleep 1
    done
    state=$(psql "$DATABASE_URL" -Atc "SELECT state FROM agent_runs WHERE id='$RUN_ID'" 2>/dev/null || true)
    if ! is_terminal_agent_run_state "$state"; then
      printf 'error: AgentRun cancellation unconfirmed after %s seconds (run_id=%s state=%s); preserving workspace at %s\n' \
        "$CANCEL_CONFIRM_TIMEOUT_SECS" "$RUN_ID" "${state:-unknown}" "$GIT_ROOT" >&2
      export MUSEHUB_KEEP_GIT_ROOT=1
      status=1
    fi
  fi
  if ! revoke_live_codex_token; then
    echo "warning: could not revoke temporary runner token" >&2
    if (( status == 0 )); then
      status=1
    fi
  fi
  remote_runner_e2e_cleanup
  exit "$status"
}
trap live_codex_cleanup EXIT
remote_runner_e2e_init
remote_runner_start_server
COOKIE=$(remote_runner_login_cookie)
export COOKIE
RUNNER_MEMBER="live-codex-$(e2e_uuid | cut -c1-8)"
remote_runner_bootstrap_repo "live-codex-$(e2e_uuid | cut -c1-8)" "$RUNNER_MEMBER" "Live-Codex-Member-1!"

ISSUE_JSON=$(curl -sf -X POST "$MUSEHUB_API_BASE/api/v1/orgs/demo/repos/$REPO_SLUG/issues" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d '{"title":"Live Codex acceptance"}')
ISSUE_ID=$(jq -r .id <<<"$ISSUE_JSON")
ISSUE_NUM=$(jq -r .number <<<"$ISSUE_JSON")
curl -sf -X POST "$MUSEHUB_API_BASE/api/v1/orgs/demo/repos/$REPO_SLUG/issues/$ISSUE_NUM/plans" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d '{"body":"Create and commit the acceptance proof file."}' >/dev/null
curl -sf -X PATCH "$MUSEHUB_API_BASE/api/v1/orgs/demo/repos/$REPO_SLUG/issues/$ISSUE_NUM/plans" \
  -H "Cookie: musehub_session=$COOKIE" >/dev/null

PAT_JSON=$(curl -sf -X POST "$MUSEHUB_API_BASE/api/v1/tokens" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d "{\"name\":\"live-codex-runner\",\"org\":\"demo\",\"scopes\":[\"agent:execute\",\"repo:read\"],\"scope_mode\":\"selected\",\"repo_ids\":[\"$REPO_ID\"],\"runner_handle\":\"$RUNNER_MEMBER\"}")
RUNNER_TOKEN_ID=$(jq -r .id <<<"$PAT_JSON")
RUNNER_PAT=$(jq -r .token <<<"$PAT_JSON")

PROOF="MuseHub live Codex acceptance: $ISSUE_ID"
TASK_MESSAGE="Create LIVE_CODEX_PROOF.md containing exactly this one line: $PROOF. Commit the file on the assigned agent branch."
AGENT_JSON=$(jq -n \
  --arg issue_id "$ISSUE_ID" \
  --arg message "$TASK_MESSAGE" \
  --argjson budget "$MUSEHUB_LIVE_CODEX_BUDGET_TOKENS" \
  '{issue_id:$issue_id,message:$message,adapter:"codex",budget_tokens:$budget}')
AGENT_JSON=$(curl -sf -X POST "$MUSEHUB_API_BASE/api/v1/orgs/demo/repos/$REPO_SLUG/agent-runs" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d "$AGENT_JSON")
RUN_ID=$(jq -r .id <<<"$AGENT_JSON")
REF_BIN="${CARGO_TARGET_DIR:-$ROOT/target}/debug/musehub-agent-reference"
remote_runner_start_runnerd "$REF_BIN"

MAX_WAIT_SECS="${MUSEHUB_LIVE_CODEX_TIMEOUT_SECS:-900}"
[[ "$MAX_WAIT_SECS" =~ ^[1-9][0-9]*$ ]] || { echo "invalid timeout" >&2; exit 2; }
for ((elapsed=0; elapsed<MAX_WAIT_SECS; elapsed++)); do
  state=$(psql "$DATABASE_URL" -Atc "SELECT state FROM agent_runs WHERE id='$RUN_ID'")
  is_terminal_agent_run_state "$state" && break
  sleep 1
done
if ! is_terminal_agent_run_state "$state"; then
  echo "live Codex acceptance timed out; requesting cancellation for run $RUN_ID" >&2
  curl --max-time 5 -sf -X POST \
    -H "Cookie: musehub_session=$COOKIE" \
    "$MUSEHUB_API_BASE/api/v1/agent-runs/${RUN_ID}/cancel" >/dev/null || true
  exit 1
fi

ROW=$(psql "$DATABASE_URL" -At -F '|' -c \
  "SELECT state, adapter_kind, issue_id, repo_id, work_branch, coalesce(result_commit_oid,''), coalesce(usage_tokens,0) FROM agent_runs WHERE id='$RUN_ID'")
IFS='|' read -r state adapter issue_id repo_id branch commit tokens <<<"$ROW"
[[ "$state" == "succeeded" ]] || { echo "live Codex run ended in state=$state" >&2; exit 1; }
[[ "$adapter" == "codex" && "$issue_id" == "$ISSUE_ID" && "$repo_id" == "$REPO_ID" ]] || {
  echo "AgentRun identity or adapter metadata mismatch" >&2
  exit 1
}
[[ -n "$commit" && "$tokens" =~ ^[0-9]+$ && "$tokens" -gt 0 ]] || {
  echo "AgentRun did not record a commit and positive token usage" >&2
  exit 1
}
(( tokens <= MUSEHUB_LIVE_CODEX_BUDGET_TOKENS )) || {
  echo "recorded token usage exceeded the configured budget" >&2
  exit 1
}
BARE="$GIT_ROOT/${STORAGE_KEY}.git"
branch_commit=$(git --git-dir="$BARE" rev-parse "refs/heads/$branch")
[[ "$branch_commit" == "$commit" ]] || { echo "bare branch does not point to AgentRun commit" >&2; exit 1; }
actual_proof=$(git --git-dir="$BARE" show "$commit:LIVE_CODEX_PROOF.md")
[[ "$actual_proof" == "$PROOF" ]] || { echo "committed proof content mismatch" >&2; exit 1; }
revoke_live_codex_token

echo "live_codex_agent_e2e=ok run_id=$RUN_ID commit=$commit usage_tokens=$tokens budget_tokens=$MUSEHUB_LIVE_CODEX_BUDGET_TOKENS"
RUN_ID=""
