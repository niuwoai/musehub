#!/usr/bin/env bash
# Install MuseHub production systemd units (root). Paths are rendered from templates; secrets stay in deploy/.env.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEPLOY_DIR="${MUSEHUB_DEPLOY_DIR:-$ROOT/deploy}"
UNIT_DIR="${MUSEHUB_SYSTEMD_UNIT_DIR:-/etc/systemd/system}"

if [[ "$(id -u)" -ne 0 ]]; then
  echo "install_systemd=fail run as root (sudo $0)" >&2
  exit 1
fi
[[ -f "$DEPLOY_DIR/.env" ]] || {
  echo "install_systemd=fail missing $DEPLOY_DIR/.env (run install-production-compose.sh first)" >&2
  exit 1
}
command -v docker >/dev/null || {
  echo "install_systemd=fail docker not installed" >&2
  exit 1
}

render() {
  local src=$1 dest=$2
  sed -e "s|@REPO_ROOT@|${ROOT}|g" -e "s|@DEPLOY_DIR@|${DEPLOY_DIR}|g" "$src" >"$dest"
}

render "$ROOT/deploy/systemd/musehub-control-plane.service.in" \
  "$UNIT_DIR/musehub-control-plane.service"
render "$ROOT/deploy/systemd/musehub-runner.service.in" \
  "$UNIT_DIR/musehub-runner.service"

chmod +x "$ROOT/scripts/systemd-compose-control-plane.sh" \
  "$ROOT/scripts/systemd-compose-runner.sh"
systemctl daemon-reload
echo "install_systemd=ok units=$UNIT_DIR/musehub-control-plane.service,$UNIT_DIR/musehub-runner.service"
echo "install_systemd=next:"
echo "  systemctl enable --now musehub-control-plane.service"
echo "  systemctl enable musehub-runner.service   # starts worker only when PAT is configured"
