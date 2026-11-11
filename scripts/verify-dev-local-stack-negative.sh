#!/usr/bin/env bash
# Negative cases: foreign pidfile, forged epoch, start/stop via real script, vite-fail cleanup.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=lib/dev-local-pid.sh
source "$ROOT/scripts/lib/dev-local-pid.sh"

DATABASE_URL="${DATABASE_URL:-${MUSEHUB_TEST_DATABASE_URL:-}}"
: "${DATABASE_URL:?DATABASE_URL or MUSEHUB_TEST_DATABASE_URL required}"

STATE="/tmp/musehub-dev-stack-neg-$$"
mkdir -p "$STATE/pids"
SERVER_PID="$STATE/pids/server.pid"
SERVER_MARKER="$STATE/pids/server.marker"

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

SLEEP_PID=
cleanup_neg() {
  kill "${SLEEP_PID:-}" 2>/dev/null || true
  if [[ -n "${MUSEHUB_DEV_STATE_DIR:-}" ]]; then
    MUSEHUB_DEV_STACK_FORCE_VITE_FAIL=0 "$ROOT/scripts/dev-local-stack.sh" stop 2>/dev/null || true
  fi
  rm -rf "$STATE"
}
trap cleanup_neg EXIT

sleep 300 &
SLEEP_PID=$!

printf '%s\n' "$SLEEP_PID" >"$SERVER_PID"
printf 'pid=%s\npattern=wrong\n' "$SLEEP_PID" >"$SERVER_MARKER"

if dev_local_slot_stop "$SERVER_PID" "$SERVER_MARKER" "musehub-server" server 2>/dev/null; then
  echo "verify_negative=fail expected helper stop refusal" >&2
  exit 1
fi
set +e
MUSEHUB_DEV_STATE_DIR="$STATE" "$ROOT/scripts/dev-local-stack.sh" stop 2>"$STATE/stop1.err"
stop_code=$?
set -e
[[ "$stop_code" -ne 0 ]] || { echo "verify_negative=fail script stop should refuse foreign slot" >&2; exit 1; }
grep -q 'pidfile_identity_mismatch' "$STATE/stop1.err"
kill -0 "$SLEEP_PID" 2>/dev/null || { echo "verify_negative=fail sleep killed" >&2; exit 1; }
[[ -f "$SERVER_PID" ]] || { echo "verify_negative=fail pidfile removed" >&2; exit 1; }

rm -f "$SERVER_PID" "$SERVER_MARKER"
kill "$SLEEP_PID" 2>/dev/null || true
SLEEP_PID=
sleep 300 &
SLEEP_PID=$!
printf '%s\n' "$SLEEP_PID" >"$SERVER_PID"
printf 'pid=%s\npattern=musehub-server\nstarted_epoch=1\n' "$SLEEP_PID" >"$SERVER_MARKER"

export MUSEHUB_DEV_STATE_DIR="$STATE"
export DATABASE_URL
chmod +x "$ROOT/scripts/dev-local-stack.sh"
"$ROOT/scripts/dev-local-stack.sh" init >/dev/null

set +e
"$ROOT/scripts/dev-local-stack.sh" start 2>"$STATE/start.err"
code=$?
set -e
[[ "$code" -ne 0 ]] || { echo "verify_negative=fail start should reject foreign pid" >&2; exit 1; }
grep -qE 'pidfile_foreign_process|pidfile_identity_mismatch' "$STATE/start.err"

kill "${SLEEP_PID:-}" 2>/dev/null || true
SLEEP_PID=
rm -f "$SERVER_PID" "$SERVER_MARKER"

"$ROOT/scripts/dev-local-stack.sh" init >/dev/null
"$ROOT/scripts/dev-local-stack.sh" start >/dev/null
server_pid=$(tr -d '[:space:]' <"$SERVER_PID")
kill -0 "$server_pid"
cp "$SERVER_MARKER" "$STATE/marker.bak"
grep -v '^started_epoch=' "$SERVER_MARKER" >"$STATE/marker.corrupt"
printf 'started_epoch=1\n' >>"$STATE/marker.corrupt"
mv "$STATE/marker.corrupt" "$SERVER_MARKER"
set +e
"$ROOT/scripts/dev-local-stack.sh" stop 2>"$STATE/stop2.err"
stop_code=$?
set -e
[[ "$stop_code" -ne 0 ]] || { echo "verify_negative=fail forged epoch stop must fail" >&2; exit 1; }
grep -q 'pidfile_identity_mismatch' "$STATE/stop2.err"
kill -0 "$server_pid" || { echo "verify_negative=fail server died on forged stop" >&2; exit 1; }
mv "$STATE/marker.bak" "$SERVER_MARKER"
"$ROOT/scripts/dev-local-stack.sh" stop >/dev/null
sleep 1
port_free 8080 || { echo "verify_negative=fail port 8080 still busy" >&2; exit 1; }

STATE_VF="/tmp/musehub-dev-stack-vitefail-$$"
mkdir -p "$STATE_VF"
export MUSEHUB_DEV_STATE_DIR="$STATE_VF"
export DATABASE_URL
printf '%s\n' '#!/bin/sh' 'exit 1' >"$STATE_VF/vite-fail.sh"
chmod +x "$STATE_VF/vite-fail.sh"
export MUSEHUB_DEV_STACK_VITE_BIN="$STATE_VF/vite-fail.sh"
"$ROOT/scripts/dev-local-stack.sh" init >/dev/null
set +e
"$ROOT/scripts/dev-local-stack.sh" start 2>"$STATE/vitefail.err"
vf_code=$?
set -e
[[ "$vf_code" -ne 0 ]] || { echo "verify_negative=fail vite launch should fail" >&2; exit 1; }
grep -qE 'vite_login_unreachable|exec_ready_process_exit|exec_ready_timeout' "$STATE/vitefail.err"
sleep 1
port_free 8080 || { echo "verify_negative=fail server left on 8080 after vite fail" >&2; exit 1; }
[[ ! -f "$STATE_VF/pids/server.pid" ]] || ! kill -0 "$(tr -d '[:space:]' <"$STATE_VF/pids/server.pid")" 2>/dev/null || {
  echo "verify_negative=fail server pid still alive after vite fail" >&2
  exit 1
}
rm -rf "$STATE_VF"
unset MUSEHUB_DEV_STACK_VITE_BIN

STATE_YF="/tmp/musehub-dev-stack-yarnfail-$$"
mkdir -p "$STATE_YF"
export MUSEHUB_DEV_STATE_DIR="$STATE_YF"
export MUSEHUB_DEV_STACK_YARN_CMD=/bin/false
export MUSEHUB_DEV_STACK_TEST_YARN=1
"$ROOT/scripts/dev-local-stack.sh" init >/dev/null
set +e
"$ROOT/scripts/dev-local-stack.sh" start 2>"$STATE/yarnfail.err"
yf_code=$?
set -e
[[ "$yf_code" -ne 0 ]] || { echo "verify_negative=fail yarn should fail" >&2; exit 1; }
grep -q 'yarn_install_failed' "$STATE/yarnfail.err"
sleep 1
port_free 8080 || { echo "verify_negative=fail server after yarn fail" >&2; exit 1; }
rm -rf "$STATE_YF"
unset MUSEHUB_DEV_STACK_YARN_CMD MUSEHUB_DEV_STACK_TEST_YARN

export MUSEHUB_DEV_STATE_DIR="$STATE"

echo "verify_dev_local_stack_negative=ok"
