#!/usr/bin/env bash
# Snapshot PostgreSQL + bare Git repos + artifact root into a portable directory.
set -euo pipefail
cd "$(dirname "$0")/.."

: "${DATABASE_URL:?set DATABASE_URL}"
: "${MUSEHUB_GIT_ROOT:?set MUSEHUB_GIT_ROOT}"
MUSEHUB_ARTIFACT_ROOT="${MUSEHUB_ARTIFACT_ROOT:-/tmp/musehub-artifacts}"
BACKUP_DIR="${1:?usage: $0 <output-dir>}"

command -v pg_dump >/dev/null || { echo "pg_dump required" >&2; exit 1; }

mkdir -p "$BACKUP_DIR"
TS=$(date -u +%Y%m%dT%H%M%SZ)
APP_VERSION=$(tr -d ' \n' < VERSION)

echo "=== dumping database ==="
pg_dump --no-owner --format=custom --file="$BACKUP_DIR/musehub-${TS}.pgdump" "$DATABASE_URL"

echo "=== archiving git + artifacts ==="
tar -C "$(dirname "$MUSEHUB_GIT_ROOT")" -czf "$BACKUP_DIR/git-${TS}.tar.gz" "$(basename "$MUSEHUB_GIT_ROOT")"
if [[ -d "$MUSEHUB_ARTIFACT_ROOT" ]]; then
  tar -C "$(dirname "$MUSEHUB_ARTIFACT_ROOT")" -czf "$BACKUP_DIR/artifacts-${TS}.tar.gz" "$(basename "$MUSEHUB_ARTIFACT_ROOT")"
else
  echo "warn: artifact root missing ($MUSEHUB_ARTIFACT_ROOT), skipping" >&2
fi

cat >"$BACKUP_DIR/manifest.json" <<EOF
{
  "created_at": "$TS",
  "app_version": "$APP_VERSION",
  "database_dump": "musehub-${TS}.pgdump",
  "git_archive": "git-${TS}.tar.gz",
  "artifacts_archive": "artifacts-${TS}.tar.gz"
}
EOF

echo "backup=ok dir=$BACKUP_DIR"
