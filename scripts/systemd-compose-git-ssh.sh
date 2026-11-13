#!/usr/bin/env bash
# systemd helper: start/stop product Git-over-SSH Compose service.
set -euo pipefail
ACTION="${1:?usage: $0 start|stop}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEPLOY_DIR="${MUSEHUB_DEPLOY_DIR:-$ROOT/deploy}"
cd "$DEPLOY_DIR"
[[ -f .env ]] || {
  echo "systemd_git_ssh=fail missing $DEPLOY_DIR/.env" >&2
  exit 1
}

# shellcheck source=lib/production-compose.sh
source "$ROOT/scripts/lib/production-compose.sh"
case "$ACTION" in
  start)
    production_compose up -d musehub-git-ssh
    ;;
  stop)
    production_compose stop musehub-git-ssh
    ;;
  *)
    echo "systemd_git_ssh=fail unknown action $ACTION" >&2
    exit 1
    ;;
esac
