#!/usr/bin/env bash
# One-way asynchronous mirror of MuseHub Git and artifact files over SSH/rsync.
set -euo pipefail

usage() {
  printf 'Usage: %s continuous|quiesced-final|verify\n' "$0" >&2
  printf 'Requires MUSEHUB_GIT_ROOT, MUSEHUB_ARTIFACT_ROOT, MUSEHUB_STORAGE_MIRROR_TARGET, MUSEHUB_STORAGE_MIRROR_REMOTE_ROOT, MUSEHUB_STORAGE_MIRROR_STATE_DIR.\n' >&2
  exit 2
}

[[ $# -eq 1 ]] || usage
MODE="$1"
case "$MODE" in
  continuous) CHECKSUM_ARGS=(--checksum) ;;
  verify) CHECKSUM_ARGS=(--checksum --delete-delay --dry-run --itemize-changes '--out-format=%i %n') ;;
  quiesced-final)
    [[ "${MUSEHUB_STORAGE_SOURCE_QUIESCED:-}" == "YES" ]] || {
      echo "stop MuseHub, SSH, and Runner writers before setting MUSEHUB_STORAGE_SOURCE_QUIESCED=YES" >&2
      exit 1
    }
    [[ "${MUSEHUB_STORAGE_MIRROR_ALLOW_DELETE:-}" == "YES" ]] || {
      echo "set MUSEHUB_STORAGE_MIRROR_ALLOW_DELETE=YES only for the dedicated mirror target after verifying its path" >&2
      exit 1
    }
    CHECKSUM_ARGS=(--checksum --delete-delay)
    ;;
  *) usage ;;
esac

GIT_ROOT="${MUSEHUB_GIT_ROOT:?set MUSEHUB_GIT_ROOT}"
ARTIFACT_ROOT="${MUSEHUB_ARTIFACT_ROOT:?set MUSEHUB_ARTIFACT_ROOT}"
TARGET="${MUSEHUB_STORAGE_MIRROR_TARGET:?set MUSEHUB_STORAGE_MIRROR_TARGET as user@host}"
REMOTE_ROOT="${MUSEHUB_STORAGE_MIRROR_REMOTE_ROOT:?set MUSEHUB_STORAGE_MIRROR_REMOTE_ROOT}"
STATE_DIR="${MUSEHUB_STORAGE_MIRROR_STATE_DIR:?set MUSEHUB_STORAGE_MIRROR_STATE_DIR}"
LOCK_FILE="${MUSEHUB_STORAGE_MIRROR_LOCK_FILE:-$STATE_DIR/mirror.lock}"
BARRIER_ID="${MUSEHUB_STORAGE_MIRROR_BARRIER_ID:-}"
BARRIER_LSN="${MUSEHUB_STORAGE_MIRROR_BARRIER_LSN:-}"
LOCAL_TARGET=0
if [[ "$TARGET" == "local" ]]; then
  [[ "${MUSEHUB_STORAGE_MIRROR_ALLOW_LOCAL:-}" == "YES" ]] || {
    echo "local mirror targets are restricted to explicit isolated acceptance runs" >&2
    exit 2
  }
  LOCAL_TARGET=1
else
  [[ "$TARGET" =~ ^[A-Za-z0-9._-]+@[A-Za-z0-9.-]+$ ]] || {
    echo "mirror target must be a simple user@host value" >&2
    exit 2
  }
fi
if [[ "$MODE" == quiesced-final ]]; then
  [[ "$BARRIER_ID" =~ ^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$ ]] || {
    echo "quiesced-final requires MUSEHUB_STORAGE_MIRROR_BARRIER_ID from create-musehub-dr-barrier.sh" >&2
    exit 2
  }
  [[ "$BARRIER_LSN" =~ ^[0-9A-Fa-f]+/[0-9A-Fa-f]+$ ]] || {
    echo "quiesced-final requires MUSEHUB_STORAGE_MIRROR_BARRIER_LSN from create-musehub-dr-barrier.sh" >&2
    exit 2
  }
fi
[[ "$REMOTE_ROOT" =~ ^/[A-Za-z0-9._/-]+$ && "$REMOTE_ROOT" != "/" && "$REMOTE_ROOT" != *".."* && "$REMOTE_ROOT" != *"//"* ]] || {
  echo "remote root must be a dedicated absolute path without parent traversal" >&2
  exit 2
}
[[ -d "$GIT_ROOT" && ! -L "$GIT_ROOT" ]] || { echo "Git root must be an existing, non-symlink directory" >&2; exit 1; }
[[ -d "$ARTIFACT_ROOT" && ! -L "$ARTIFACT_ROOT" ]] || { echo "artifact root must be an existing, non-symlink directory" >&2; exit 1; }
command -v rsync >/dev/null 2>&1 || { echo "rsync is required" >&2; exit 1; }
command -v flock >/dev/null 2>&1 || { echo "flock is required" >&2; exit 1; }
if (( LOCAL_TARGET == 0 )); then
  command -v ssh >/dev/null 2>&1 || { echo "ssh is required" >&2; exit 1; }
fi
mkdir -p "$STATE_DIR"
exec 9>"$LOCK_FILE"
flock -n -x 9 || { echo "another storage mirror pass is running" >&2; exit 1; }

