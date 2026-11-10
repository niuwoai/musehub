#!/usr/bin/env bash
# Restore from backup-musehub-state.sh output (destructive to target paths/DB).
set -euo pipefail
cd "$(dirname "$0")/.."

: "${DATABASE_URL:?set DATABASE_URL}"
: "${MUSEHUB_GIT_ROOT:?set MUSEHUB_GIT_ROOT}"
MUSEHUB_ARTIFACT_ROOT="${MUSEHUB_ARTIFACT_ROOT:-/tmp/musehub-artifacts}"
BACKUP_DIR="${1:?usage: $0 <backup-dir>}"

run_pg() {
  local tool=$1
  shift
  if [[ -n "${MUSEHUB_PG_CLIENT_IMAGE:-}" ]]; then
    docker run --rm --network host -v "${BACKUP_DIR}:/backup:ro" \
      "$MUSEHUB_PG_CLIENT_IMAGE" "$tool" "$@"
  else
    command -v "$tool" >/dev/null || { echo "$tool required" >&2; exit 1; }
    "$tool" "$@"
  fi
}
MANIFEST="$BACKUP_DIR/manifest.json"
[[ -f "$MANIFEST" ]] || { echo "missing manifest.json" >&2; exit 1; }

DB_DUMP=$(python3 -c "import json; print(json.load(open('$MANIFEST'))['database_dump'])")
GIT_TAR=$(python3 -c "import json; print(json.load(open('$MANIFEST'))['git_archive'])")

echo "=== restoring database (drop/create public) ==="
run_pg psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -c "DROP SCHEMA public CASCADE; CREATE SCHEMA public;"
if [[ -n "${MUSEHUB_PG_CLIENT_IMAGE:-}" ]]; then
  run_pg pg_restore --no-owner --dbname="$DATABASE_URL" "/backup/$DB_DUMP"
else
  run_pg pg_restore --no-owner --dbname="$DATABASE_URL" "$BACKUP_DIR/$DB_DUMP"
fi

echo "=== restoring git tree ==="
rm -rf "$MUSEHUB_GIT_ROOT"
mkdir -p "$(dirname "$MUSEHUB_GIT_ROOT")"
tar -C "$(dirname "$MUSEHUB_GIT_ROOT")" -xzf "$BACKUP_DIR/$GIT_TAR"

ART_TAR=$(python3 -c "import json; d=json.load(open('$MANIFEST')); print(d.get('artifacts_archive',''))" 2>/dev/null || true)
if [[ -n "$ART_TAR" && -f "$BACKUP_DIR/$ART_TAR" ]]; then
  echo "=== restoring artifacts ==="
  rm -rf "$MUSEHUB_ARTIFACT_ROOT"
  mkdir -p "$(dirname "$MUSEHUB_ARTIFACT_ROOT")"
  tar -C "$(dirname "$MUSEHUB_ARTIFACT_ROOT")" -xzf "$BACKUP_DIR/$ART_TAR"
fi

echo "restore=ok"
