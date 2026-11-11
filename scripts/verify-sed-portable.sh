#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=lib/sed-portable.sh
source "$ROOT/scripts/lib/sed-portable.sh"
TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT
printf '%s\n' 'POSTGRES_PASSWORD=old' 'MUSEHUB_SESSION_SECRET=old' >"$TMP"
sed_inplace 's/^POSTGRES_PASSWORD=.*/POSTGRES_PASSWORD=new/' "$TMP"
grep -q '^POSTGRES_PASSWORD=new' "$TMP"
sed_inplace 's/^MUSEHUB_SESSION_SECRET=.*/MUSEHUB_SESSION_SECRET=secret42/' "$TMP"
grep -q '^MUSEHUB_SESSION_SECRET=secret42' "$TMP"
echo "verify_sed_portable=ok uname=$(uname -s)"
