#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

cargo test --workspace \
  --exclude musehub-container-probe \
  --exclude musehub-rootless-probe \
  --exclude musehub-ssh-probe \
  --exclude musehub-migrate \
  --exclude musehub-app \
  -- --test-threads=1

cargo test -p musehub-app --lib -- --test-threads=1
cargo test -p musehub-agent-adapter -- --test-threads=1
cargo test -p musehub-mcp-probe -- --test-threads=1

# product_slice integration: CI job `product slice integration` (needs Postgres).
