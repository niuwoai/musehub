#!/usr/bin/env bash
# Two-host shared Git/artifact visibility, rename, lock, and survivor acceptance.
set -euo pipefail

usage() {
  printf 'Usage: %s publish|verify|survivor-check <run-id> <host-id> <peer-id>\n' "$0" >&2
  printf '       %s lock-hold <run-id> <host-id> <peer-id> <seconds>\n' "$0" >&2
  printf '       %s lock-check <run-id> <host-id> <peer-id>\n' "$0" >&2
  printf 'Requires MUSEHUB_GIT_ROOT and MUSEHUB_ARTIFACT_ROOT on shared mounts.\n' >&2
  exit 2
}

[[ $# -eq 4 || $# -eq 5 ]] || usage
MODE="$1"
RUN_ID="$2"
HOST_ID="$3"
PEER_ID="$4"
HOLD_SECONDS="${5:-0}"
case "$MODE" in
  publish|verify|survivor-check|lock-check) [[ $# -eq 4 ]] || usage ;;
  lock-hold)
    [[ $# -eq 5 && "$HOLD_SECONDS" =~ ^[0-9]+$ ]] || usage
    (( HOLD_SECONDS >= 10 && HOLD_SECONDS <= 300 )) || usage
    ;;
  *) usage ;;
esac
for value in "$RUN_ID" "$HOST_ID" "$PEER_ID"; do
  [[ "$value" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$ ]] || usage
done
[[ "$HOST_ID" != "$PEER_ID" ]] || { echo "host-id and peer-id must differ" >&2; exit 2; }
GIT_ROOT="${MUSEHUB_GIT_ROOT:?set MUSEHUB_GIT_ROOT to the shared Git mount}"
ARTIFACT_ROOT="${MUSEHUB_ARTIFACT_ROOT:?set MUSEHUB_ARTIFACT_ROOT to the shared artifact mount}"

probe_root() {
  local root="$1"
  local label="$2"
  local probe_dir="$root/.musehub-shared-storage-probe/$RUN_ID"
  mkdir -p "$probe_dir"
  printf '%s\n' "musehub-shared-storage-v1|$RUN_ID|$HOST_ID|$label" \
    > "$probe_dir/$HOST_ID.tmp"
  mv "$probe_dir/$HOST_ID.tmp" "$probe_dir/$HOST_ID.marker"
  if [[ "$MODE" == verify || "$MODE" == survivor-check ]]; then
    local peer_marker="$probe_dir/$PEER_ID.marker"
    [[ -f "$peer_marker" ]] || {
      echo "$label shared mount cannot see peer marker: $peer_marker" >&2
      return 1
    }
    local expected="musehub-shared-storage-v1|$RUN_ID|$PEER_ID|$label"
    local expected_sha observed_sha observed
    expected_sha="$(printf '%s\n' "$expected" | sha256sum | cut -d ' ' -f 1)"
    observed_sha="$(sha256sum "$peer_marker" | cut -d ' ' -f 1)"
    observed="$(<"$peer_marker")"
    [[ "$observed" == "$expected" && "$observed_sha" == "$expected_sha" ]] || {
      echo "$label peer marker content mismatch" >&2
      return 1
    }
    printf '%s shared visibility and atomic rename: ok\n' "$label"
  else
    printf '%s marker published for host %s\n' "$label" "$HOST_ID"
  fi
}

probe_lock_hold() {
  local root="$1"
  local probe_dir="$root/.musehub-shared-storage-probe/$RUN_ID"
  local lock_file="$probe_dir/.cross-host-lock"
  local marker="$probe_dir/$HOST_ID.lock-held"
  mkdir -p "$probe_dir"
  rm -f "$marker"
  (
    exec 9>"$lock_file"
    flock -n -x 9 || exit 1
    : > "$marker"
    sleep "$HOLD_SECONDS"
    rm -f "$marker"
  ) &
  HOLD_PIDS+=("$!")
}

probe_lock_check() {
  local root="$1"
  local label="$2"
  local probe_dir="$root/.musehub-shared-storage-probe/$RUN_ID"
  local lock_file="$probe_dir/.cross-host-lock"
  local marker="$probe_dir/$PEER_ID.lock-held"
  local deadline=$((SECONDS + 30))
  while [[ ! -f "$marker" ]]; do
    (( SECONDS < deadline )) || {
      echo "$label peer lock-holder marker did not appear" >&2
      return 1
    }
    sleep 0.2
  done
  exec 9>"$lock_file"
  if flock -n -x 9; then
    echo "$label advisory lock was available while peer held it" >&2
    return 1
  else
    local status=$?
    [[ "$status" -eq 1 ]] || {
      echo "$label advisory lock probe failed with status $status" >&2
      return 1
    }
  fi
  printf '%s cross-host advisory lock: ok\n' "$label"
}

probe_survivor_lock() {
  local root="$1"
  local label="$2"
  local lock_file="$root/.musehub-shared-storage-probe/$RUN_ID/.cross-host-lock"
  exec 9>"$lock_file"
  if ! flock -n -x 9; then
    echo "$label advisory lock remained held after the peer host was stopped" >&2
    return 1
  fi
  flock -u 9
  printf '%s survivor write and released lock: ok\n' "$label"
}

case "$MODE" in
  publish|verify|survivor-check)
    probe_root "$GIT_ROOT" git
    probe_root "$ARTIFACT_ROOT" artifacts
    if [[ "$MODE" == survivor-check ]]; then
      probe_survivor_lock "$GIT_ROOT" git
      probe_survivor_lock "$ARTIFACT_ROOT" artifacts
      printf 'shared_storage_survivor_check=ok survivor=%s stopped_peer=%s\n' "$HOST_ID" "$PEER_ID"
    fi
    ;;
  lock-hold)
    command -v flock >/dev/null 2>&1 || { echo "flock is required" >&2; exit 1; }
    HOLD_PIDS=()
    probe_lock_hold "$GIT_ROOT"
    probe_lock_hold "$ARTIFACT_ROOT"
    local_deadline=$((SECONDS + 10))
    for root in "$GIT_ROOT" "$ARTIFACT_ROOT"; do
      marker="$root/.musehub-shared-storage-probe/$RUN_ID/$HOST_ID.lock-held"
      while [[ ! -f "$marker" ]]; do
        (( SECONDS < local_deadline )) || { echo "failed to acquire shared lock on $root" >&2; exit 1; }
        sleep 0.1
      done
    done
    printf 'cross-host locks held for %s seconds; run lock-check on peer %s now\n' "$HOLD_SECONDS" "$PEER_ID"
    for pid in "${HOLD_PIDS[@]}"; do wait "$pid"; done
    ;;
  lock-check)
    command -v flock >/dev/null 2>&1 || { echo "flock is required" >&2; exit 1; }
    probe_lock_check "$GIT_ROOT" git
    probe_lock_check "$ARTIFACT_ROOT" artifacts
    ;;
esac
