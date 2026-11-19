#!/usr/bin/env bash
# Copy one completed MuseHub backup to immutable off-host object storage and verify it.
set -euo pipefail

usage() {
  printf 'Usage: %s <backup-dir> <rclone-remote:path> <snapshot-id>\n' "$0" >&2
  printf 'Requires rclone and a configured remote (for example, s3:bucket/musehub).\n' >&2
  exit 2
}

[[ $# -eq 3 ]] || usage
BACKUP_DIR="$1"
REMOTE_ROOT="$2"
SNAPSHOT_ID="$3"

[[ -d "$BACKUP_DIR" ]] || { echo "backup directory does not exist" >&2; exit 1; }
[[ -f "$BACKUP_DIR/manifest.json" ]] || { echo "backup manifest is missing" >&2; exit 1; }
[[ "$SNAPSHOT_ID" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$ ]] || usage
[[ "$REMOTE_ROOT" != *$'\n'* && "$REMOTE_ROOT" != *$'\r'* ]] || usage
command -v rclone >/dev/null 2>&1 || { echo "rclone is required" >&2; exit 1; }

FILE_COUNT="$(find "$BACKUP_DIR" -maxdepth 1 -type f | wc -l | tr -d ' ')"
(( FILE_COUNT >= 2 )) || { echo "backup must contain a manifest and at least one data file" >&2; exit 1; }

DESTINATION="${REMOTE_ROOT%/}/${SNAPSHOT_ID}"
echo "=== copying immutable backup snapshot to object storage ==="
rclone copy "$BACKUP_DIR" "$DESTINATION" --immutable --create-empty-src-dirs

echo "=== verifying remote snapshot by downloading and comparing bytes ==="
rclone check "$BACKUP_DIR" "$DESTINATION" --download
printf 'backup_replication=ok snapshot=%s destination=%s files=%s\n' \
  "$SNAPSHOT_ID" "$DESTINATION" "$FILE_COUNT"
