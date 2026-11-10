#!/usr/bin/env bash
# Static validation for MuseHub systemd units (no host reboot required).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# Use the real checkout paths so systemd-analyze can resolve ExecStart when available.
VERIFY_REPO="$ROOT"
VERIFY_DEPLOY="$ROOT/deploy"

render() {
  sed -e "s|@REPO_ROOT@|${VERIFY_REPO}|g" -e "s|@DEPLOY_DIR@|${VERIFY_DEPLOY}|g" "$1"
}

render "$ROOT/deploy/systemd/musehub-control-plane.service.in" \
  >"$TMP/musehub-control-plane.service"
render "$ROOT/deploy/systemd/musehub-runner.service.in" >"$TMP/musehub-runner.service"

for unit in "$TMP"/*.service; do
  grep -q 'After=.*docker\.service' "$unit" || {
    echo "systemd_verify=fail missing After=docker.service in $unit" >&2
    exit 1
  }
  if grep -qE '^(Environment=.*(POSTGRES_PASSWORD|MUSEHUB_SESSION_SECRET|MUSEHUB_RUNNER_PAT)|Password=)' "$unit"; then
    echo "systemd_verify=fail secret-like directive in $unit" >&2
    exit 1
  fi
done

bash -n "$ROOT/scripts/systemd-compose-control-plane.sh"
bash -n "$ROOT/scripts/systemd-compose-runner.sh"
bash -n "$ROOT/scripts/install-musehub-systemd.sh"

if command -v systemd-analyze >/dev/null 2>&1; then
  systemd-analyze verify "$TMP/musehub-control-plane.service" "$TMP/musehub-runner.service"
  echo "systemd_verify=systemd-analyze ok"
else
  grep -q "ExecStart=${VERIFY_REPO}/scripts/systemd-compose-control-plane.sh start" \
    "$TMP/musehub-control-plane.service"
  grep -q "ExecStart=${VERIFY_REPO}/scripts/systemd-compose-runner.sh start" \
    "$TMP/musehub-runner.service"
  echo "systemd_verify=fallback layout ok (systemd-analyze not installed)"
fi
