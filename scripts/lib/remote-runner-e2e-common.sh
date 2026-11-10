#!/usr/bin/env bash
# Shared helpers for subprocess remote-runner acceptance (server + musehub-runnerd).
e2e_uuid() {
  if command -v uuidgen >/dev/null 2>&1; then
    uuidgen
  elif [[ -r /proc/sys/kernel/random/uuid ]]; then
    cat /proc/sys/kernel/random/uuid
  else
    python3 -c 'import uuid; print(uuid.uuid4())'
  fi
}

remote_runner_e2e_init() {
  set -euo pipefail
  ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  cd "$ROOT"
  export DATABASE_URL="${MUSEHUB_TEST_DATABASE_URL:-${DATABASE_URL:-}}"
  test -n "$DATABASE_URL" || { echo "MUSEHUB_TEST_DATABASE_URL or DATABASE_URL required" >&2; exit 1; }
  if [[ -n "${MUSEHUB_GIT_ROOT:-}" ]]; then
    GIT_ROOT="$MUSEHUB_GIT_ROOT"
    E2E_CREATED_GIT_ROOT=0
  else
    GIT_ROOT="$(mktemp -d)"
    E2E_CREATED_GIT_ROOT=1
  fi
  ARTIFACT_ROOT="${MUSEHUB_ARTIFACT_ROOT:-$GIT_ROOT/artifacts}"
  mkdir -p "$ARTIFACT_ROOT"
  export MUSEHUB_GIT_ROOT="$GIT_ROOT"
  export MUSEHUB_ARTIFACT_ROOT="$ARTIFACT_ROOT"
  export MUSEHUB_PIPELINE_EXECUTOR="${MUSEHUB_PIPELINE_EXECUTOR:-remote}"
  export MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD="${MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD:-Remote-Runner-E2E-Strong-1!}"
  export MUSEHUB_SESSION_SECRET="${MUSEHUB_SESSION_SECRET:-$(openssl rand -hex 32)}"
  API_PORT="${MUSEHUB_API_PORT:-$((52000 + RANDOM % 10000))}"
  export MUSEHUB_BIND="127.0.0.1:${API_PORT}"
  export MUSEHUB_API_BASE="http://127.0.0.1:${API_PORT}"
  command -v jq >/dev/null || { echo "jq required" >&2; exit 1; }
  cargo build -q -p musehub-server -p musehub-runnerd -p musehub-agent-adapter
  cargo run -q -p musehub-migrate -- up migrations "$DATABASE_URL" >/dev/null
  BIN_SERVER="${CARGO_TARGET_DIR:-$ROOT/target}/debug/musehub-server"
  BIN_RUNNERD="${CARGO_TARGET_DIR:-$ROOT/target}/debug/musehub-runnerd"
  SERVER_PID=""
  RUNNER_PID=""
}

remote_runner_e2e_cleanup() {
  if [[ -n "${RUNNER_PID:-}" ]]; then kill "$RUNNER_PID" 2>/dev/null || true; fi
  if [[ -n "${RUNNER_PID_2:-}" ]]; then kill "$RUNNER_PID_2" 2>/dev/null || true; fi
  if [[ -n "${SERVER_PID:-}" ]]; then kill "$SERVER_PID" 2>/dev/null || true; fi
  if [[ "${E2E_CREATED_GIT_ROOT:-0}" == "1" && "${MUSEHUB_KEEP_GIT_ROOT:-}" != "1" ]]; then
    rm -rf "$GIT_ROOT"
  fi
}

remote_runner_wait_ready() {
  for _ in $(seq 1 80); do
    curl -sf "http://127.0.0.1:${API_PORT}/readyz" | jq -e '.checks.product_api == "ok"' >/dev/null && return 0
    sleep 0.5
  done
  echo "server not ready" >&2
  return 1
}

