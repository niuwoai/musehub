#!/usr/bin/env bash
# Subprocess acceptance: API (remote executor) + musehub-runnerd, shared DB/volumes.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export DATABASE_URL="${MUSEHUB_TEST_DATABASE_URL:-${DATABASE_URL:-}}"
test -n "$DATABASE_URL" || { echo "MUSEHUB_TEST_DATABASE_URL or DATABASE_URL required" >&2; exit 1; }

GIT_ROOT="$(mktemp -d)"
ARTIFACT_ROOT="$GIT_ROOT/artifacts"
mkdir -p "$ARTIFACT_ROOT"
export MUSEHUB_GIT_ROOT="$GIT_ROOT"
export MUSEHUB_ARTIFACT_ROOT="$ARTIFACT_ROOT"
export MUSEHUB_PIPELINE_EXECUTOR=remote
export MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD="${MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD:-Remote-Runner-E2E-Strong-1!}"
export MUSEHUB_SESSION_SECRET="${MUSEHUB_SESSION_SECRET:-$(openssl rand -hex 32)}"
API_PORT=$((52000 + RANDOM % 10000))
export MUSEHUB_BIND="127.0.0.1:${API_PORT}"

command -v jq >/dev/null || { echo "jq required" >&2; exit 1; }

cargo build -q -p musehub-server -p musehub-runnerd -p musehub-agent-adapter
cargo test -q -p musehub-app --test runner_daemon_acceptance_e2e -- --test-threads=1
cargo run -q -p musehub-migrate -- up migrations "$DATABASE_URL" >/dev/null

BIN_SERVER="${CARGO_TARGET_DIR:-$ROOT/target}/debug/musehub-server"
BIN_RUNNERD="${CARGO_TARGET_DIR:-$ROOT/target}/debug/musehub-runnerd"
PIPE_BIN="${CARGO_TARGET_DIR:-$ROOT/target}/debug/musehub-pipeline-reference"

cleanup() {
  if [[ -n "${RUNNER_PID:-}" ]]; then kill "$RUNNER_PID" 2>/dev/null || true; fi
  if [[ -n "${SERVER_PID:-}" ]]; then kill "$SERVER_PID" 2>/dev/null || true; fi
  rm -rf "$GIT_ROOT"
}
trap cleanup EXIT

DATABASE_URL="$DATABASE_URL" \
MUSEHUB_GIT_ROOT="$GIT_ROOT" \
MUSEHUB_SESSION_SECRET="$MUSEHUB_SESSION_SECRET" \
MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD="$MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD" \
MUSEHUB_PIPELINE_EXECUTOR=remote \
MUSEHUB_BIND="127.0.0.1:${API_PORT}" \
"$BIN_SERVER" &
SERVER_PID=$!
for _ in $(seq 1 60); do
  curl -sf "http://127.0.0.1:${API_PORT}/readyz" | jq -e '.checks.product_api == "ok"' >/dev/null && break
  sleep 0.5
done
curl -sf "http://127.0.0.1:${API_PORT}/readyz" | jq -e '.checks.product_api == "ok"' >/dev/null

COOKIE=$(curl -sf -c - -X POST "http://127.0.0.1:${API_PORT}/api/v1/auth/login" \
  -H 'Content-Type: application/json' \
  -d "{\"handle\":\"admin\",\"password\":\"$MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD\"}" | awk '/musehub_session/ {print $NF}')

curl -sf -X POST "http://127.0.0.1:${API_PORT}/api/v1/orgs/demo/members" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d '{"handle":"subproc-runner","password":"Subproc-Runner-Member-1!","role":"member"}' >/dev/null

curl -sf -X POST "http://127.0.0.1:${API_PORT}/api/v1/orgs/demo/repos" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d '{"slug":"subproc-pipe","description":""}' >/dev/null

REPO_ID=$(psql "$DATABASE_URL" -Atc "SELECT id FROM repositories WHERE slug='subproc-pipe'")

curl -sf -X POST "http://127.0.0.1:${API_PORT}/api/v1/orgs/demo/repos/subproc-pipe/collaborators" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d '{"handle":"subproc-runner","role":"developer"}' >/dev/null

PAT_JSON=$(curl -sf -X POST "http://127.0.0.1:${API_PORT}/api/v1/tokens" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d "{\"name\":\"subproc\",\"org\":\"demo\",\"scopes\":[\"pipeline:execute\",\"repo:read\"],\"scope_mode\":\"selected\",\"repo_ids\":[\"$REPO_ID\"],\"runner_handle\":\"subproc-runner\"}")
RUNNER_PAT=$(echo "$PAT_JSON" | jq -r .token)

curl -sf -b "musehub_session=$COOKIE" "http://127.0.0.1:${API_PORT}/api/v1/runner/capabilities" | jq -e '.pipeline_executor == "remote"' >/dev/null

MUSEHUB_API_BASE="http://127.0.0.1:${API_PORT}" \
MUSEHUB_DATABASE_URL="$DATABASE_URL" \
MUSEHUB_GIT_ROOT="$GIT_ROOT" \
MUSEHUB_ARTIFACT_ROOT="$ARTIFACT_ROOT" \
MUSEHUB_PIPELINE_ADAPTER_BIN="$PIPE_BIN" \
MUSEHUB_RUNNER_PAT="$RUNNER_PAT" \
MUSEHUB_RUNNER_POLL_SECS=1 \
"$BIN_RUNNERD" &
RUNNER_PID=$!

sleep 2
kill -0 "$SERVER_PID" && kill -0 "$RUNNER_PID"

echo "remote_runner_subprocess_e2e=ok"
