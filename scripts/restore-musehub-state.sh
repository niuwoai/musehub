#!/usr/bin/env bash
# Restore a backup into a fresh/empty database and fresh storage paths.
set -euo pipefail
cd "$(dirname "$0")/.."

: "${DATABASE_URL:?set DATABASE_URL}"
: "${MUSEHUB_GIT_ROOT:?set MUSEHUB_GIT_ROOT}"
[[ "${MUSEHUB_RESTORE_ALLOW_DESTRUCTIVE:-}" == "1" ]] || {
  echo "set MUSEHUB_RESTORE_ALLOW_DESTRUCTIVE=1 to authorize replacing the target database" >&2
  exit 1
}
MUSEHUB_ARTIFACT_ROOT="${MUSEHUB_ARTIFACT_ROOT:-/tmp/musehub-artifacts}"
BACKUP_DIR="${1:?usage: $0 <backup-dir>}"

for target in "$MUSEHUB_GIT_ROOT" "$MUSEHUB_ARTIFACT_ROOT"; do
  if [[ -e "$target" ]]; then
    [[ -d "$target" && -z "$(find "$target" -mindepth 1 -maxdepth 1 -print -quit)" ]] || {
      echo "refusing restore: storage target must not exist or must be empty: $target" >&2
      exit 1
    }
    rmdir "$target"
  fi
done

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
DB_DUMP=$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["database_dump"])' "$MANIFEST")
GIT_TAR=$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["git_archive"])' "$MANIFEST")
ART_TAR=$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1])).get("artifacts_archive", ""))' "$MANIFEST")
for archive in "$DB_DUMP" "$GIT_TAR" ${ART_TAR:+"$ART_TAR"}; do
  [[ "$archive" != */* && "$archive" != *..* && -f "$BACKUP_DIR/$archive" ]] || {
    echo "invalid or missing backup member: $archive" >&2
    exit 1
  }
done

validate_tree_archive() {
  python3 -I -c 'import pathlib,sys,tarfile; archive,root=sys.argv[1:]; tf=tarfile.open(archive,"r:gz"); members=tf.getmembers(); bad=[m.name for m in members if pathlib.PurePosixPath(m.name).is_absolute() or ".." in pathlib.PurePosixPath(m.name).parts or not pathlib.PurePosixPath(m.name).parts or pathlib.PurePosixPath(m.name).parts[0] != root or not (m.isdir() or m.isfile())]; tf.close(); sys.exit((print("unsafe or unexpected archive member: " + ", ".join(bad[:5]), file=sys.stderr),1)[1] if bad else 0)' "$1" "$2"
}
validate_tree_archive "$BACKUP_DIR/$GIT_TAR" "$(basename "$MUSEHUB_GIT_ROOT")"
if [[ -n "$ART_TAR" ]]; then
  validate_tree_archive "$BACKUP_DIR/$ART_TAR" "$(basename "$MUSEHUB_ARTIFACT_ROOT")"
fi
if [[ -n "${MUSEHUB_PG_CLIENT_IMAGE:-}" ]]; then
  run_pg pg_restore --list "/backup/$DB_DUMP" >/dev/null
else
  run_pg pg_restore --list "$BACKUP_DIR/$DB_DUMP" >/dev/null
fi

echo "=== restoring database (drop/create public) ==="
run_pg psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -c "DROP SCHEMA public CASCADE; CREATE SCHEMA public;"
if [[ -n "${MUSEHUB_PG_CLIENT_IMAGE:-}" ]]; then
  run_pg pg_restore --no-owner --dbname="$DATABASE_URL" "/backup/$DB_DUMP"
else
  run_pg pg_restore --no-owner --dbname="$DATABASE_URL" "$BACKUP_DIR/$DB_DUMP"
fi

echo "=== restoring git tree ==="
mkdir -p "$(dirname "$MUSEHUB_GIT_ROOT")"
tar -C "$(dirname "$MUSEHUB_GIT_ROOT")" -xzf "$BACKUP_DIR/$GIT_TAR"
if [[ -n "$ART_TAR" ]]; then
  echo "=== restoring artifacts ==="
  mkdir -p "$(dirname "$MUSEHUB_ARTIFACT_ROOT")"
  tar -C "$(dirname "$MUSEHUB_ARTIFACT_ROOT")" -xzf "$BACKUP_DIR/$ART_TAR"
fi

echo "restore=ok"
