#!/usr/bin/env bash
# Seed a disposable backup fixture with access, delivery, release, audit, and Git refs.
set -euo pipefail
: "${DATABASE_URL:?}"
: "${MUSEHUB_GIT_ROOT:?}"
: "${MUSEHUB_ARTIFACT_ROOT:?}"

ORG_ID=$(psql "$DATABASE_URL" -At -c \
  "SELECT id FROM organizations ORDER BY created_at LIMIT 1")
[[ -n "$ORG_ID" ]] || { echo "no organization to seed git fixture" >&2; exit 1; }
OWNER_ID=$(psql "$DATABASE_URL" -At -c \
  "SELECT principal_id FROM memberships WHERE org_id = '$ORG_ID'::uuid AND role = 'owner' ORDER BY created_at LIMIT 1")
[[ -n "$OWNER_ID" ]] || { echo "no organization owner to seed fixture" >&2; exit 1; }

STORAGE_KEY=$(python3 -I -c 'import uuid; print(uuid.uuid4())')
REPO_ID=$(python3 -I -c 'import uuid; print(uuid.uuid4())')
PIPELINE_ID=$(python3 -I -c 'import uuid; print(uuid.uuid4())')
RUN_ATTEMPT_ID=$(python3 -I -c 'import uuid; print(uuid.uuid4())')
ARTIFACT_ID=$(python3 -I -c 'import uuid; print(uuid.uuid4())')
RELEASE_ID=$(python3 -I -c 'import uuid; print(uuid.uuid4())')
AUDIT_ID=$(python3 -I -c 'import uuid; print(uuid.uuid4())')
UUID_RE='^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'
for value in "$ORG_ID" "$OWNER_ID" "$STORAGE_KEY" "$REPO_ID" "$PIPELINE_ID" "$RUN_ATTEMPT_ID" "$ARTIFACT_ID" "$RELEASE_ID" "$AUDIT_ID"; do
  [[ "$value" =~ $UUID_RE ]] || { echo "invalid UUID in backup fixture" >&2; exit 1; }
done
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -c \
  "INSERT INTO repositories (id, org_id, slug, slug_normalized, storage_key, state, default_branch)
   VALUES ('$REPO_ID'::uuid, '$ORG_ID'::uuid, 'backup-fixture', 'backup-fixture', '$STORAGE_KEY'::uuid, 'ready', 'main');
   INSERT INTO repository_grants (id, org_id, repo_id, principal_id, role)
   VALUES (gen_random_uuid(), '$ORG_ID'::uuid, '$REPO_ID'::uuid, '$OWNER_ID'::uuid, 'maintainer');"

BARE="$MUSEHUB_GIT_ROOT/${STORAGE_KEY}.git"
mkdir -p "$MUSEHUB_GIT_ROOT" "$MUSEHUB_ARTIFACT_ROOT/verify"
git init --bare "$BARE"
git --git-dir="$BARE" symbolic-ref HEAD refs/heads/main
TMP=$(mktemp -d "${TMPDIR:-/tmp}/musehub-backup-fixture.XXXXXX")
trap 'rm -rf "$TMP"' EXIT
git -C "$TMP" init -b main
printf '%s' 'artifactbytesv1' >"$MUSEHUB_ARTIFACT_ROOT/verify/bundle.bin"
ARTIFACT_SHA=$(if command -v sha256sum >/dev/null 2>&1; then sha256sum "$MUSEHUB_ARTIFACT_ROOT/verify/bundle.bin" | awk '{print $1}'; else shasum -a 256 "$MUSEHUB_ARTIFACT_ROOT/verify/bundle.bin" | awk '{print $1}'; fi)
BYTE_LENGTH=$(wc -c <"$MUSEHUB_ARTIFACT_ROOT/verify/bundle.bin" | tr -d ' ')
printf '%s' 'main fixture' >"$TMP/main.txt"
git -C "$TMP" add main.txt
git -C "$TMP" -c user.email=git@musehub.local -c user.name=musehub commit -m main --no-gpg-sign >/dev/null
git -C "$TMP" push "$BARE" HEAD:main >/dev/null
MAIN_OID=$(git --git-dir="$BARE" rev-parse refs/heads/main)
git -C "$TMP" checkout -b feature/restore-check >/dev/null
printf '%s' 'feature fixture' >"$TMP/feature.txt"
git -C "$TMP" add feature.txt
git -C "$TMP" -c user.email=git@musehub.local -c user.name=musehub commit -m feature --no-gpg-sign >/dev/null
git -C "$TMP" tag restore-check
git -C "$TMP" push "$BARE" HEAD:refs/heads/feature/restore-check >/dev/null
git -C "$TMP" push "$BARE" refs/tags/restore-check >/dev/null
FEATURE_OID=$(git --git-dir="$BARE" rev-parse refs/heads/feature/restore-check)
git --git-dir="$BARE" update-ref refs/musehub/backup-restore-check "$FEATURE_OID"

OID_RE='^[0-9a-fA-F]{40}$'
SHA_RE='^[0-9a-fA-F]{64}$'
[[ "$MAIN_OID" =~ $OID_RE && "$FEATURE_OID" =~ $OID_RE ]] || { echo "invalid Git object ID in backup fixture" >&2; exit 1; }
[[ "$ARTIFACT_SHA" =~ $SHA_RE && "$BYTE_LENGTH" =~ ^[0-9]+$ ]] || { echo "invalid artifact metadata in backup fixture" >&2; exit 1; }
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -c \
  "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state, requested_by, run_attempt_id)
   VALUES ('$PIPELINE_ID'::uuid, '$ORG_ID'::uuid, '$REPO_ID'::uuid, '$MAIN_OID', 'backup-fixture', 'succeeded', '$OWNER_ID'::uuid, '$RUN_ATTEMPT_ID'::uuid);
   INSERT INTO artifacts (id, org_id, repo_id, pipeline_run_id, name, sha256, byte_length, storage_path, state)
   VALUES ('$ARTIFACT_ID'::uuid, '$ORG_ID'::uuid, '$REPO_ID'::uuid, '$PIPELINE_ID'::uuid, 'bundle.bin', '$ARTIFACT_SHA', $BYTE_LENGTH::bigint, 'verify/bundle.bin', 'ready');
   INSERT INTO releases (id, org_id, repo_id, version, tag_name, commit_oid, pipeline_run_id, manifest_sha256, state)
   VALUES ('$RELEASE_ID'::uuid, '$ORG_ID'::uuid, '$REPO_ID'::uuid, 'restore-check-1', 'restore-check', '$FEATURE_OID', '$PIPELINE_ID'::uuid, '$ARTIFACT_SHA', 'published');
   INSERT INTO audit_events (id, org_id, repo_id, actor_id, action, resource_type, resource_id, decision)
   VALUES ('$AUDIT_ID'::uuid, '$ORG_ID'::uuid, '$REPO_ID'::uuid, '$OWNER_ID'::uuid, 'backup.restore.fixture', 'release', '$RELEASE_ID'::uuid, 'allow');"
echo "backup_git_fixture=ok storage_key=$STORAGE_KEY main_oid=$MAIN_OID feature_oid=$FEATURE_OID artifact_sha=$ARTIFACT_SHA"
