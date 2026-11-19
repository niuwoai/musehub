#!/usr/bin/env bash
# Destructive restore acceptance; restricted to a dedicated disposable database and temp tree.
set -euo pipefail
cd "$(dirname "$0")/.."

DATABASE_URL="${MUSEHUB_TEST_DATABASE_URL:-}"
: "${DATABASE_URL:?set MUSEHUB_TEST_DATABASE_URL to the dedicated musehub_backup_restore database}"
[[ "${MUSEHUB_BACKUP_RESTORE_ALLOW_RESET:-}" == "1" ]] || {
  echo "set MUSEHUB_BACKUP_RESTORE_ALLOW_RESET=1 to confirm the dedicated acceptance database may be reset" >&2
  exit 1
}
DB_HOST=$(python3 -I -c 'import sys, urllib.parse; print(urllib.parse.urlparse(sys.argv[1]).hostname or "")' "$DATABASE_URL")
DB_NAME=$(python3 -I -c 'import sys, urllib.parse; print(urllib.parse.urlparse(sys.argv[1]).path.lstrip("/"))' "$DATABASE_URL")
[[ "$DB_HOST" == "localhost" || "$DB_HOST" == "127.0.0.1" || "$DB_HOST" == "::1" ]] || {
  echo "refusing remote database host: ${DB_HOST:-<empty>}" >&2
  exit 1
}
[[ "$DB_NAME" == "musehub_backup_restore" ]] || {
  echo "refusing database reset: expected database musehub_backup_restore, got ${DB_NAME:-<empty>}" >&2
  exit 1
}

command -v rclone >/dev/null 2>&1 || { echo "rclone is required for backup replication acceptance" >&2; exit 1; }

WORK_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/musehub-backup-restore.XXXXXX")
trap 'rm -rf "$WORK_ROOT"' EXIT
export DATABASE_URL
export MUSEHUB_SESSION_SECRET="${MUSEHUB_SESSION_SECRET:-$(python3 -I -c 'import secrets; print(secrets.token_urlsafe(32))')}"
export MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD="${MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD:-$(python3 -I -c 'import secrets; print(secrets.token_urlsafe(24))')}"
export MUSEHUB_GIT_ROOT="$WORK_ROOT/git"
export MUSEHUB_ARTIFACT_ROOT="$WORK_ROOT/artifacts"
export MUSEHUB_RESTORE_ALLOW_DESTRUCTIVE=1
BACKUP_DIR="$WORK_ROOT/backup"
OBJECT_STORE="$WORK_ROOT/object-store"
SNAPSHOT_ID="acceptance-$(date -u +%Y%m%dT%H%M%SZ)"
REMOTE_SNAPSHOT="$OBJECT_STORE/$SNAPSHOT_ID"
MATERIALIZED_BACKUP="$WORK_ROOT/materialized-backup"
mkdir -p "$MUSEHUB_GIT_ROOT" "$MUSEHUB_ARTIFACT_ROOT" "$BACKUP_DIR" "$OBJECT_STORE"

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

cargo run -p musehub-migrate -- up migrations "$DATABASE_URL"
export MUSEHUB_TEST_DATABASE_URL="$DATABASE_URL"
cargo test -p musehub-app --test security_db -- --test-threads=1 >/dev/null
./scripts/seed-backup-git-fixture.sh

STORAGE_KEY=$(psql "$DATABASE_URL" -At -c \
  "SELECT storage_key FROM repositories WHERE slug_normalized = 'backup-fixture' LIMIT 1")
[[ -n "$STORAGE_KEY" ]] || { echo "no repository row after seed" >&2; exit 1; }
BARE_REPO="$MUSEHUB_GIT_ROOT/${STORAGE_KEY}.git"
[[ -d "$BARE_REPO" ]] || { echo "missing bare repo at $BARE_REPO" >&2; exit 1; }

