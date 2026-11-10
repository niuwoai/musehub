#!/bin/sh
# Product server entrypoint: migrate (idempotent) then start API. No host docker access.
set -eu

: "${DATABASE_URL:?DATABASE_URL is required}"
: "${MUSEHUB_GIT_ROOT:?MUSEHUB_GIT_ROOT is required}"
: "${MUSEHUB_SESSION_SECRET:?MUSEHUB_SESSION_SECRET is required}"
: "${MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD:?MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD is required}"

# Keep in sync with musehub_app::security::validate_bootstrap_password (min length).
pw_len="$(printf '%s' "$MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD" | wc -c | tr -d ' ')"
if [ "$pw_len" -lt 16 ]; then
  echo "musehub-entrypoint: bootstrap password must be at least 16 characters (got $pw_len)" >&2
  exit 1
fi

mkdir -p "$MUSEHUB_GIT_ROOT" "${MUSEHUB_ARTIFACT_ROOT:-/var/lib/musehub/artifacts}"

echo "musehub-entrypoint: migrate"
migrate_ok=0
for i in 1 2 3 4 5 6 7 8 9 10; do
  if /usr/local/bin/musehub-migrate up /migrations "$DATABASE_URL"; then
    migrate_ok=1
    break
  fi
  echo "musehub-entrypoint: migrate retry $i" >&2
  sleep 3
done
if [ "$migrate_ok" -ne 1 ]; then
  echo "musehub-entrypoint: migrate failed after retries" >&2
  exit 1
fi

case "${1:-server}" in
  migrate-only)
    echo "musehub-entrypoint: migrate-only complete"
    exit 0
    ;;
  server)
    echo "musehub-entrypoint: starting musehub-server"
    exec /usr/local/bin/musehub-server
    ;;
  *)
    echo "unknown command: $1" >&2
    exit 2
    ;;
esac
