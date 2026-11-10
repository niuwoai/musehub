#!/usr/bin/env bash
# After security_db: insert one repository row and init bare main (matches product create_repo).
set -euo pipefail
: "${DATABASE_URL:?}"
: "${MUSEHUB_GIT_ROOT:?}"

ORG_ID=$(psql "$DATABASE_URL" -At -c \
  "SELECT id FROM organizations ORDER BY created_at LIMIT 1")
[[ -n "$ORG_ID" ]] || { echo "no organization to seed git fixture" >&2; exit 1; }

STORAGE_KEY=$(python3 -c "import uuid; print(uuid.uuid4())")
REPO_ID=$(python3 -c "import uuid; print(uuid.uuid4())")
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -c \
  "INSERT INTO repositories (id, org_id, slug, slug_normalized, storage_key, state, default_branch)
   VALUES ('$REPO_ID'::uuid, '$ORG_ID'::uuid, 'backup-fixture', 'backup-fixture', '$STORAGE_KEY'::uuid, 'ready', 'main');"

BARE="$MUSEHUB_GIT_ROOT/${STORAGE_KEY}.git"
mkdir -p "$MUSEHUB_GIT_ROOT"
git init --bare "$BARE"
git --git-dir="$BARE" symbolic-ref HEAD refs/heads/main
TMP=$(mktemp -d)
git -C "$TMP" init
git -C "$TMP" -c user.email=git@musehub.local -c user.name=musehub commit --allow-empty -m init --no-gpg-sign
git -C "$TMP" push "$BARE" HEAD:main
rm -rf "$TMP"
MAIN_OID=$(git --git-dir="$BARE" rev-parse refs/heads/main)
echo "backup_git_fixture=ok storage_key=$STORAGE_KEY main_oid=$MAIN_OID"
