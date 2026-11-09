#!/usr/bin/env bash
# Evidence: backup → wipe → restore → org row + bare Git OID + artifact bytes.
set -euo pipefail
cd "$(dirname "$0")/.."

DATABASE_URL="${MUSEHUB_TEST_DATABASE_URL:-${DATABASE_URL:-}}"
[[ -n "$DATABASE_URL" ]] || { echo "set DATABASE_URL" >&2; exit 1; }

export MUSEHUB_GIT_ROOT="${MUSEHUB_GIT_ROOT:-/tmp/musehub-backup-git}"
export MUSEHUB_ARTIFACT_ROOT="${MUSEHUB_ARTIFACT_ROOT:-/tmp/musehub-backup-artifacts}"
export MUSEHUB_SESSION_SECRET="${MUSEHUB_SESSION_SECRET:-backup-restore-secret-32chars}"
export MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD="${MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD:-Backup-Bootstrap-Strong-1!}"
rm -rf "$MUSEHUB_GIT_ROOT" "$MUSEHUB_ARTIFACT_ROOT"
mkdir -p "$MUSEHUB_GIT_ROOT" "$MUSEHUB_ARTIFACT_ROOT"

MARKER_REL="verify/bundle.bin"
MARKER_CONTENT="musehub-backup-marker-$(date +%s)"
mkdir -p "$MUSEHUB_ARTIFACT_ROOT/verify"
printf '%s' "$MARKER_CONTENT" >"$MUSEHUB_ARTIFACT_ROOT/$MARKER_REL"
MARKER_SHA=$(sha256sum "$MUSEHUB_ARTIFACT_ROOT/$MARKER_REL" | awk '{print $1}')

BACKUP_DIR=$(mktemp -d /tmp/musehub-backup-XXXXXX)

cargo run -p musehub-migrate -- down migrations "$DATABASE_URL" || true
cargo run -p musehub-migrate -- up migrations "$DATABASE_URL"

echo "=== seed demo org (security_db) ==="
export MUSEHUB_TEST_DATABASE_URL="$DATABASE_URL"
cargo test -p musehub-app --test security_db -- --test-threads=1 >/dev/null

chmod +x ./scripts/seed-backup-git-fixture.sh
./scripts/seed-backup-git-fixture.sh

STORAGE_KEY=$(psql "$DATABASE_URL" -At -c \
  "SELECT storage_key FROM repositories WHERE slug_normalized = 'backup-fixture' LIMIT 1")
[[ -n "$STORAGE_KEY" ]] || { echo "no repository row after seed" >&2; exit 1; }
BARE_REPO="$MUSEHUB_GIT_ROOT/${STORAGE_KEY}.git"
[[ -d "$BARE_REPO" ]] || { echo "missing bare repo at $BARE_REPO" >&2; exit 1; }
MAIN_OID=$(git --git-dir="$BARE_REPO" rev-parse refs/heads/main 2>/dev/null || true)
[[ -n "$MAIN_OID" ]] || { echo "bare repo missing refs/heads/main" >&2; exit 1; }
GIT_OBJECT_COUNT=$(git --git-dir="$BARE_REPO" rev-list --count "$MAIN_OID")
[[ "$GIT_OBJECT_COUNT" -ge 1 ]] || { echo "bare repo has no commits" >&2; exit 1; }

count_orgs() {
  psql "$DATABASE_URL" -At -c "SELECT COUNT(*) FROM organizations"
}

echo "orgs before=$(count_orgs) main_oid=$MAIN_OID git_objects=$GIT_OBJECT_COUNT marker_sha=$MARKER_SHA"

chmod +x ./scripts/backup-musehub-state.sh ./scripts/restore-musehub-state.sh
./scripts/backup-musehub-state.sh "$BACKUP_DIR"

BEFORE_ORGS=$(count_orgs)
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -c "DROP SCHEMA public CASCADE; CREATE SCHEMA public;"
rm -rf "$MUSEHUB_GIT_ROOT" "$MUSEHUB_ARTIFACT_ROOT"
mkdir -p "$MUSEHUB_GIT_ROOT" "$MUSEHUB_ARTIFACT_ROOT"

./scripts/restore-musehub-state.sh "$BACKUP_DIR"
RESTORED_ORGS=$(count_orgs)
[[ "$BEFORE_ORGS" == "$RESTORED_ORGS" ]] || { echo "org count mismatch" >&2; exit 1; }

RESTORED_OID=$(git --git-dir="$MUSEHUB_GIT_ROOT/${STORAGE_KEY}.git" rev-parse refs/heads/main)
[[ "$MAIN_OID" == "$RESTORED_OID" ]] || {
  echo "main oid mismatch before=$MAIN_OID after=$RESTORED_OID" >&2
  exit 1
}

[[ -f "$MUSEHUB_ARTIFACT_ROOT/$MARKER_REL" ]] || { echo "artifact missing" >&2; exit 1; }
RESTORED_SHA=$(sha256sum "$MUSEHUB_ARTIFACT_ROOT/$MARKER_REL" | awk '{print $1}')
[[ "$MARKER_SHA" == "$RESTORED_SHA" ]] || { echo "artifact sha mismatch" >&2; exit 1; }

GIT_FILES=$(find "$MUSEHUB_GIT_ROOT" -type f | wc -l | tr -d ' ')
[[ "$GIT_FILES" -gt 0 ]] || { echo "restored git tree empty" >&2; exit 1; }

rm -rf "$BACKUP_DIR"
echo "backup_restore_verify=ok orgs=$RESTORED_ORGS main_oid=$RESTORED_OID artifact_sha=$RESTORED_SHA git_files=$GIT_FILES"
