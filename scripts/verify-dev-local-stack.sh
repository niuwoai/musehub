#!/usr/bin/env bash
# Smoke: sed portable + dev stack init/start/status/stop/restart (Linux CI / local Postgres).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DATABASE_URL="${DATABASE_URL:-${MUSEHUB_TEST_DATABASE_URL:-}}"
: "${DATABASE_URL:?DATABASE_URL or MUSEHUB_TEST_DATABASE_URL required}"

chmod +x "$ROOT/scripts/dev-local-stack.sh"
chmod +x "$ROOT/scripts/verify-sed-portable.sh"

"$ROOT/scripts/verify-sed-portable.sh"

STATE="/tmp/musehub-dev-stack-verify-$$"
export MUSEHUB_DEV_STATE_DIR="$STATE"
trap 'test -x "$ROOT/scripts/dev-local-stack.sh" && "$ROOT/scripts/dev-local-stack.sh" stop 2>/dev/null || true; rm -rf "$STATE"' EXIT

export DATABASE_URL

"$ROOT/scripts/dev-local-stack.sh" init
"$ROOT/scripts/dev-local-stack.sh" start
curl -sf http://127.0.0.1:8080/readyz | jq -e '.ready == true' >/dev/null
"$ROOT/scripts/dev-local-stack.sh" status >/dev/null
"$ROOT/scripts/dev-local-stack.sh" stop
sleep 2
test ! -f "$STATE/pids/server.pid" || test ! kill -0 "$(cat "$STATE/pids/server.pid")" 2>/dev/null

"$ROOT/scripts/dev-local-stack.sh" start
curl -sf http://127.0.0.1:8080/readyz | jq -e '.checks.product_api == "ok"' >/dev/null
curl -sf http://127.0.0.1:5173/login | grep -q '<div id="root">'
"$ROOT/scripts/dev-local-stack.sh" stop

echo "verify_dev_local_stack=ok state_dir=$STATE"
