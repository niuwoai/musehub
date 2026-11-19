#!/usr/bin/env bash
# Record a durable PostgreSQL barrier after application writes are quiesced.
set -euo pipefail

DATABASE_URL="${MUSEHUB_DR_DATABASE_URL:?set MUSEHUB_DR_DATABASE_URL to the writable primary}"
[[ "${MUSEHUB_STORAGE_SOURCE_QUIESCED:-}" == "YES" ]] || {
  echo "stop all application writers and set MUSEHUB_STORAGE_SOURCE_QUIESCED=YES" >&2
  exit 1
}
command -v psql >/dev/null 2>&1 || { echo "psql is required" >&2; exit 1; }
command -v uuidgen >/dev/null 2>&1 || { echo "uuidgen is required" >&2; exit 1; }

writable="$(psql "$DATABASE_URL" -X -A -t -v ON_ERROR_STOP=1 \
  -c "SELECT NOT pg_is_in_recovery() AND NOT current_setting('transaction_read_only')::boolean" \
  | tr -d '[:space:]')"
[[ "$writable" == "t" ]] || {
  echo "DR barrier requires a writable PostgreSQL primary" >&2
  exit 1
}

barrier_id="$(uuidgen | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')"
[[ "$barrier_id" =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ ]] || {
  echo "uuidgen returned an invalid barrier ID" >&2
  exit 1
}
printf '%s\n' "INSERT INTO storage_recovery_barriers (barrier_id) VALUES (:'barrier_id'::uuid);" \
  | psql "$DATABASE_URL" -X -v ON_ERROR_STOP=1 -v barrier_id="$barrier_id" -f - >/dev/null
barrier_lsn="$(psql "$DATABASE_URL" -X -A -t -v ON_ERROR_STOP=1 \
  -c "SELECT pg_current_wal_lsn()::text" | tr -d '[:space:]')"
[[ "$barrier_lsn" =~ ^[0-9A-F]+/[0-9A-F]+$ ]] || {
  echo "PostgreSQL returned an invalid WAL barrier position" >&2
  exit 1
}
printf 'dr_barrier_id=%s\ndr_barrier_lsn=%s\n' "$barrier_id" "$barrier_lsn"