remote_runner_start_server() {
  local server_env=(
    DATABASE_URL="$DATABASE_URL"
    MUSEHUB_GIT_ROOT="$GIT_ROOT"
    MUSEHUB_ARTIFACT_ROOT="$ARTIFACT_ROOT"
    MUSEHUB_SESSION_SECRET="$MUSEHUB_SESSION_SECRET"
    MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD="$MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD"
    MUSEHUB_PIPELINE_EXECUTOR=remote
    MUSEHUB_BIND="$MUSEHUB_BIND"
  )
  if [[ -n "${MUSEHUB_LEASE_SECS:-}" ]]; then
    server_env+=(MUSEHUB_LEASE_SECS="$MUSEHUB_LEASE_SECS")
  fi
  if [[ -n "${MUSEHUB_AGENT_EXECUTOR:-}" ]]; then
    server_env+=(MUSEHUB_AGENT_EXECUTOR="$MUSEHUB_AGENT_EXECUTOR")
  fi
  if [[ -n "${MUSEHUB_DEPLOY_EXECUTOR:-}" ]]; then
    server_env+=(MUSEHUB_DEPLOY_EXECUTOR="$MUSEHUB_DEPLOY_EXECUTOR")
  fi
  if [[ -n "${MUSEHUB_COMPOSE_ADAPTER_MODE:-}" ]]; then
    server_env+=(MUSEHUB_COMPOSE_ADAPTER_MODE="$MUSEHUB_COMPOSE_ADAPTER_MODE")
  fi
  if [[ -n "${MUSEHUB_PIPELINE_EXECUTOR:-}" ]]; then
    server_env+=(MUSEHUB_PIPELINE_EXECUTOR="$MUSEHUB_PIPELINE_EXECUTOR")
  fi
  env "${server_env[@]}" "$BIN_SERVER" &
  SERVER_PID=$!
  remote_runner_wait_ready
}

remote_runner_login_cookie() {
  local pass cookie
  local candidates=(
    "${MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD:-}"
    "Ci-Bootstrap-Strong!2026"
    "Acceptance-Bootstrap-Strong-1!"
    "Remote-Runner-E2E-Strong-1!"
    "Product-Stack-E2E-Strong-1!"
  )
  for pass in "${candidates[@]}"; do
    [[ -z "$pass" ]] && continue
    cookie=$(curl -sf -c - -X POST "http://127.0.0.1:${API_PORT}/api/v1/auth/login" \
      -H 'Content-Type: application/json' \
      -d "{\"handle\":\"admin\",\"password\":\"$pass\"}" | awk '/musehub_session/ {print $NF}') || true
    if [[ -n "$cookie" ]]; then
      echo "$cookie"
      return 0
    fi
  done
  echo "admin login failed (set MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD to match this DB)" >&2
  return 1
}

remote_runner_bootstrap_repo() {
  local slug="${1:-subproc-pipe-$(e2e_uuid | tr '[:upper:]' '[:lower:]' | cut -c1-8)}"
  local member_handle="${2:-subproc-runner}"
  local member_pass="${3:-Subproc-Runner-Member-1!}"
  if [[ -z "${COOKIE:-}" ]]; then
    COOKIE=$(remote_runner_login_cookie)
  fi
  curl -sf -X POST "http://127.0.0.1:${API_PORT}/api/v1/orgs/demo/members" \
    -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
    -d "{\"handle\":\"$member_handle\",\"password\":\"$member_pass\",\"role\":\"member\"}" >/dev/null || true
  curl -sf -X POST "http://127.0.0.1:${API_PORT}/api/v1/orgs/demo/repos" \
    -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
    -d "{\"slug\":\"$slug\",\"description\":\"runner e2e\"}" >/dev/null
  REPO_ID=$(psql "$DATABASE_URL" -Atc "SELECT id FROM repositories WHERE slug='$slug'")
  STORAGE_KEY=$(psql "$DATABASE_URL" -Atc "SELECT storage_key FROM repositories WHERE slug='$slug'")
  curl -sf -X POST "http://127.0.0.1:${API_PORT}/api/v1/orgs/demo/repos/$slug/collaborators" \
    -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
    -d "{\"handle\":\"$member_handle\",\"role\":\"developer\"}" >/dev/null
  PAT_JSON=$(curl -sf -X POST "http://127.0.0.1:${API_PORT}/api/v1/tokens" \
    -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
    -d "{\"name\":\"subproc\",\"org\":\"demo\",\"scopes\":[\"pipeline:execute\",\"repo:read\"],\"scope_mode\":\"selected\",\"repo_ids\":[\"$REPO_ID\"],\"runner_handle\":\"$member_handle\"}")
  RUNNER_PAT=$(echo "$PAT_JSON" | jq -r .token)
  BARE="$GIT_ROOT/${STORAGE_KEY}.git"
  SOURCE_OID=$(git --git-dir="$BARE" rev-parse refs/heads/main)
  export RUNNER_PAT SOURCE_OID REPO_ID STORAGE_KEY COOKIE slug member_handle REPO_SLUG="$slug"
}

