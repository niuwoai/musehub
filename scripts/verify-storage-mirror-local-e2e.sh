#!/usr/bin/env bash
# Isolated acceptance for local mirror behavior, loopback SSH/rsync, and PostgreSQL DR barriers.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
E2E_SCRIPT="$SCRIPT_DIR/$(basename -- "${BASH_SOURCE[0]}")"
MIRROR_SCRIPT="$SCRIPT_DIR/mirror-musehub-storage.sh"
if [[ "${MUSEHUB_MIRROR_E2E_IN_CONTAINER:-}" != "YES" ]]; then
  command -v docker >/dev/null 2>&1 || { echo "docker is required" >&2; exit 1; }
  E2E_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/musehub-storage-mirror-e2e.XXXXXX")"
  CONTAINER_NAME="musehub-storage-mirror-e2e-${E2E_ROOT##*.}"
  if ! docker create --name "$CONTAINER_NAME" alpine:3.21 sh -c 'sleep 3600' >/dev/null; then
    rmdir -- "$E2E_ROOT"
    exit 1
  fi
  trap 'docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true; rm -rf -- "$E2E_ROOT"' EXIT
  docker start "$CONTAINER_NAME" >/dev/null
  docker cp "$MIRROR_SCRIPT" "$CONTAINER_NAME:/opt/mirror-musehub-storage.sh"
  docker cp "$E2E_SCRIPT" "$CONTAINER_NAME:/opt/verify-storage-mirror-local-e2e.sh"
  docker cp "$SCRIPT_DIR/create-musehub-dr-barrier.sh" "$CONTAINER_NAME:/opt/create-musehub-dr-barrier.sh"
  docker cp "$SCRIPT_DIR/verify-musehub-dr-barrier.sh" "$CONTAINER_NAME:/opt/verify-musehub-dr-barrier.sh"
  docker cp "$SCRIPT_DIR/../migrations/0034_storage_recovery_barriers.up.sql" "$CONTAINER_NAME:/opt/0034_storage_recovery_barriers.up.sql"
  docker cp "$SCRIPT_DIR/../migrations/0034_storage_recovery_barriers.down.sql" "$CONTAINER_NAME:/opt/0034_storage_recovery_barriers.down.sql"
  docker exec "$CONTAINER_NAME" apk add --no-cache bash rsync util-linux coreutils openssh postgresql17 postgresql17-client >/dev/null
  docker exec --env MUSEHUB_MIRROR_E2E_IN_CONTAINER=YES "$CONTAINER_NAME" bash /opt/verify-storage-mirror-local-e2e.sh
  exit $?
fi

MIRROR_SCRIPT=/opt/mirror-musehub-storage.sh
GIT_ROOT=/e2e/source/git
ARTIFACT_ROOT=/e2e/source/artifacts
REMOTE_ROOT=/e2e/target
STATE_DIR=/e2e/state
mkdir -p "$GIT_ROOT" "$ARTIFACT_ROOT" "$REMOTE_ROOT/git" "$REMOTE_ROOT/artifacts" "$REMOTE_ROOT/.musehub-mirror-status" "$STATE_DIR"
printf '%s\n' 'musehub-storage-mirror-target-v1' >"$REMOTE_ROOT/.musehub-mirror-target"
printf '%s\n' 'git-object-v1' >"$GIT_ROOT/object"
printf '%s\n' 'artifact-v1' >"$ARTIFACT_ROOT/old.bin"

export MUSEHUB_GIT_ROOT="$GIT_ROOT"
export MUSEHUB_ARTIFACT_ROOT="$ARTIFACT_ROOT"
export MUSEHUB_STORAGE_MIRROR_TARGET=local
export MUSEHUB_STORAGE_MIRROR_REMOTE_ROOT="$REMOTE_ROOT"
export MUSEHUB_STORAGE_MIRROR_STATE_DIR="$STATE_DIR"
export MUSEHUB_STORAGE_MIRROR_ALLOW_LOCAL=YES

bash "$MIRROR_SCRIPT" continuous >/dev/null
test "$(<"$REMOTE_ROOT/git/object")" = 'git-object-v1'
test "$(<"$REMOTE_ROOT/artifacts/old.bin")" = 'artifact-v1'
test -s "$STATE_DIR/latest"

