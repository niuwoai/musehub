#!/usr/bin/env bash
# Negative cases: foreign pidfile must not be killed; start rejects foreign pid (needs DATABASE_URL).
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

SLEEP_PID=
cleanup_neg() {
  kill "${SLEEP_PID:-}" 2>/dev/null || true
  if [[ -n "${MUSEHUB_DEV_STATE_DIR:-}" ]]; then
    "$ROOT/scripts/dev-local-stack.sh" stop 2>/dev/null || true
  fi
  rm -rf "$STATE"
}
trap cleanup_neg EXIT

sleep 300 &
SLEEP_PID=$!

printf '%s\n' "$SLEEP_PID" >"$SERVER_PID"
printf 'pid=%s\npattern=wrong\n' "$SLEEP_PID" >"$SERVER_MARKER"

if dev_local_slot_stop "$SERVER_PID" "$SERVER_MARKER" "musehub-server" server 2>/dev/null; then
  echo "verify_negative=fail expected stop refusal" >&2
  exit 1
fi
kill -0 "$SLEEP_PID" 2>/dev/null || { echo "verify_negative=fail sleep killed" >&2; exit 1; }
[[ -f "$SERVER_PID" ]] || { echo "verify_negative=fail pidfile removed" >&2; exit 1; }

rm -f "$SERVER_PID" "$SERVER_MARKER"
kill "$SLEEP_PID" 2>/dev/null || true
SLEEP_PID=
sleep 300 &
SLEEP_PID=$!
printf '%s\n' "$SLEEP_PID" >"$SERVER_PID"
printf 'pid=%s\npattern=musehub-server\n' "$SLEEP_PID" >"$SERVER_MARKER"

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
"$ROOT/scripts/dev-local-stack.sh" stop 2>/dev/null || true

echo "verify_dev_local_stack_negative=ok"
