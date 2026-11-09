#!/usr/bin/env bash
# Product acceptance: migrations up → schema check → product_slice → down → up (PostgreSQL).
set -euo pipefail
cd "$(dirname "$0")/.."

DATABASE_URL="${MUSEHUB_TEST_DATABASE_URL:-${DATABASE_URL:-}}"
if [[ -z "$DATABASE_URL" ]]; then
  echo "set MUSEHUB_TEST_DATABASE_URL or DATABASE_URL" >&2
  exit 1
fi

export MUSEHUB_GIT_ROOT="${MUSEHUB_GIT_ROOT:-/tmp/musehub-acceptance-git}"
export MUSEHUB_SESSION_SECRET="${MUSEHUB_SESSION_SECRET:-acceptance-session-secret-32chars}"
export MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD="${MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD:-Acceptance-Bootstrap-Strong-1!}"
mkdir -p "$MUSEHUB_GIT_ROOT"
export MUSEHUB_TEST_DATABASE_URL="$DATABASE_URL"

cargo build -p musehub-agent-adapter --bin musehub-agent-reference -q

echo "=== migrate up ==="
cargo run -p musehub-migrate -- up migrations "$DATABASE_URL"

echo "=== product_slice ==="
cargo test -p musehub-app --test product_slice -- --test-threads=1

echo "=== migrate down ==="
cargo run -p musehub-migrate -- down migrations "$DATABASE_URL"

echo "=== migrate up (again) ==="
cargo run -p musehub-migrate -- up migrations "$DATABASE_URL"

echo "acceptance=ok"
