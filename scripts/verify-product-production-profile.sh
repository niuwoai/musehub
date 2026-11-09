#!/usr/bin/env bash
# Fail-closed: production profile refuses mock compose (no silent fallback).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -q -p musehub-server
export DATABASE_URL="${DATABASE_URL:-postgres://invalid:invalid@127.0.0.1:1/none}"
export MUSEHUB_GIT_ROOT=/tmp/musehub-prod-profile-git
export MUSEHUB_SESSION_SECRET=prod-profile-test-session-32c
export MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD=Prod-Profile-Test-Strong-1!
export MUSEHUB_PRODUCT_PROFILE=production
export MUSEHUB_PIPELINE_EXECUTOR=remote
export MUSEHUB_AGENT_EXECUTOR=remote
export MUSEHUB_COMPOSE_ADAPTER_MODE=mock
mkdir -p "$MUSEHUB_GIT_ROOT"
set +e
timeout 3 "$PWD/target/debug/musehub-server" >/tmp/prod-profile-server.log 2>&1
ec=$?
set -e
if [[ "$ec" != "78" ]] && [[ "$ec" != "124" ]]; then
  echo "expected exit 78 from production profile validation, got $ec" >&2
  tail -5 /tmp/prod-profile-server.log >&2
  exit 1
fi
grep -q 'product profile validation failed' /tmp/prod-profile-server.log
echo "product_production_profile_fail_closed=ok"
