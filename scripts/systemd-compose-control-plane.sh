#!/usr/bin/env bash
# systemd helper: start/stop production control plane (no secrets in unit files).
set -euo pipefail
ACTION="${1:?usage: $0 start|stop}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEPLOY_DIR="${MUSEHUB_DEPLOY_DIR:-$ROOT/deploy}"
cd "$DEPLOY_DIR"
[[ -f .env ]] || {
  echo "systemd_control_plane=fail missing $DEPLOY_DIR/.env" >&2
  exit 1
}

COMPOSE=(docker compose -f docker-compose.yml -f docker-compose.production.yml --env-file .env)
case "$ACTION" in
  start)
    "${COMPOSE[@]}" up -d postgres musehub-server gateway
    ;;
  stop)
    "${COMPOSE[@]}" stop postgres musehub-server gateway
    ;;
  *)
    echo "systemd_control_plane=fail unknown action $ACTION" >&2
    exit 1
    ;;
esac