SSH_COMMAND='ssh -o BatchMode=yes -o StrictHostKeyChecking=yes'
if (( LOCAL_TARGET == 1 )); then
  [[ -d "$REMOTE_ROOT" && ! -L "$REMOTE_ROOT" && -f "$REMOTE_ROOT/.musehub-mirror-target" && ! -L "$REMOTE_ROOT/.musehub-mirror-target" && -d "$REMOTE_ROOT/git" && ! -L "$REMOTE_ROOT/git" && -d "$REMOTE_ROOT/artifacts" && ! -L "$REMOTE_ROOT/artifacts" && -d "$REMOTE_ROOT/.musehub-mirror-status" && ! -L "$REMOTE_ROOT/.musehub-mirror-status" ]] || {
    echo "cannot validate isolated local mirror target directories and marker" >&2
    exit 1
  }
  remote_marker="$(<"$REMOTE_ROOT/.musehub-mirror-target")"
else
  remote_marker="$(ssh -o BatchMode=yes -o StrictHostKeyChecking=yes "$TARGET" "test -d '$REMOTE_ROOT' && test ! -L '$REMOTE_ROOT' && test -f '$REMOTE_ROOT/.musehub-mirror-target' && test ! -L '$REMOTE_ROOT/.musehub-mirror-target' && test -d '$REMOTE_ROOT/git' && test ! -L '$REMOTE_ROOT/git' && test -d '$REMOTE_ROOT/artifacts' && test ! -L '$REMOTE_ROOT/artifacts' && test -d '$REMOTE_ROOT/.musehub-mirror-status' && test ! -L '$REMOTE_ROOT/.musehub-mirror-status' && cat -- '$REMOTE_ROOT/.musehub-mirror-target'")" || {
    echo "cannot validate mirror target directories and marker; verify the dedicated destination path and SSH access" >&2
    exit 1
  }
fi
[[ "$remote_marker" == "musehub-storage-mirror-target-v1" ]] || {
  echo "remote path is not marked as a MuseHub storage mirror target" >&2
  exit 1
}
RSYNC_ARGS=(--archive --hard-links --numeric-ids --no-owner --delay-updates --human-readable)

mirror_tree() {
  local label="$1" source="$2" destination="$3"
  local output
  local -a exclude_args=()
  if [[ "$label" == artifacts ]]; then
    exclude_args=(--exclude=/.workflow-cache/***)
  fi
  printf 'storage_mirror=syncing component=%s mode=%s\n' "$label" "$MODE"
  local target_path
  if (( LOCAL_TARGET == 1 )); then
    target_path="$REMOTE_ROOT/$destination/"
  else
    target_path="$TARGET:$REMOTE_ROOT/$destination/"
  fi
  if [[ "$MODE" == verify ]]; then
    if (( LOCAL_TARGET == 1 )); then
      output="$(rsync "${RSYNC_ARGS[@]}" "${CHECKSUM_ARGS[@]}" "${exclude_args[@]}" -- "$source/" "$target_path")"
    else
      output="$(rsync "${RSYNC_ARGS[@]}" "${CHECKSUM_ARGS[@]}" "${exclude_args[@]}" -e "$SSH_COMMAND" -- "$source/" "$target_path")"
    fi
    if [[ -n "$output" ]]; then
      printf '%s\n' "$output" >&2
      echo "storage mirror differs for component=$label" >&2
      return 1
    fi
  elif (( LOCAL_TARGET == 1 )); then
    rsync "${RSYNC_ARGS[@]}" "${CHECKSUM_ARGS[@]}" "${exclude_args[@]}" -- "$source/" "$target_path"
  else
    rsync "${RSYNC_ARGS[@]}" "${CHECKSUM_ARGS[@]}" "${exclude_args[@]}" -e "$SSH_COMMAND" -- "$source/" "$target_path"
  fi
}

mirror_tree git "$GIT_ROOT" git
mirror_tree artifacts "$ARTIFACT_ROOT" artifacts
if [[ "$MODE" == verify ]]; then
  printf 'storage_mirror_verify=ok target=%s\n' "$TARGET"
  exit 0
fi

completed_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
source_host="$(hostname -s | tr -cd 'A-Za-z0-9._-')"
[[ -n "$source_host" ]] || { echo "could not determine a safe source hostname" >&2; exit 1; }
status_tmp="$(mktemp "$STATE_DIR/status.XXXXXX")"
trap 'rm -f "$status_tmp"' EXIT
{
  printf 'format=musehub-storage-mirror-v1\n'
  printf 'source_host=%s\n' "$source_host"
  printf 'completed_at=%s\n' "$completed_at"
  printf 'mode=%s\n' "$MODE"
  if [[ "$MODE" == quiesced-final ]]; then
    printf 'barrier_id=%s\n' "$BARRIER_ID"
    printf 'barrier_lsn=%s\n' "$BARRIER_LSN"
  fi
} >"$status_tmp"
if (( LOCAL_TARGET == 1 )); then
  rsync --archive --delay-updates -- "$status_tmp" "$REMOTE_ROOT/.musehub-mirror-status/latest"
else
  rsync --archive --delay-updates -e "$SSH_COMMAND" -- "$status_tmp" "$TARGET:$REMOTE_ROOT/.musehub-mirror-status/latest"
fi
mv -f "$status_tmp" "$STATE_DIR/latest"
printf 'storage_mirror=ok source=%s completed_at=%s mode=%s\n' "$source_host" "$completed_at" "$MODE"
