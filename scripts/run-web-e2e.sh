#!/usr/bin/env bash
# Browser E2E: musehub-server + Vite + Playwright (requires Postgres bootstrap env).
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ -z "${MUSEHUB_E2E_DOCKER_GROUP:-}" ]] && command -v sg >/dev/null && command -v docker >/dev/null; then
  if ! docker info >/dev/null 2>&1; then
    export MUSEHUB_E2E_DOCKER_GROUP=1
    exec sg docker -c "export MUSEHUB_E2E_DOCKER_GROUP=1 MUSEHUB_TEST_DATABASE_URL='${MUSEHUB_TEST_DATABASE_URL:-${DATABASE_URL:-}}' MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD='${MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD:-}'; cd '$PWD' && ./scripts/run-web-e2e.sh"
  fi
fi

DATABASE_URL="${MUSEHUB_TEST_DATABASE_URL:-${DATABASE_URL:-}}"
: "${DATABASE_URL:?DATABASE_URL or MUSEHUB_TEST_DATABASE_URL required}"
: "${MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD:?set MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD}"

export MUSEHUB_GIT_ROOT="${MUSEHUB_GIT_ROOT:-/tmp/musehub-e2e-git}"
export MUSEHUB_GIT_STORAGE_GROUP="${MUSEHUB_GIT_STORAGE_GROUP:-musehub-git}"
export MUSEHUB_GIT_STORAGE_SUDO_FALLBACK="${MUSEHUB_GIT_STORAGE_SUDO_FALLBACK:-1}"
export MUSEHUB_SESSION_SECRET="${MUSEHUB_SESSION_SECRET:-e2e-session-secret-32chars-min}"
mkdir -p "$MUSEHUB_GIT_ROOT"
if command -v sudo >/dev/null; then
  getent group "$MUSEHUB_GIT_STORAGE_GROUP" >/dev/null 2>&1 || sudo groupadd -r "$MUSEHUB_GIT_STORAGE_GROUP" 2>/dev/null || true
  sudo usermod -aG "$MUSEHUB_GIT_STORAGE_GROUP" "$(whoami)" 2>/dev/null || true
  if sudo chgrp "$MUSEHUB_GIT_STORAGE_GROUP" "$MUSEHUB_GIT_ROOT" 2>/dev/null; then
    sudo chmod 2775 "$MUSEHUB_GIT_ROOT" 2>/dev/null || true
  fi
fi

SERVER_PIDFILE="${MUSEHUB_E2E_PIDFILE:-/tmp/musehub-e2e-server.pid}"
SERVER_LOG="${MUSEHUB_E2E_SERVER_LOG:-/tmp/musehub-e2e-server.log}"
VITE_PIDFILE="${MUSEHUB_E2E_VITE_PID:-/tmp/musehub-e2e-vite.pid}"
RUNNER_PIDFILE="${MUSEHUB_E2E_RUNNER_PID:-/tmp/musehub-e2e-runner.pid}"

stop_owned() {
  local pidfile=$1
  if [[ -f "$pidfile" ]]; then
    local pid
    pid=$(cat "$pidfile")
    if kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
    rm -f "$pidfile"
  fi
}

on_exit() {
  local code=$?
  if [[ "$code" -ne 0 ]]; then
    chmod +x ./scripts/collect-e2e-diagnostics.sh 2>/dev/null || true
    ./scripts/collect-e2e-diagnostics.sh || true
  fi
  stop_owned "$VITE_PIDFILE"
  stop_owned "$RUNNER_PIDFILE"
  stop_owned "$SERVER_PIDFILE"
}
trap on_exit EXIT

cargo run -p musehub-migrate -- down migrations "$DATABASE_URL" || true
cargo run -p musehub-migrate -- up migrations "$DATABASE_URL"
cargo build -p musehub-agent-adapter --bin musehub-agent-reference -q
cargo build -p musehub-agent-adapter --bin musehub-pipeline-reference -q
cargo build -p musehub-agent-adapter --bin musehub-pipeline-oci -q
cargo build -p musehub-agent-adapter --bin musehub-compose-mock -q
cargo build -p musehub-agent-adapter --bin musehub-compose-runner -q
cargo build -p musehub-server -q

export MUSEHUB_ARTIFACT_ROOT="${MUSEHUB_ARTIFACT_ROOT:-/tmp/musehub-e2e-artifacts}"
mkdir -p "$MUSEHUB_ARTIFACT_ROOT"
MUSEHUB_REFERENCE_ADAPTER_BIN="$PWD/target/debug/musehub-agent-reference"
MUSEHUB_PIPELINE_ADAPTER_BIN="$PWD/target/debug/musehub-pipeline-reference"
PRODUCT_OCI=0
docker_ok() {
  command -v docker >/dev/null || return 1
  docker info >/dev/null 2>&1 && return 0
  sg docker -c 'docker info' >/dev/null 2>&1
}

