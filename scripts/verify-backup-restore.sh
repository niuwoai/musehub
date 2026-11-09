#!/usr/bin/env bash
# Evidence: backup → wipe DB schema → restore → org + git + artifact byte checks.
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

MARKER_CONTENT="musehub-backup-marker-$(date +%s)"
MARKER_REL="verify-marker.txt"
echo "$MARKER_CONTENT" >"$MUSEHUB_ARTIFACT_ROOT/$MARKER_REL"
MARKER_SHA=$(sha256sum "$MUSEHUB_ARTIFACT_ROOT/$MARKER_REL" | awk '{print $1}')

BACKUP_DIR=$(mktemp -d /tmp/musehub-backup-XXXXXX)

cargo run -p musehub-migrate -- down migrations "$DATABASE_URL" || true
cargo run -p musehub-migrate -- up migrations "$DATABASE_URL"

echo "=== seed demo org via product prepare (security_db) ==="
export MUSEHUB_TEST_DATABASE_URL="$DATABASE_URL"
cargo test -p musehub-app --test security_db -- --test-threads=1 >/dev/null

count_orgs() {
  psql "$DATABASE_URL" -At -c "SELECT COUNT(*) FROM organizations"
}

echo "orgs before=$(count_orgs)"
[[ "$(count_orgs)" -gt 0 ]] || { echo "expected seeded org row" >&2; exit 1; }

chmod +x ./scripts/backup-musehub-state.sh ./scripts/restore-musehub-state.sh
./scripts/backup-musehub-state.sh "$BACKUP_DIR"

BEFORE=$(count_orgs)
BEFORE_MARKER_SHA="$MARKER_SHA"
GIT_BEFORE=$(find "$MUSEHUB_GIT_ROOT" -type f 2>/dev/null | wc -l | tr -d ' ')

psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -c "DROP SCHEMA public CASCADE; CREATE SCHEMA public;"
AFTER_DROP=$(psql "$DATABASE_URL" -At -c "SELECT COUNT(*) FROM pg_tables WHERE schemaname='public'" || echo 0)
echo "tables after drop=$AFTER_DROP"

rm -rf "$MUSEHUB_GIT_ROOT" "$MUSEHUB_ARTIFACT_ROOT"
mkdir -p "$MUSEHUB_GIT_ROOT" "$MUSEHUB_ARTIFACT_ROOT"

./scripts/restore-musehub-state.sh "$BACKUP_DIR"
RESTORED=$(count_orgs)

if [[ "$BEFORE" != "$RESTORED" ]]; then
  echo "org count mismatch before=$BEFORE after=$RESTORED" >&2
  exit 1
fi

if [[ ! -f "$MUSEHUB_ARTIFACT_ROOT/$MARKER_REL" ]]; then
  echo "artifact marker file missing after restore" >&2
  exit 1
fi
RESTORED_SHA=$(sha256sum "$MUSEHUB_ARTIFACT_ROOT/$MARKER_REL" | awk '{print $1}')
if [[ "$BEFORE_MARKER_SHA" != "$RESTORED_SHA" ]]; then
  echo "artifact marker sha mismatch before=$BEFORE_MARKER_SHA after=$RESTORED_SHA" >&2
  exit 1
fi

GIT_AFTER=$(find "$MUSEHUB_GIT_ROOT" -type f 2>/dev/null | wc -l | tr -d ' ')
if [[ "$GIT_BEFORE" != "$GIT_AFTER" ]]; then
  echo "git file count mismatch before=$GIT_BEFORE after=$GIT_AFTER" >&2
  exit 1
fi

rm -rf "$BACKUP_DIR"
echo "backup_restore_verify=ok orgs=$RESTORED git_files=$GIT_AFTER artifact_sha=$RESTORED_SHA"
