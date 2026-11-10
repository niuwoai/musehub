#!/usr/bin/env bash
# Retry yarn install for CI when lockfile resolved URLs use slow mirrors.
set -euo pipefail
WEB_DIR="${1:-web}"
cd "$(dirname "$0")/.."
attempt=1
while [[ "$attempt" -le 5 ]]; do
  if yarn --cwd "$WEB_DIR" install --frozen-lockfile --network-timeout 300000; then
    exit 0
  fi
  echo "yarn install attempt $attempt failed" >&2
  attempt=$((attempt + 1))
  sleep "$((attempt * 5))"
done
echo "yarn install failed after 5 attempts" >&2
exit 1