if docker_ok; then
  chmod +x ./scripts/start-ci-registry.sh
  registry_start_ok=0
  if docker info >/dev/null 2>&1; then
    ./scripts/start-ci-registry.sh && registry_start_ok=1 || true
  else
    sg docker -c './scripts/start-ci-registry.sh' && registry_start_ok=1 || true
  fi
  if [[ "$registry_start_ok" == "1" ]]; then
    export MUSEHUB_OCI_REGISTRY="${MUSEHUB_OCI_REGISTRY:-127.0.0.1:5000}"
    export MUSEHUB_REQUIRE_PIPELINE_OCI=1
    export MUSEHUB_COMPOSE_ADAPTER_MODE=runner
    export MUSEHUB_PRODUCT_OCI_E2E=1
    MUSEHUB_PIPELINE_ADAPTER_BIN="$PWD/target/debug/musehub-pipeline-oci"
    PRODUCT_OCI=1
  fi
fi
echo "web_e2e product_oci=$PRODUCT_OCI registry=${MUSEHUB_OCI_REGISTRY:-none}"
if [[ "$PRODUCT_OCI" == "1" ]]; then
  export MUSEHUB_PIPELINE_EXECUTOR=remote
  export MUSEHUB_AGENT_EXECUTOR=remote
  export MUSEHUB_DEPLOY_EXECUTOR=remote
fi
MUSEHUB_COMPOSE_ADAPTER_BIN="$PWD/target/debug/musehub-compose-mock"
MUSEHUB_COMPOSE_RUNNER_BIN="$PWD/target/debug/musehub-compose-runner"
for bin in "$MUSEHUB_REFERENCE_ADAPTER_BIN" "$MUSEHUB_PIPELINE_ADAPTER_BIN" "$MUSEHUB_COMPOSE_ADAPTER_BIN" "$MUSEHUB_COMPOSE_RUNNER_BIN"; do
  test -x "$bin" || { echo "missing adapter binary: $bin" >&2; exit 1; }
done

stop_owned "$SERVER_PIDFILE"
: >"$SERVER_LOG"

(
  env DATABASE_URL="$DATABASE_URL" \
    MUSEHUB_GIT_ROOT="$MUSEHUB_GIT_ROOT" \
    MUSEHUB_ARTIFACT_ROOT="$MUSEHUB_ARTIFACT_ROOT" \
    MUSEHUB_SESSION_SECRET="$MUSEHUB_SESSION_SECRET" \
    MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD="$MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD" \
    MUSEHUB_REFERENCE_ADAPTER_BIN="$MUSEHUB_REFERENCE_ADAPTER_BIN" \
    MUSEHUB_PIPELINE_ADAPTER_BIN="${MUSEHUB_PIPELINE_ADAPTER_BIN:-$PWD/target/debug/musehub-pipeline-reference}" \
    MUSEHUB_COMPOSE_ADAPTER_BIN="$MUSEHUB_COMPOSE_ADAPTER_BIN" \
    MUSEHUB_COMPOSE_RUNNER_BIN="$MUSEHUB_COMPOSE_RUNNER_BIN" \
    MUSEHUB_OCI_REGISTRY="${MUSEHUB_OCI_REGISTRY:-}" \
    MUSEHUB_REQUIRE_PIPELINE_OCI="${MUSEHUB_REQUIRE_PIPELINE_OCI:-0}" \
    MUSEHUB_COMPOSE_ADAPTER_MODE="${MUSEHUB_COMPOSE_ADAPTER_MODE:-mock}" \
    MUSEHUB_PIPELINE_EXECUTOR="${MUSEHUB_PIPELINE_EXECUTOR:-inprocess}" \
    MUSEHUB_AGENT_EXECUTOR="${MUSEHUB_AGENT_EXECUTOR:-inprocess}" \
    MUSEHUB_DEPLOY_EXECUTOR="${MUSEHUB_DEPLOY_EXECUTOR:-inprocess}" \
    MUSEHUB_INSTANCE_ID="${MUSEHUB_INSTANCE_ID:-$(cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen)}" \
    RUST_LOG="${RUST_LOG:-info}" \
    cargo run -p musehub-server >>"$SERVER_LOG" 2>&1
) &
echo $! >"$SERVER_PIDFILE"

for _ in $(seq 1 90); do
  if curl -sf http://127.0.0.1:8080/healthz >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
if ! curl -sf http://127.0.0.1:8080/healthz >/dev/null 2>&1; then
  echo "musehub-server failed to start" >&2
  tail -40 "$SERVER_LOG" | sed 's/password.*/password=REDACTED/g' >&2 || true
  exit 1
fi

