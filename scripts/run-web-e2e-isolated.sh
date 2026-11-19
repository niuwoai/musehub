#!/usr/bin/env bash
# One Playwright run with unique log/exit/meta (no shared /tmp/run-web-e2e-full.log).
set -euo pipefail
cd "$(dirname "$0")/.."

: "${MUSEHUB_TEST_DATABASE_URL:?set MUSEHUB_TEST_DATABASE_URL to the dedicated browser E2E database}"
if [[ "${MUSEHUB_ACCEPTANCE_ALLOW_DATABASE_RESET:-}" != "1" ]]; then
  echo "set MUSEHUB_ACCEPTANCE_ALLOW_DATABASE_RESET=1 to authorize the browser E2E migration down/up reset" >&2
  exit 1
fi
: "${MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD:?set MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD}"

SHA="$(git rev-parse --short HEAD)"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
RUN_ID="${SHA}-${STAMP}"
TMP_ROOT="${MUSEHUB_E2E_TMP_ROOT:-/tmp}"
mkdir -p "$TMP_ROOT"
TMP_ROOT="$(cd "$TMP_ROOT" && pwd -P)"
RUN_DIR="${TMP_ROOT%/}/musehub-web-e2e-runs/${RUN_ID}"
mkdir -p "$(dirname "$RUN_DIR")"
mkdir "$RUN_DIR"

META="$RUN_DIR/meta.env"
LOG="$RUN_DIR/playwright.log"
EXIT="$RUN_DIR/exit.code"
PIDFILE="$RUN_DIR/wrapper.pid"

{
  echo "git_sha=$(git rev-parse HEAD)"
  echo "git_sha_short=$SHA"
  echo "started_at_utc=$STAMP"
  echo "log_file=$LOG"
  echo "exit_file=$EXIT"
  echo "playwright_args=${PLAYWRIGHT_ARGS:-<all>}"
} >"$META"

export TMPDIR="$RUN_DIR/tmp"
mkdir -p "$TMPDIR"
export MUSEHUB_GIT_ROOT="$RUN_DIR/git"
export MUSEHUB_SESSION_SECRET="${MUSEHUB_SESSION_SECRET:-ci-web-e2e-session-secret-32chars}"
export MUSEHUB_ARTIFACT_ROOT="$RUN_DIR/artifacts"
export MUSEHUB_E2E_PIDFILE="$RUN_DIR/server.pid"
export MUSEHUB_E2E_SERVER_LOG="$RUN_DIR/server.log"
export MUSEHUB_E2E_VITE_PID="$RUN_DIR/vite.pid"
export MUSEHUB_E2E_RUNNER_PID="$RUN_DIR/runner.pid"
export MUSEHUB_E2E_ENABLE_DOCKER="${MUSEHUB_E2E_ENABLE_DOCKER:-0}"

echo "e2e_run_id=$RUN_ID meta=$META log=$LOG"
echo $$ >"$PIDFILE"

set +o histexpand 2>/dev/null || true
set +e
./scripts/run-web-e2e.sh >"$LOG" 2>&1
code=$?
set -e
echo "$code" >"$EXIT"
echo "e2e_finished pid=$$ exit=$code run_id=$RUN_ID"
exit "$code"