snapshot_refs() {
  git --git-dir="$1" for-each-ref --format='%(refname) %(objectname)' | LC_ALL=C sort
}
snapshot_db() {
  psql "$DATABASE_URL" -At -F '|' -c \
    "SELECT entity || '|' || row_data::text
     FROM (
       SELECT 'organizations' AS entity, to_jsonb(t) AS row_data FROM organizations AS t
       UNION ALL SELECT 'principals', to_jsonb(t) FROM principals AS t
       UNION ALL SELECT 'memberships', to_jsonb(t) FROM memberships AS t
       UNION ALL SELECT 'repositories', to_jsonb(t) FROM repositories AS t
       UNION ALL SELECT 'repository_grants', to_jsonb(t) FROM repository_grants AS t
       UNION ALL SELECT 'pipeline_runs', to_jsonb(t) FROM pipeline_runs AS t
       UNION ALL SELECT 'artifacts', to_jsonb(t) FROM artifacts AS t
       UNION ALL SELECT 'releases', to_jsonb(t) FROM releases AS t
       UNION ALL SELECT 'audit_events', to_jsonb(t) FROM audit_events AS t
     ) AS rows
     ORDER BY entity, row_data::text"
}

snapshot_refs "$BARE_REPO" >"$WORK_ROOT/refs.before"
git --git-dir="$BARE_REPO" symbolic-ref HEAD >"$WORK_ROOT/head.before"
snapshot_db >"$WORK_ROOT/db.before"
MARKER_SHA=$(sha256_file "$MUSEHUB_ARTIFACT_ROOT/verify/bundle.bin")

./scripts/backup-musehub-state.sh "$BACKUP_DIR"
./scripts/replicate-musehub-backup.sh "$BACKUP_DIR" "$OBJECT_STORE" "$SNAPSHOT_ID"
./scripts/materialize-musehub-backup.sh "$REMOTE_SNAPSHOT" "$MATERIALIZED_BACKUP"

# Reset only paths created beneath this invocation's private mktemp directory.
[[ "$MUSEHUB_GIT_ROOT" == "$WORK_ROOT/git" && "$MUSEHUB_ARTIFACT_ROOT" == "$WORK_ROOT/artifacts" ]]
rm -rf "$MUSEHUB_GIT_ROOT" "$MUSEHUB_ARTIFACT_ROOT"
./scripts/restore-musehub-state.sh "$MATERIALIZED_BACKUP"

RESTORED_DB="$WORK_ROOT/db.after"
snapshot_db >"$RESTORED_DB"
cmp -s "$WORK_ROOT/db.before" "$RESTORED_DB" || {
  diff -u "$WORK_ROOT/db.before" "$RESTORED_DB" >&2 || true
  echo "restored database entity counts differ" >&2
  exit 1
}

RESTORED_BARE="$MUSEHUB_GIT_ROOT/${STORAGE_KEY}.git"
snapshot_refs "$RESTORED_BARE" >"$WORK_ROOT/refs.after"
git --git-dir="$RESTORED_BARE" symbolic-ref HEAD >"$WORK_ROOT/head.after"
cmp -s "$WORK_ROOT/refs.before" "$WORK_ROOT/refs.after" || {
  diff -u "$WORK_ROOT/refs.before" "$WORK_ROOT/refs.after" >&2 || true
  echo "restored Git refs differ" >&2
  exit 1
}
cmp -s "$WORK_ROOT/head.before" "$WORK_ROOT/head.after" || {
  echo "restored Git HEAD symref differs" >&2
  exit 1
}
grep -q '^refs/heads/feature/restore-check ' "$WORK_ROOT/refs.after"
grep -q '^refs/tags/restore-check ' "$WORK_ROOT/refs.after"
grep -q '^refs/musehub/backup-restore-check ' "$WORK_ROOT/refs.after"

RESTORED_SHA=$(sha256_file "$MUSEHUB_ARTIFACT_ROOT/verify/bundle.bin")
[[ "$MARKER_SHA" == "$RESTORED_SHA" ]] || { echo "artifact sha mismatch" >&2; exit 1; }

GIT_FILES=$(find "$MUSEHUB_GIT_ROOT" -type f | wc -l | tr -d ' ')
[[ "$GIT_FILES" -gt 0 ]] || { echo "restored git tree empty" >&2; exit 1; }
echo "backup_restore_verify=ok db_entities=9 git_refs=$(wc -l < "$WORK_ROOT/refs.after" | tr -d ' ') artifact_sha=$RESTORED_SHA git_files=$GIT_FILES"
