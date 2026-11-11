#!/usr/bin/env bash
# Smoke: dev stack init/start/stop/restart + permissions (Linux CI only; not macOS proof).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DATABASE_URL="${DATABASE_URL:-${MUSEHUB_TEST_DATABASE_URL:-}}"
: "${DATABASE_URL:?DATABASE_URL or MUSEHUB_TEST_DATABASE_URL required}"

chmod +x "$ROOT/scripts/dev-local-stack.sh" "$ROOT/scripts/verify-sed-portable.sh"
"$ROOT/scripts/verify-sed-portable.sh"

STATE="/tmp/musehub-dev-stack-verify-$$"
export MUSEHUB_DEV_STATE_DIR="$STATE"
cleanup_verify() {
  "$ROOT/scripts/dev-local-stack.sh" stop 2>/dev/null || true
  rm -rf "$STATE"
}
trap cleanup_verify EXIT

port_free() {
  local port=$1
  if command -v ss >/dev/null 2>&1; then
    ! ss -ltn 2>/dev/null | grep -q ":${port} "
    return
  fi
  if command -v lsof >/dev/null 2>&1; then
    ! lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1
    return
  fi
  return 1
}

assert_mode_600() {
  local f=$1
  local mode
  mode=$(stat -c '%a' "$f" 2>/dev/null || stat -f '%Lp' "$f")
  [[ "$mode" == "600" ]]
}

assert_stack_down() {
  port_free 8080
  port_free 5173
  ! [[ -f "$STATE/pids/server.pid" ]] || ! kill -0 "$(tr -d '[:space:]' <"$STATE/pids/server.pid")" 2>/dev/null
  ! [[ -f "$STATE/pids/vite.pid" ]] || ! kill -0 "$(tr -d '[:space:]' <"$STATE/pids/vite.pid")" 2>/dev/null
}

export DATABASE_URL

"$ROOT/scripts/dev-local-stack.sh" init
assert_mode_600 "$STATE/env"
assert_mode_600 "$STATE/secrets/bootstrap-admin.password"

MARKER="do_not_overwrite_$(date +%s)"
printf '\n# %s\n' "$MARKER" >>"$STATE/env"
BEFORE=$(wc -l <"$STATE/env")
"$ROOT/scripts/dev-local-stack.sh" init
AFTER=$(wc -l <"$STATE/env")
[[ "$BEFORE" -eq "$AFTER" ]]
grep -q "$MARKER" "$STATE/env"

"$ROOT/scripts/dev-local-stack.sh" start
curl -sf http://127.0.0.1:8080/readyz | jq -e '.ready == true' >/dev/null
curl -sf http://127.0.0.1:5173/login | grep -q 'id="root"'

"$ROOT/scripts/dev-local-stack.sh" stop
sleep 1
assert_stack_down

"$ROOT/scripts/dev-local-stack.sh" start
curl -sf http://127.0.0.1:8080/readyz | jq -e '.ready == true and .checks.product_api == "ok"' >/dev/null
curl -sf http://127.0.0.1:5173/login | grep -q 'id="root"'
"$ROOT/scripts/dev-local-stack.sh" stop
sleep 1
assert_stack_down

STATE_ALT="/tmp/musehub-dev-stack-verify-alt-$$"
export MUSEHUB_DEV_STATE_DIR="$STATE_ALT"
export MUSEHUB_BIND="127.0.0.1:18080"
export MUSEHUB_DEV_API_URL="http://127.0.0.1:18080"
"$ROOT/scripts/dev-local-stack.sh" init
set -a
# shellcheck disable=SC1090
source "$STATE_ALT/env"
set +a
[[ "${MUSEHUB_DEV_API_URL:-}" == "http://127.0.0.1:18080" ]]
"$ROOT/scripts/dev-local-stack.sh" start
curl -sf http://127.0.0.1:18080/readyz | jq -e '.ready == true' >/dev/null
"$ROOT/scripts/dev-local-stack.sh" stop
sleep 1
port_free 18080 || { rm -rf "$STATE_ALT"; exit 1; }
rm -rf "$STATE_ALT"

export MUSEHUB_DEV_STATE_DIR="$STATE"
unset MUSEHUB_BIND MUSEHUB_DEV_API_URL

echo "verify_dev_local_stack=ok platform=$(uname -s) state_dir=$STATE"
echo "verify_note=macOS_not_verified_on_this_runner"
