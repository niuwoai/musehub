#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

command -v /usr/bin/timeout >/dev/null || { echo "/usr/bin/timeout required" >&2; exit 1; }
PORTABLE_TEST_TIMEOUT_SEC="${PORTABLE_TEST_TIMEOUT_SEC:-2400}"

echo "=== portable workspace tests (timeout=${PORTABLE_TEST_TIMEOUT_SEC}s) ==="
/usr/bin/timeout -k 60 "${PORTABLE_TEST_TIMEOUT_SEC}" cargo test --workspace \
  --exclude musehub-container-probe \
  --exclude musehub-rootless-probe \
  --exclude musehub-ssh-probe \
  --exclude musehub-migrate \
  --exclude musehub-app \
  -- --test-threads=1

/usr/bin/timeout -k 60 900 cargo test -p musehub-app --lib -- --test-threads=1
/usr/bin/timeout -k 60 600 cargo test -p musehub-agent-adapter -- --test-threads=1
/usr/bin/timeout -k 60 300 cargo test -p musehub-mcp-probe -- --test-threads=1

# product_slice integration: CI job `product slice integration` (needs Postgres).