remote_runner_start_runnerd() {
  local adapter_bin="$1"
  local instance_id="${2:-}"
  local env_extra=("${@:3}")
  local ref_bin="${MUSEHUB_REFERENCE_ADAPTER_BIN:-${CARGO_TARGET_DIR:-$ROOT/target}/debug/musehub-agent-reference}"
  local pipe_bin="${MUSEHUB_PIPELINE_ADAPTER_BIN:-$adapter_bin}"
  local runner_env=(
    MUSEHUB_API_BASE="$MUSEHUB_API_BASE"
    MUSEHUB_DATABASE_URL="$DATABASE_URL"
    MUSEHUB_GIT_ROOT="$GIT_ROOT"
    MUSEHUB_ARTIFACT_ROOT="$ARTIFACT_ROOT"
    MUSEHUB_PIPELINE_ADAPTER_BIN="$pipe_bin"
    MUSEHUB_REFERENCE_ADAPTER_BIN="$ref_bin"
    MUSEHUB_RUNNER_PAT="$RUNNER_PAT"
    MUSEHUB_RUNNER_POLL_SECS="${MUSEHUB_RUNNER_POLL_SECS:-1}"
  )
  if [[ -n "${MUSEHUB_RUNNER_WORK_KINDS:-}" ]]; then
    runner_env+=(MUSEHUB_RUNNER_WORK_KINDS="$MUSEHUB_RUNNER_WORK_KINDS")
  fi
  if [[ -n "${MUSEHUB_COMPOSE_RUNNER_BIN:-}" ]]; then
    runner_env+=(MUSEHUB_COMPOSE_RUNNER_BIN="$MUSEHUB_COMPOSE_RUNNER_BIN")
    runner_env+=(MUSEHUB_COMPOSE_ADAPTER_BIN="${MUSEHUB_COMPOSE_ADAPTER_BIN:-$MUSEHUB_COMPOSE_RUNNER_BIN}")
  fi
  if [[ -n "${MUSEHUB_REQUIRE_PIPELINE_OCI:-}" ]]; then
    runner_env+=(MUSEHUB_REQUIRE_PIPELINE_OCI="$MUSEHUB_REQUIRE_PIPELINE_OCI")
  fi
  if [[ -n "${MUSEHUB_PIPELINE_HOLD_SECS:-}" ]]; then
    runner_env+=(MUSEHUB_PIPELINE_HOLD_SECS="$MUSEHUB_PIPELINE_HOLD_SECS")
  fi
  if [[ -n "${MUSEHUB_LEASE_SECS:-}" ]]; then
    runner_env+=(MUSEHUB_LEASE_SECS="$MUSEHUB_LEASE_SECS")
  fi
  if [[ -n "${MUSEHUB_RUNNER_LEASE_RENEW_SECS:-}" ]]; then
    runner_env+=(MUSEHUB_RUNNER_LEASE_RENEW_SECS="$MUSEHUB_RUNNER_LEASE_RENEW_SECS")
  fi
  if [[ -n "$instance_id" ]]; then
    runner_env+=(MUSEHUB_RUNNER_INSTANCE_ID="$instance_id")
  fi
  env "${runner_env[@]}" "${env_extra[@]}" "$BIN_RUNNERD" &
  RUNNER_PID=$!
}

remote_runner_enqueue_validation() {
  local run_id job_id attempt_id admin_id org_id
  run_id=$(e2e_uuid)
  job_id=$(e2e_uuid)
  attempt_id=$(e2e_uuid)
  admin_id=$(psql "$DATABASE_URL" -Atc "SELECT id FROM principals WHERE handle_normalized='admin'")
  org_id=$(psql "$DATABASE_URL" -Atc "SELECT id FROM organizations WHERE slug='demo'")
  psql "$DATABASE_URL" -v ON_ERROR_STOP=1 <<-SQL
    INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state,
      requested_by, run_attempt_id, run_kind, cancel_requested)
      VALUES ('$run_id', '$org_id', '$REPO_ID', '$SOURCE_OID', 'product-ci', 'queued', '$admin_id', '$attempt_id', 'pr_validation', false);
    INSERT INTO pipeline_jobs (id, org_id, repo_id, pipeline_run_id, job_key, state)
      VALUES ('$job_id', '$org_id', '$REPO_ID', '$run_id', 'build', 'queued');
SQL
  export E2E_RUN_ID="$run_id" E2E_JOB_ID="$job_id"
}

remote_runner_poll_state() {
  local run_id="$1"
  psql "$DATABASE_URL" -Atc "SELECT state FROM pipeline_runs WHERE id='$run_id'"
}

remote_runner_adapter_pid() {
  local run_id="$1"
  local pid envf
  for pid in $(pgrep -f 'musehub-pipeline-' 2>/dev/null || true); do
    envf="/proc/$pid/environ"
    if [[ -r "$envf" ]] && tr '\0' '\n' <"$envf" | grep -q "MUSEHUB_PIPELINE_RUN_ID=${run_id}"; then
      echo "$pid"
      return 0
    fi
  done
  return 1
}

remote_runner_count_ready_artifacts() {
  local run_id="$1"
  psql "$DATABASE_URL" -Atc "SELECT COUNT(*) FROM artifacts WHERE pipeline_run_id='$run_id' AND state='ready'"
}