printf '%s\n' 'git-object-v2' >"$GIT_ROOT/object"
printf '%s\n' 'artifact-v2' >"$ARTIFACT_ROOT/new.bin"
rm -- "$ARTIFACT_ROOT/old.bin"
bash "$MIRROR_SCRIPT" continuous >/dev/null
test "$(<"$REMOTE_ROOT/git/object")" = 'git-object-v2'
test -e "$REMOTE_ROOT/artifacts/old.bin"
test -e "$REMOTE_ROOT/artifacts/new.bin"
checkpoint_before_failure="$(<"$STATE_DIR/latest")"
mv -- "$REMOTE_ROOT/artifacts" "$REMOTE_ROOT/artifacts.unavailable"
if bash "$MIRROR_SCRIPT" continuous >/dev/null 2>&1; then
  echo 'continuous sync unexpectedly succeeded without its artifact target' >&2
  exit 1
fi
mv -- "$REMOTE_ROOT/artifacts.unavailable" "$REMOTE_ROOT/artifacts"
test "$(<"$STATE_DIR/latest")" = "$checkpoint_before_failure"

if bash "$MIRROR_SCRIPT" verify >/dev/null 2>&1; then
  echo 'verify unexpectedly accepted a stale destination file' >&2
  exit 1
fi
if bash "$MIRROR_SCRIPT" quiesced-final >/dev/null 2>&1; then
  echo 'quiesced-final unexpectedly ran without explicit acknowledgements' >&2
  exit 1
fi
export MUSEHUB_STORAGE_SOURCE_QUIESCED=YES
if bash "$MIRROR_SCRIPT" quiesced-final >/dev/null 2>&1; then
  echo 'quiesced-final unexpectedly ran without deletion acknowledgement' >&2
  exit 1
fi
export MUSEHUB_STORAGE_MIRROR_ALLOW_DELETE=YES
export MUSEHUB_STORAGE_MIRROR_BARRIER_ID=e57d4c82-6f4a-4d1c-972a-23d7ea2aef07
export MUSEHUB_STORAGE_MIRROR_BARRIER_LSN=0/16B6C50
bash "$MIRROR_SCRIPT" quiesced-final >/dev/null
test ! -e "$REMOTE_ROOT/artifacts/old.bin"
grep -Fqx "barrier_id=$MUSEHUB_STORAGE_MIRROR_BARRIER_ID" "$REMOTE_ROOT/.musehub-mirror-status/latest"
grep -Fqx "barrier_lsn=$MUSEHUB_STORAGE_MIRROR_BARRIER_LSN" "$REMOTE_ROOT/.musehub-mirror-status/latest"
test "$(<"$REMOTE_ROOT/artifacts/new.bin")" = 'artifact-v2'
grep -qx 'mode=quiesced-final' "$STATE_DIR/latest"
bash "$MIRROR_SCRIPT" verify >/dev/null

SSH_REMOTE_ROOT=/e2e/ssh-target
SSH_STATE_DIR=/e2e/ssh-state
mkdir -p "$SSH_REMOTE_ROOT/git" "$SSH_REMOTE_ROOT/artifacts" "$SSH_REMOTE_ROOT/.musehub-mirror-status" "$SSH_STATE_DIR" /root/.ssh /run/sshd
printf '%s\n' 'musehub-storage-mirror-target-v1' >"$SSH_REMOTE_ROOT/.musehub-mirror-target"
ssh-keygen -q -t ed25519 -N '' -f /tmp/musehub-mirror-e2e-key
cp /tmp/musehub-mirror-e2e-key.pub /root/.ssh/authorized_keys
chmod 700 /root/.ssh
chmod go-rwx,u+rw /root/.ssh/authorized_keys
ssh-keygen -A >/dev/null 2>&1
/usr/sbin/sshd -f /dev/null -D -e -p 2222 \
  -o PidFile=/tmp/musehub-mirror-e2e-sshd.pid \
  -o HostKey=/etc/ssh/ssh_host_ed25519_key \
  -o HostKey=/etc/ssh/ssh_host_rsa_key \
  -o AuthorizedKeysFile=/root/.ssh/authorized_keys \
  -o PermitRootLogin=yes \
  -o PasswordAuthentication=no \
  -o StrictModes=no >/tmp/musehub-mirror-e2e-sshd.log 2>&1 &
for attempt in {1..30}; do
  if ssh-keyscan -T 1 -p 2222 -t ed25519 127.0.0.1 >/root/.ssh/known_hosts 2>/dev/null && [[ -s /root/.ssh/known_hosts ]]; then
    break
  fi
  if [[ "$attempt" -eq 30 ]]; then
    echo 'isolated sshd did not publish its host key' >&2
    exit 1
  fi
  sleep 0.2
