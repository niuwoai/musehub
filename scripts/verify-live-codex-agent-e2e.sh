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
(( MUSEHUB_LIVE_CODEX_BUDGET_TOKENS <= 128000 )) || {
  echo "token budget must not exceed 128000 for this acceptance task" >&2
  exit 2
}

# shellcheck source=lib/remote-runner-e2e-common.sh
source "$ROOT/scripts/lib/remote-runner-e2e-common.sh"
export MUSEHUB_AGENT_EXECUTOR=remote
export MUSEHUB_RUNNER_WORK_KINDS=agent
export MUSEHUB_AGENT_CODEX_BIN
export MUSEHUB_AGENT_CODEX_REASONING_EFFORT="${MUSEHUB_AGENT_CODEX_REASONING_EFFORT:-low}"

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
api_request() {
  local context="$1"
  shift
  local response http_status body
  response=$(curl --silent --show-error --write-out $'\n%{http_code}' "$@") || {
    printf 'API transport failed: %s\n' "$context" >&2
    return 1
  }
  http_status="${response##*$'\n'}"
  body="${response%$'\n'*}"
  if [[ ! "$http_status" =~ ^2[0-9][0-9]$ ]]; then
    printf 'API request failed: %s (HTTP %s): %s\n' "$context" "$http_status" "$body" >&2
    return 1
  fi
  printf '%s' "$body"
}
revoke_live_codex_token() {
  [[ -n "${RUNNER_TOKEN_ID:-}" && -n "${DATABASE_URL:-}" ]] || return 0
  if ! psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -v token_id="$RUNNER_TOKEN_ID" -f - >/dev/null <<'SQL'
UPDATE access_tokens
SET revoked_at = CURRENT_TIMESTAMP
WHERE id = :'token_id'::uuid AND revoked_at IS NULL;
SQL
  then
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
  if (( status != 0 )) && [[ -n "${RUN_ID:-}" && "${E2E_CREATED_GIT_ROOT:-0}" == "1" ]]; then
    printf 'Live Codex acceptance failed; preserving workspace at %s for diagnosis\n' "$GIT_ROOT" >&2
    export MUSEHUB_KEEP_GIT_ROOT=1
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

ISSUE_JSON=$(api_request "create acceptance issue" -X POST "$MUSEHUB_API_BASE/api/v1/orgs/demo/repos/$REPO_SLUG/issues" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d '{"title":"Live Codex acceptance"}')
ISSUE_ID=$(jq -r .id <<<"$ISSUE_JSON")
ISSUE_NUM=$(jq -r .number <<<"$ISSUE_JSON")
api_request "create acceptance plan" -X POST "$MUSEHUB_API_BASE/api/v1/orgs/demo/repos/$REPO_SLUG/issues/$ISSUE_NUM/plans" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d '{"body":"Create and commit the acceptance proof file."}' >/dev/null
api_request "approve acceptance plan" -X PATCH "$MUSEHUB_API_BASE/api/v1/orgs/demo/repos/$REPO_SLUG/issues/$ISSUE_NUM/plans" \
  -H "Cookie: musehub_session=$COOKIE" >/dev/null

PAT_JSON=$(api_request "create scoped runner token" -X POST "$MUSEHUB_API_BASE/api/v1/tokens" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d "{\"name\":\"live-codex-runner\",\"org\":\"demo\",\"scopes\":[\"agent:execute\",\"repo:read\"],\"scope_mode\":\"selected\",\"repo_ids\":[\"$REPO_ID\"],\"runner_handle\":\"$RUNNER_MEMBER\"}")
RUNNER_TOKEN_ID=$(jq -r .id <<<"$PAT_JSON")
RUNNER_PAT=$(jq -r .token <<<"$PAT_JSON")

PROOF="MuseHub live Codex acceptance: $ISSUE_ID"
TASK_MESSAGE="Create LIVE_CODEX_PROOF.md whose entire contents are exactly the single line between BEGIN_PROOF and END_PROOF below. Do not include the markers or add punctuation.
BEGIN_PROOF
$PROOF
END_PROOF
Do not inspect or modify any other path. This empty acceptance repository has no test suite; do not explore or run broad checks. Immediately run these commands in order: git diff --check; git add -- LIVE_CODEX_PROOF.md; git -c user.name='MuseHub Live Acceptance' -c user.email='live-codex@musehub.local' -c commit.gpgsign=false commit -m 'Add live Codex acceptance proof'; git status --porcelain; git rev-parse HEAD. If the commit fails, diagnose and fix the Git error, then retry. Do not finish until the commit succeeds and git status is empty."
AGENT_JSON=$(jq -n \
  --arg issue_id "$ISSUE_ID" \
  --arg message "$TASK_MESSAGE" \
  --argjson budget "$MUSEHUB_LIVE_CODEX_BUDGET_TOKENS" \
  '{issue_id:$issue_id,message:$message,adapter:"codex",budget_tokens:$budget}')
AGENT_JSON=$(api_request "create Codex AgentRun" -X POST "$MUSEHUB_API_BASE/api/v1/orgs/demo/repos/$REPO_SLUG/agent-runs" \
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
  "SELECT state, adapter_kind, issue_id, repo_id, work_branch, coalesce(result_commit_oid,''), coalesce(usage_tokens,0), coalesce(error_code,'') FROM agent_runs WHERE id='$RUN_ID'")
IFS='|' read -r state adapter issue_id repo_id branch commit tokens error_code <<<"$ROW"
USAGE_EVIDENCE=$(psql "$DATABASE_URL" -Atc \
  "SELECT coalesce(usage_evidence::text,'') FROM agent_runs WHERE id='$RUN_ID'")
if [[ -n "$USAGE_EVIDENCE" ]]; then
  echo "codex_usage_evidence=$USAGE_EVIDENCE"
fi
[[ "$state" == "succeeded" ]] || {
  echo "live Codex run ended in state=$state error_code=${error_code:-none} usage_tokens=${tokens:-0}" >&2
  exit 1
}
[[ "$adapter" == "codex" && "$issue_id" == "$ISSUE_ID" && "$repo_id" == "$REPO_ID" ]] || {
  echo "AgentRun identity or adapter metadata mismatch" >&2
  exit 1
}
[[ -n "$commit" && "$tokens" =~ ^[0-9]+$ && "$tokens" -gt 0 ]] || {
  echo "AgentRun did not record a commit and positive token usage" >&2
  exit 1
}
[[ -n "$USAGE_EVIDENCE" ]] || {
  echo "AgentRun did not persist provider-reported Codex usage evidence" >&2
  exit 1
}
REPORTED_USAGE=$(jq -ce '
  select(.protocol == "codex_exec_jsonl" and .event_type == "turn.completed") | .usage
' <<<"$USAGE_EVIDENCE") || {
  echo "AgentRun usage evidence does not identify a Codex turn.completed event" >&2
  exit 1
}
REPORTED_TOKENS=$(jq -er '
  if (.total_tokens | type) == "number" then .total_tokens
  elif ((.input_tokens | type) == "number" and (.output_tokens | type) == "number") then .input_tokens + .output_tokens
  else error("usage object has no supported token fields") end
' <<<"$REPORTED_USAGE") || {
  echo "Codex usage evidence has no verifiable token total" >&2
  exit 1
}
[[ "$REPORTED_TOKENS" == "$tokens" ]] || {
  echo "database usage_tokens=$tokens does not match provider evidence=$REPORTED_TOKENS" >&2
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
