#!/usr/bin/env bash
# One Playwright run with unique log/exit/meta (no shared /tmp/run-web-e2e-full.log).
set -euo pipefail
cd "$(dirname "$0")/.."

SHA="$(git rev-parse --short HEAD)"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
RUN_ID="${SHA}-${STAMP}"
RUN_DIR="/tmp/musehub-web-e2e-runs/${RUN_ID}"
mkdir -p "$RUN_DIR"

META="$RUN_DIR/meta.env"
LOG="$RUN_DIR/playwright.log"
EXIT="$RUN_DIR/exit.code"
PIDFILE="$RUN_DIR/runner.pid"

{
  echo "git_sha=$(git rev-parse HEAD)"
  echo "git_sha_short=$SHA"
  echo "started_at_utc=$STAMP"
  echo "log_file=$LOG"
  echo "exit_file=$EXIT"
  echo "playwright_args=${PLAYWRIGHT_ARGS:-<all>}"
} >"$META"

export MUSEHUB_TEST_DATABASE_URL="${MUSEHUB_TEST_DATABASE_URL:-${DATABASE_URL:-}}"
export MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD="${MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD:-Ci-Web-E2E-Strong!2026}"
export MUSEHUB_GIT_ROOT="${MUSEHUB_GIT_ROOT:-/tmp/musehub-web-e2e-ci-git}"
export MUSEHUB_SESSION_SECRET="${MUSEHUB_SESSION_SECRET:-ci-web-e2e-session-secret-32chars}"
export MUSEHUB_ARTIFACT_ROOT="${MUSEHUB_ARTIFACT_ROOT:-/tmp/musehub-web-e2e-artifacts}"

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