done
printf 'Host 127.0.0.1\n  Port 2222\n  IdentityFile /tmp/musehub-mirror-e2e-key\n  UserKnownHostsFile /root/.ssh/known_hosts\n' > /root/.ssh/config
chmod go-rwx,u+rw /root/.ssh/config
if ! ssh -o BatchMode=yes -o StrictHostKeyChecking=yes root@127.0.0.1 true; then
  echo 'isolated SSH authentication failed' >&2
  cat /tmp/musehub-mirror-e2e-sshd.log >&2
  exit 1
fi

export MUSEHUB_STORAGE_MIRROR_TARGET=root@127.0.0.1
export MUSEHUB_STORAGE_MIRROR_REMOTE_ROOT="$SSH_REMOTE_ROOT"
export MUSEHUB_STORAGE_MIRROR_STATE_DIR="$SSH_STATE_DIR"
unset MUSEHUB_STORAGE_MIRROR_ALLOW_DELETE MUSEHUB_STORAGE_SOURCE_QUIESCED
bash "$MIRROR_SCRIPT" continuous >/dev/null
test "$(<"$SSH_REMOTE_ROOT/git/object")" = 'git-object-v2'
printf '%s\n' 'git-object-v3' >"$GIT_ROOT/object"
bash "$MIRROR_SCRIPT" continuous >/dev/null
test "$(<"$SSH_REMOTE_ROOT/git/object")" = 'git-object-v3'
printf '%s\n' 'stale-artifact' >"$ARTIFACT_ROOT/stale.bin"
bash "$MIRROR_SCRIPT" continuous >/dev/null
test -e "$SSH_REMOTE_ROOT/artifacts/stale.bin"
rm -- "$ARTIFACT_ROOT/stale.bin"
if bash "$MIRROR_SCRIPT" verify >/dev/null 2>&1; then
  echo 'SSH verify unexpectedly accepted a stale destination file' >&2
  exit 1
fi
export MUSEHUB_STORAGE_SOURCE_QUIESCED=YES MUSEHUB_STORAGE_MIRROR_ALLOW_DELETE=YES
bash "$MIRROR_SCRIPT" quiesced-final >/dev/null
test ! -e "$SSH_REMOTE_ROOT/artifacts/stale.bin"
bash "$MIRROR_SCRIPT" verify >/dev/null

PG_PRIMARY_DIR=/var/lib/postgresql/musehub-primary
PG_STANDBY_DIR=/var/lib/postgresql/musehub-standby
PRIMARY_URL=postgresql://postgres@127.0.0.1:55432/postgres
STANDBY_URL=postgresql://postgres@127.0.0.1:55433/postgres
mkdir -p "$PG_PRIMARY_DIR" /run/postgresql
chown -R postgres:postgres "$PG_PRIMARY_DIR" /run/postgresql
su postgres -s /bin/sh -c "initdb -D '$PG_PRIMARY_DIR' --username=postgres --auth-local=trust --auth-host=trust"
if ! su postgres -s /bin/sh -c "pg_ctl -D '$PG_PRIMARY_DIR' -l '$PG_PRIMARY_DIR/server.log' -o '-h 127.0.0.1 -p 55432 -c wal_level=replica -c max_wal_senders=10' start"; then
  cat "$PG_PRIMARY_DIR/server.log" >&2
  exit 1
fi
psql "$PRIMARY_URL" -X -v ON_ERROR_STOP=1 \
  -c 'CREATE FUNCTION musehub_set_updated_at() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.updated_at = CURRENT_TIMESTAMP; RETURN NEW; END $$' >/dev/null
psql "$PRIMARY_URL" -X -v ON_ERROR_STOP=1 \
  -f /opt/0034_storage_recovery_barriers.up.sql >/dev/null
psql "$PRIMARY_URL" -X -v ON_ERROR_STOP=1 \
  -c 'CREATE ROLE musehub_replication WITH REPLICATION LOGIN' >/dev/null
su postgres -s /bin/sh -c "pg_basebackup -h 127.0.0.1 -p 55432 -U musehub_replication -D '$PG_STANDBY_DIR' -R -X stream"
if ! su postgres -s /bin/sh -c "pg_ctl -D '$PG_STANDBY_DIR' -l '$PG_STANDBY_DIR/server.log' -o '-h 127.0.0.1 -p 55433 -c hot_standby=on' start"; then
  cat "$PG_STANDBY_DIR/server.log" >&2
  exit 1