if [[ "$PRODUCT_OCI" == "1" ]]; then
  command -v jq psql >/dev/null || { echo "jq and psql required for OCI web e2e runner" >&2; exit 1; }
  cargo build -q -p musehub-runnerd
  COOKIE=$(curl -sf -c - -X POST "http://127.0.0.1:8080/api/v1/auth/login" \
    -H 'Content-Type: application/json' \
    -d "{\"handle\":\"admin\",\"password\":\"$MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD\"}" | awk '/musehub_session/ {print $NF}')
  curl -sf -X POST "http://127.0.0.1:8080/api/v1/orgs/demo/members" \
    -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
    -d '{"handle":"e2e-oci-runner","password":"E2e-Oci-Runner-12!","role":"member"}' >/dev/null || true
  curl -sf -X POST "http://127.0.0.1:8080/api/v1/orgs/demo/repos" \
    -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
    -d '{"slug":"e2e-pat-anchor","description":"pat scope anchor"}' >/dev/null || true
  ANCHOR_REPO=$(psql "$DATABASE_URL" -Atc "SELECT id FROM repositories WHERE slug='e2e-pat-anchor'" 2>/dev/null || true)
  PAT_JSON=$(curl -sf -X POST "http://127.0.0.1:8080/api/v1/tokens" \
    -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
    -d "{\"name\":\"web-e2e-runner\",\"org\":\"demo\",\"scopes\":[\"pipeline:execute\",\"agent:execute\",\"deploy:execute\",\"checks:write\",\"repo:read\"],\"scope_mode\":\"all_accessible\",\"runner_handle\":\"e2e-oci-runner\"}")
  RUNNER_PAT=$(echo "$PAT_JSON" | jq -r .token)
  stop_owned "$RUNNER_PIDFILE"
  RUNNER_ENV=(
    MUSEHUB_API_BASE=http://127.0.0.1:8080
    MUSEHUB_DATABASE_URL="$DATABASE_URL"
    MUSEHUB_GIT_ROOT="$MUSEHUB_GIT_ROOT"
    MUSEHUB_ARTIFACT_ROOT="$MUSEHUB_ARTIFACT_ROOT"
    MUSEHUB_PIPELINE_ADAPTER_BIN="$MUSEHUB_PIPELINE_ADAPTER_BIN"
    MUSEHUB_REFERENCE_ADAPTER_BIN="$MUSEHUB_REFERENCE_ADAPTER_BIN"
    MUSEHUB_RUNNER_PAT="$RUNNER_PAT"
    MUSEHUB_RUNNER_WORK_KINDS=all
    MUSEHUB_RUNNER_POLL_SECS=1
    MUSEHUB_OCI_REGISTRY="${MUSEHUB_OCI_REGISTRY:-}"
    MUSEHUB_REQUIRE_PIPELINE_OCI="${MUSEHUB_REQUIRE_PIPELINE_OCI:-0}"
    MUSEHUB_COMPOSE_ADAPTER_MODE="${MUSEHUB_COMPOSE_ADAPTER_MODE:-runner}"
    MUSEHUB_COMPOSE_ADAPTER_BIN="$MUSEHUB_COMPOSE_RUNNER_BIN"
    MUSEHUB_COMPOSE_RUNNER_BIN="$MUSEHUB_COMPOSE_RUNNER_BIN"
  )
  _run_runnerd() {
    env "${RUNNER_ENV[@]}" "$PWD/target/debug/musehub-runnerd" >>"$SERVER_LOG" 2>&1
  }
  if groups | grep -qw docker 2>/dev/null; then
    ( _run_runnerd ) &
  else
    ( sg docker -c "cd '$PWD' && env $(printf '%q ' "${RUNNER_ENV[@]}") '$PWD/target/debug/musehub-runnerd'" >>"$SERVER_LOG" 2>&1 ) &
  fi
  echo $! >"$RUNNER_PIDFILE"
  sleep 2
  curl -sf "http://127.0.0.1:8080/api/v1/runner/capabilities" | jq -e '.pipeline_executor == "remote"' >/dev/null
  echo "web_e2e remote_pipeline_runner=ok anchor_repo=${ANCHOR_REPO:-none}"
fi

yarn --cwd web install
export MUSEHUB_E2E_BASE_URL=http://127.0.0.1:5173
export MUSEHUB_E2E_SERVER=http://127.0.0.1:8080
export MUSEHUB_E2E_API="${MUSEHUB_E2E_BASE_URL}"
yarn --cwd web playwright install chromium

export VITE_E2E_ISSUE_PAGE_LIMIT="${VITE_E2E_ISSUE_PAGE_LIMIT:-2}"
(
  env VITE_E2E_ISSUE_PAGE_LIMIT="$VITE_E2E_ISSUE_PAGE_LIMIT" \
    yarn --cwd web dev --host 127.0.0.1 --port 5173 >>"$SERVER_LOG" 2>&1
) &
echo $! >"$VITE_PIDFILE"
for _ in $(seq 1 60); do
  if curl -sf http://127.0.0.1:5173/ >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

export DATABASE_URL
export MUSEHUB_PRODUCT_OCI_E2E="${MUSEHUB_PRODUCT_OCI_E2E:-0}"
# Playwright is started only by this script (avoid reuseExistingServer picking a stale :5173).
export MUSEHUB_E2E_SKIP_WEB=1
if [[ -n "${PLAYWRIGHT_ARGS:-}" ]]; then
  # shellcheck disable=SC2086
  yarn --cwd web test:e2e ${PLAYWRIGHT_ARGS}
else
  yarn --cwd web test:e2e
fi
