#!/usr/bin/env bash
# Browser E2E: musehub-server + Vite + Playwright (requires Postgres bootstrap env).
set -euo pipefail
cd "$(dirname "$0")/.."

DATABASE_URL="${MUSEHUB_TEST_DATABASE_URL:-${DATABASE_URL:-}}"
: "${DATABASE_URL:?DATABASE_URL or MUSEHUB_TEST_DATABASE_URL required}"
: "${MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD:?set MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD}"

export MUSEHUB_GIT_ROOT="${MUSEHUB_GIT_ROOT:-/tmp/musehub-e2e-git}"
export MUSEHUB_SESSION_SECRET="${MUSEHUB_SESSION_SECRET:-e2e-session-secret-32chars-min}"
mkdir -p "$MUSEHUB_GIT_ROOT"

SERVER_PIDFILE="${MUSEHUB_E2E_PIDFILE:-/tmp/musehub-e2e-server.pid}"
SERVER_LOG="${MUSEHUB_E2E_SERVER_LOG:-/tmp/musehub-e2e-server.log}"
VITE_PIDFILE="${MUSEHUB_E2E_VITE_PID:-/tmp/musehub-e2e-vite.pid}"

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
if command -v docker >/dev/null && docker info >/dev/null 2>&1; then
  chmod +x ./scripts/start-ci-registry.sh
  if ./scripts/start-ci-registry.sh; then
    export MUSEHUB_OCI_REGISTRY="${MUSEHUB_OCI_REGISTRY:-127.0.0.1:5000}"
    export MUSEHUB_REQUIRE_PIPELINE_OCI=1
    export MUSEHUB_COMPOSE_ADAPTER_MODE=runner
    export MUSEHUB_PRODUCT_OCI_E2E=1
    MUSEHUB_PIPELINE_ADAPTER_BIN="$PWD/target/debug/musehub-pipeline-oci"
    PRODUCT_OCI=1
  fi
fi
echo "web_e2e product_oci=$PRODUCT_OCI registry=${MUSEHUB_OCI_REGISTRY:-none}"
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

yarn --cwd web install
export MUSEHUB_E2E_API=http://127.0.0.1:8080
export MUSEHUB_E2E_BASE_URL=http://127.0.0.1:5173
yarn --cwd web playwright install chromium

(
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
yarn --cwd web test:e2e
