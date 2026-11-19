#!/usr/bin/env bash
# Install the MuseHub storage mirror service and timer (root).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE=/etc/musehub/storage-mirror.env
UNIT_DIR=/etc/systemd/system
LIBEXEC_DIR=/usr/local/libexec/musehub
ENABLE_TIMER=0

usage() {
  printf 'Usage: sudo %s [--enable]\n' "$0"
  printf 'Install the mirror unit; --enable also enables and starts its timer.\n'
}

for arg in "$@"; do
  case "$arg" in
    --enable) ENABLE_TIMER=1 ;;
    -h|--help) usage; exit 0 ;;
    *)
      printf 'install_storage_mirror=fail unknown argument: %s\n' "$arg" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ "$(id -u)" -ne 0 ]]; then
  printf 'install_storage_mirror=fail run as root (sudo %s)\n' "$0" >&2
  exit 1
fi

for command_name in getent install stat systemctl; do
  command -v "$command_name" >/dev/null 2>&1 || {
    printf 'install_storage_mirror=fail required command not found: %s\n' "$command_name" >&2
    exit 1
  }
done

[[ -f "$ENV_FILE" && ! -L "$ENV_FILE" ]] || {
  printf 'install_storage_mirror=fail missing regular, non-symlink config: %s\n' "$ENV_FILE" >&2
  printf 'install_storage_mirror=next copy deploy/storage-mirror.env.example and configure it\n' >&2
  exit 1
}
config_owner="$(stat -c '%u' -- "$ENV_FILE")"
config_mode="$(stat -c '%a' -- "$ENV_FILE")"
[[ "$config_owner" == 0 ]] || {
  printf 'install_storage_mirror=fail config must be owned by root: %s\n' "$ENV_FILE" >&2
  exit 1
}
config_mode_value=$((8#$config_mode))
(( (config_mode_value & 0022) == 0 )) || {
  printf 'install_storage_mirror=fail config must not be group/world writable: %s\n' "$ENV_FILE" >&2
  exit 1
}

getent passwd musehub >/dev/null || {
  printf 'install_storage_mirror=fail required service user is missing: musehub\n' >&2
  exit 1
}
getent group musehub-git >/dev/null || {
  printf 'install_storage_mirror=fail required service group is missing: musehub-git\n' >&2
  exit 1
}

install -D -m 0755 "$ROOT/scripts/mirror-musehub-storage.sh" \
  "$LIBEXEC_DIR/mirror-musehub-storage.sh"
install -m 0644 "$ROOT/deploy/systemd/musehub-storage-mirror.service" \
  "$UNIT_DIR/musehub-storage-mirror.service"
install -m 0644 "$ROOT/deploy/systemd/musehub-storage-mirror.timer" \
  "$UNIT_DIR/musehub-storage-mirror.timer"
systemctl daemon-reload

if (( ENABLE_TIMER )); then
  systemctl enable --now musehub-storage-mirror.timer
  printf 'install_storage_mirror=ok timer=enabled-and-started\n'
else
  printf 'install_storage_mirror=ok timer=installed\n'
  printf 'install_storage_mirror=note timer enable state was not changed\n'
  printf 'install_storage_mirror=next (run on the primary only):\n'
  printf '  systemctl enable --now musehub-storage-mirror.timer\n'
fi
