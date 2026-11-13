#!/usr/bin/env bash
# Offline regression: compose_dc_timeout must pass argv split (spaces, -f paths) to compose.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=../lib/compose-dc-timeout.sh
source "$ROOT/scripts/lib/compose-dc-timeout.sh"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
LOG="$TMP/argv.log"
STUB="$TMP/docker-stub"
cat >"$STUB" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$@" >>"${COMPOSE_DC_STUB_LOG:?}"
exit 0
STUB
chmod +x "$STUB"

export COMPOSE_DC_STUB_LOG="$LOG"
export COMPOSE_PROJECT_NAME=stubproj
export COMPOSE_GIT_TIMEOUT_SEC=30

# Simulate stack parent: each COMPOSE_BASE word separately quoted.
MUSEHUB_STACK_DC=$(printf '%q ' "$STUB" compose -f "$TMP/spaced compose/file.yml" -p myproj)

compose_dc_timeout exec -T musehub-server git --git-dir=/var/lib/musehub/git/x.git rev-parse refs/heads/main

[[ -f "$LOG" ]] || { echo "stub log missing" >&2; exit 1; }
ARGS="$(tr '\n' ' ' <"$LOG")"

echo "captured: $ARGS"

[[ "$ARGS" == *compose* ]] || { echo "compose subcommand missing" >&2; exit 1; }
[[ "$ARGS" == *"spaced compose/file.yml"* ]] || { echo "compose file path not preserved as token" >&2; exit 1; }
[[ "$ARGS" == *exec* ]] || { echo "exec missing" >&2; exit 1; }
[[ "$ARGS" == *musehub-server* ]] || { echo "service missing" >&2; exit 1; }
[[ "$ARGS" == *rev-parse* ]] || { echo "rev-parse missing" >&2; exit 1; }

# Broken quoting would pass one giant token containing spaces without separate -f path.
bad_line="$(grep -c 'compose -f' <<<"$ARGS" || true)"
[[ "$bad_line" -ge 1 ]] || { echo "expected split argv, got single blob: $ARGS" >&2; exit 1; }

echo "compose_dc_timeout_regression=ok"
