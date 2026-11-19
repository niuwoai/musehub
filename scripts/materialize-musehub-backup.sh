#!/usr/bin/env bash
# Download and validate one immutable remote MuseHub backup snapshot.
set -euo pipefail

usage() {
  printf 'Usage: %s <rclone-remote:snapshot-path> <new-local-directory>\n' "$0" >&2
  exit 2
}

[[ $# -eq 2 ]] || usage
REMOTE_SNAPSHOT="$1"
DESTINATION="${2%/}"
[[ -n "$REMOTE_SNAPSHOT" && -n "$DESTINATION" ]] || usage
[[ "$REMOTE_SNAPSHOT" != *$'\n'* && "$REMOTE_SNAPSHOT" != *$'\r'* ]] || usage
[[ ! -e "$DESTINATION" ]] || { printf 'destination already exists: %s\n' "$DESTINATION" >&2; exit 1; }
command -v rclone >/dev/null 2>&1 || { echo "rclone is required" >&2; exit 1; }
command -v python3 >/dev/null 2>&1 || { echo "python3 is required" >&2; exit 1; }
command -v tar >/dev/null 2>&1 || { echo "tar is required" >&2; exit 1; }

PARENT="$(dirname "$DESTINATION")"
mkdir -p "$PARENT"
STAGING="$(mktemp -d "$PARENT/.musehub-restore.XXXXXX")"
cleanup() {
  if [[ -n "${STAGING:-}" && -d "$STAGING" ]]; then
    rm -rf -- "$STAGING"
  fi
}
trap cleanup EXIT

printf '=== downloading remote backup snapshot ===\n'
rclone copy "$REMOTE_SNAPSHOT" "$STAGING" --create-empty-src-dirs
[[ -f "$STAGING/manifest.json" ]] || { echo "remote snapshot is missing manifest.json" >&2; exit 1; }

printf '=== comparing downloaded files with remote snapshot ===\n'
rclone check "$REMOTE_SNAPSHOT" "$STAGING" --download

python3 -I -c '
import json, pathlib, sys, tarfile
root = pathlib.Path(sys.argv[1])
with (root / "manifest.json").open(encoding="utf-8") as stream:
    manifest = json.load(stream)
for field in ("database_dump", "git_archive"):
    name = manifest.get(field)
    if not isinstance(name, str) or not name or pathlib.PurePosixPath(name).name != name:
        raise SystemExit(f"invalid manifest field: {field}")
    path = root / name
    if path.is_symlink() or not path.is_file():
        raise SystemExit(f"missing or unsafe backup file: {field}")
for field in ("git_archive", "artifacts_archive"):
    name = manifest.get(field)
    if not name:
        continue
    if not isinstance(name, str) or pathlib.PurePosixPath(name).name != name:
        raise SystemExit(f"invalid manifest field: {field}")
    archive = root / name
    if archive.is_symlink() or not archive.is_file():
        raise SystemExit(f"missing or unsafe backup file: {field}")
    with tarfile.open(archive, "r:gz") as bundle:
        for member in bundle.getmembers():
            path = pathlib.PurePosixPath(member.name)
            if path.is_absolute() or ".." in path.parts:
                raise SystemExit(f"unsafe path in {field}: {member.name}")
            if member.issym() or member.islnk():
                raise SystemExit(f"links are not accepted in {field}: {member.name}")
            elif not (member.isfile() or member.isdir()):
                raise SystemExit(f"unsupported archive entry in {field}: {member.name}")
print("manifest_and_archives=ok")
' "$STAGING"

mv "$STAGING" "$DESTINATION"
STAGING=""
printf 'remote_backup_materialized=ok source=%s destination=%s\n' "$REMOTE_SNAPSHOT" "$DESTINATION"
