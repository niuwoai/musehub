#!/usr/bin/env bash
# Verify the standby or promoted primary matches the final storage mirror barrier.
set -euo pipefail

DATABASE_URL="${MUSEHUB_DR_DATABASE_URL:?set MUSEHUB_DR_DATABASE_URL to the DR database endpoint}"
STATUS_FILE="${MUSEHUB_DR_MIRROR_STATUS_FILE:?set the local path to .musehub-mirror-status/latest}"
BARRIER_ID="${MUSEHUB_DR_BARRIER_ID:?set the PostgreSQL/storage barrier UUID}"
BARRIER_LSN="${MUSEHUB_DR_BARRIER_LSN:?set the PostgreSQL barrier WAL position}"

[[ "$BARRIER_ID" =~ ^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$ ]] || {
  echo "barrier ID must be a UUID" >&2
  exit 2
}
[[ "$BARRIER_LSN" =~ ^[0-9A-Fa-f]+/[0-9A-Fa-f]+$ ]] || {
  echo "barrier LSN must have PostgreSQL X/Y format" >&2
  exit 2
}
[[ -f "$STATUS_FILE" && ! -L "$STATUS_FILE" ]] || {
  echo "mirror status must be an existing, non-symlink file" >&2
  exit 1
}
grep -Fqx 'format=musehub-storage-mirror-v1' "$STATUS_FILE" || {
  echo "mirror status format is invalid" >&2
  exit 1
}
grep -Fqx 'mode=quiesced-final' "$STATUS_FILE" || {
  echo "mirror status is not from a quiesced final sync" >&2
  exit 1
}
grep -Fqx "barrier_id=$BARRIER_ID" "$STATUS_FILE" || {
  echo "mirror status barrier ID does not match PostgreSQL" >&2
  exit 1
}
grep -Fqx "barrier_lsn=$BARRIER_LSN" "$STATUS_FILE" || {
  echo "mirror status barrier LSN does not match PostgreSQL" >&2
  exit 1
}
command -v psql >/dev/null 2>&1 || { echo "psql is required" >&2; exit 1; }

query="SELECT CASE WHEN pg_is_in_recovery() THEN EXISTS (SELECT 1 FROM storage_recovery_barriers WHERE barrier_id = :'barrier_id'::uuid) AND pg_last_wal_replay_lsn() >= :'barrier_lsn'::pg_lsn ELSE NOT current_setting('transaction_read_only')::boolean AND EXISTS (SELECT 1 FROM storage_recovery_barriers WHERE barrier_id = :'barrier_id'::uuid) END;"
caught_up="$(printf '%s\n' "$query" \
  | psql "$DATABASE_URL" -X -A -t -v ON_ERROR_STOP=1 \
      -v barrier_id="$BARRIER_ID" -v barrier_lsn="$BARRIER_LSN" -f - \
  | tr -d '[:space:]')"
[[ "$caught_up" == "t" ]] || {
  echo "DR database has not replayed the barrier or is not a writable promoted primary" >&2
  exit 1
}
state="$(psql "$DATABASE_URL" -X -A -t -v ON_ERROR_STOP=1 \
  -c "SELECT CASE WHEN pg_is_in_recovery() THEN 'replica-ready' ELSE 'primary-ready' END" \
  | tr -d '[:space:]')"
printf 'musehub_dr_barrier=ok state=%s barrier_id=%s barrier_lsn=%s\n' \
  "$state" "$BARRIER_ID" "$BARRIER_LSN"
