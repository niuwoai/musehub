#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export DATABASE_URL="${MUSEHUB_TEST_DATABASE_URL:-}"
: "${DATABASE_URL:?set MUSEHUB_TEST_DATABASE_URL to the dedicated browser E2E database}"
if [[ "${MUSEHUB_ACCEPTANCE_ALLOW_DATABASE_RESET:-}" != "1" ]]; then
  echo "set MUSEHUB_ACCEPTANCE_ALLOW_DATABASE_RESET=1 to authorize the browser E2E migration down/up reset" >&2
  exit 1
fi
export MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD="${MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD:-Config-Grants-E2E-Strong-1!}"
port_in_use() {
  local port=$1
  if command -v lsof >/dev/null 2>&1; then
    lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1
  elif command -v ss >/dev/null 2>&1; then
    ss -ltn "sport = :$port" 2>/dev/null | grep -q .
  else
    echo "port_check_unavailable port=$port need lsof or ss" >&2
    return 2
  fi
}

for port in 8080 5173; do
  if port_in_use "$port"; then
    echo "port_in_use port=$port; refusing to migrate or stop an existing service" >&2
    exit 1
  else
    status=$?
    if [[ "$status" -ne 1 ]]; then
      echo "port_check_failed port=$port status=$status" >&2
      exit 1
    fi
  fi
done

export MUSEHUB_GIT_ROOT="${MUSEHUB_GIT_ROOT:-$(mktemp -d)}"
export MUSEHUB_SESSION_SECRET="${MUSEHUB_SESSION_SECRET:-$(openssl rand -hex 32)}"
export MUSEHUB_E2E_API=http://127.0.0.1:8080
export MUSEHUB_E2E_BASE_URL=http://127.0.0.1:5173

cargo run -q -p musehub-migrate -- down migrations "$DATABASE_URL" >/dev/null || true
cargo run -q -p musehub-migrate -- up migrations "$DATABASE_URL" >/dev/null
cargo build -q -p musehub-server

SERVER_PID=""
VITE_PID=""
cleanup() {
  [[ -n "$VITE_PID" ]] && kill "$VITE_PID" 2>/dev/null || true
  [[ -n "$SERVER_PID" ]] && kill "$SERVER_PID" 2>/dev/null || true
}
trap cleanup EXIT

(
  DATABASE_URL="$DATABASE_URL" MUSEHUB_GIT_ROOT="$MUSEHUB_GIT_ROOT" \
  MUSEHUB_SESSION_SECRET="$MUSEHUB_SESSION_SECRET" \
  MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD="$MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD" \
  MUSEHUB_BIND=127.0.0.1:8080 \
  cargo run -q -p musehub-server
) &
SERVER_PID=$!
for _ in $(seq 1 90); do curl -sf http://127.0.0.1:8080/healthz >/dev/null && break; sleep 1; done

yarn --cwd web install --frozen-lockfile 2>/dev/null || yarn --cwd web install
yarn --cwd web playwright install chromium
(yarn --cwd web dev --host 127.0.0.1 --port 5173 >/tmp/vite-config-grants.log 2>&1) &
VITE_PID=$!
for _ in $(seq 1 60); do curl -sf http://127.0.0.1:5173/ >/dev/null && break; sleep 1; done

yarn --cwd web playwright test e2e/config-grants.spec.ts
echo "config_grants_playwright=ok"
