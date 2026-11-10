#!/usr/bin/env bash
# systemd helper: start/stop musehub-runner only when MUSEHUB_RUNNER_PAT is configured.
set -euo pipefail
ACTION="${1:?usage: $0 start|stop}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEPLOY_DIR="${MUSEHUB_DEPLOY_DIR:-$ROOT/deploy}"
cd "$DEPLOY_DIR"
[[ -f .env ]] || {
  echo "systemd_runner=fail missing $DEPLOY_DIR/.env" >&2
  exit 1
}

# shellcheck source=lib/production-compose.sh
source "$ROOT/scripts/lib/production-compose.sh"
runner_pat_configured() {
  local pat
  pat=$(grep -E '^MUSEHUB_RUNNER_PAT=' .env | cut -d= -f2- | tr -d '\r' || true)
  [[ -n "$pat" && "$pat" != placeholder-set-after-bootstrap ]]
}

case "$ACTION" in
  start)
    if ! runner_pat_configured; then
      echo "systemd_runner=skip MUSEHUB_RUNNER_PAT not set (run configure-production-runner-pat.sh)"
      exit 0
    fi
    production_compose up -d musehub-runner
    ;;
  stop)
    if ! runner_pat_configured; then
      echo "systemd_runner=skip stop (runner was not configured)"
      exit 0
    fi
    production_compose stop musehub-runner
    ;;
  *)
    echo "systemd_runner=fail unknown action $ACTION" >&2
    exit 1
    ;;
esac