fi
barrier_output="$(MUSEHUB_DR_DATABASE_URL="$PRIMARY_URL" MUSEHUB_STORAGE_SOURCE_QUIESCED=YES bash /opt/create-musehub-dr-barrier.sh)"
MUSEHUB_STORAGE_MIRROR_BARRIER_ID="$(printf '%s\n' "$barrier_output" | awk -F= '$1 == "dr_barrier_id" { print $2 }')"
MUSEHUB_STORAGE_MIRROR_BARRIER_LSN="$(printf '%s\n' "$barrier_output" | awk -F= '$1 == "dr_barrier_lsn" { print $2 }')"
export MUSEHUB_STORAGE_MIRROR_BARRIER_ID MUSEHUB_STORAGE_MIRROR_BARRIER_LSN
export MUSEHUB_STORAGE_SOURCE_QUIESCED=YES MUSEHUB_STORAGE_MIRROR_ALLOW_DELETE=YES
bash "$MIRROR_SCRIPT" quiesced-final >/dev/null
export MUSEHUB_DR_DATABASE_URL="$STANDBY_URL"
export MUSEHUB_DR_MIRROR_STATUS_FILE="$SSH_REMOTE_ROOT/.musehub-mirror-status/latest"
export MUSEHUB_DR_BARRIER_ID="$MUSEHUB_STORAGE_MIRROR_BARRIER_ID"
export MUSEHUB_DR_BARRIER_LSN="$MUSEHUB_STORAGE_MIRROR_BARRIER_LSN"
mirror_status_backup=/tmp/musehub-mirror-status-before-lag-check
cp "$MUSEHUB_DR_MIRROR_STATUS_FILE" "$mirror_status_backup"
awk 'BEGIN { replaced = 0 } /^barrier_lsn=/ { print "barrier_lsn=FFFFFFFF/FFFFFFFF"; replaced = 1; next } { print } END { if (!replaced) exit 1 }' \
  "$mirror_status_backup" >"$MUSEHUB_DR_MIRROR_STATUS_FILE"
export MUSEHUB_DR_BARRIER_LSN=FFFFFFFF/FFFFFFFF
if bash /opt/verify-musehub-dr-barrier.sh >/dev/null 2>&1; then
  echo 'DR barrier verifier unexpectedly accepted an unreplayed future LSN' >&2
  exit 1
fi
mv -- "$mirror_status_backup" "$MUSEHUB_DR_MIRROR_STATUS_FILE"
export MUSEHUB_DR_BARRIER_LSN="$MUSEHUB_STORAGE_MIRROR_BARRIER_LSN"
for attempt in {1..30}; do
  if bash /opt/verify-musehub-dr-barrier.sh >/dev/null 2>&1; then
    break
  fi
  if [[ "$attempt" -eq 30 ]]; then
    bash /opt/verify-musehub-dr-barrier.sh
    exit 1
  fi
  sleep 0.2
done
replica_result="$(bash /opt/verify-musehub-dr-barrier.sh)"
[[ "$replica_result" == *'state=replica-ready'* ]] || {
  echo "expected a caught-up standby, got: $replica_result" >&2
  exit 1
}
su postgres -s /bin/sh -c "pg_ctl -D '$PG_PRIMARY_DIR' -m fast stop"
su postgres -s /bin/sh -c "pg_ctl -D '$PG_STANDBY_DIR' promote"
for attempt in {1..30}; do
  primary_state="$(psql "$STANDBY_URL" -X -A -t -v ON_ERROR_STOP=1 \
    -c "SELECT NOT pg_is_in_recovery() AND NOT current_setting('transaction_read_only')::boolean" \
    | tr -d '[:space:]')"
  [[ "$primary_state" == "t" ]] && break
  if [[ "$attempt" -eq 30 ]]; then
    echo 'local PostgreSQL standby did not promote' >&2
    exit 1
  fi
  sleep 0.2
done
primary_result="$(bash /opt/verify-musehub-dr-barrier.sh)"
[[ "$primary_result" == *'state=primary-ready'* ]] || {
  echo "expected the promoted primary, got: $primary_result" >&2
  exit 1
}
psql "$STANDBY_URL" -X -v ON_ERROR_STOP=1 -f /opt/0034_storage_recovery_barriers.down.sql >/dev/null
down_table_state="$(psql "$STANDBY_URL" -X -A -t -v ON_ERROR_STOP=1 \
  -c "SELECT to_regclass('public.storage_recovery_barriers') IS NULL" | tr -d '[:space:]')"
[[ "$down_table_state" == "t" ]] || {
  echo 'storage recovery barrier down migration left its table behind' >&2
  exit 1
}
printf 'storage_mirror_local_e2e=ok\nstorage_mirror_ssh_e2e=ok\n%s\n%s\nstorage_recovery_barrier_down=ok\n' "$replica_result" "$primary_result"
