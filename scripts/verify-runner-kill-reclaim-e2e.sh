#!/usr/bin/env bash
# Real SIGKILL on runnerd → lease TTL → second worker reclaims (no DB state fakery).
# Side-effect uniqueness (one ready artifact): `runner_reclaim_build_e2e` (same lease/reclaim semantics).
set -euo pipefail
unset MUSEHUB_GIT_ROOT MUSEHUB_ARTIFACT_ROOT || true
# shellcheck source=lib/remote-runner-e2e-common.sh
source "$(dirname "$0")/lib/remote-runner-e2e-common.sh"
remote_runner_e2e_init
trap remote_runner_e2e_cleanup EXIT

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SLOW_BIN="${CARGO_TARGET_DIR:-$ROOT/target}/debug/musehub-pipeline-slow"
REF_BIN="${CARGO_TARGET_DIR:-$ROOT/target}/debug/musehub-pipeline-reference"
export MUSEHUB_LEASE_SECS="${MUSEHUB_LEASE_SECS:-10}"
export MUSEHUB_RUNNER_LEASE_RENEW_SECS="${MUSEHUB_RUNNER_LEASE_RENEW_SECS:-3}"
export MUSEHUB_PIPELINE_HOLD_SECS="${MUSEHUB_PIPELINE_HOLD_SECS:-60}"

INSTANCE_A=$(e2e_uuid)
INSTANCE_B=$(e2e_uuid)

remote_runner_start_server
remote_runner_bootstrap_repo "kill-pipe-$(e2e_uuid | cut -c1-8)" "kill-runner-$(e2e_uuid | cut -c1-8)" "Kill-Runner-Member-1!"

remote_runner_start_runnerd "$SLOW_BIN" "$INSTANCE_A"
export RUNNER_PID
sleep 1
remote_runner_enqueue_validation

for _ in $(seq 1 90); do
  st=$(remote_runner_poll_state "$E2E_RUN_ID")
  if [[ "$st" == "running" ]]; then
    break
  fi
  sleep 0.5
done
[[ "$(remote_runner_poll_state "$E2E_RUN_ID")" == "running" ]] || {
  echo "expected running before kill, got $(remote_runner_poll_state "$E2E_RUN_ID")" >&2
  exit 1
}

sleep 2
kill -9 "$RUNNER_PID"
wait "$RUNNER_PID" 2>/dev/null || true
RUNNER_PID=""
echo "killed runnerd pid (instance A) with SIGKILL"

sleep_secs=$((MUSEHUB_LEASE_SECS + 6))
echo "waiting ${sleep_secs}s for lease TTL reclaim window"
sleep "$sleep_secs"

lease_expired=$(psql "$DATABASE_URL" -Atc "SELECT (lease_until < CURRENT_TIMESTAMP) FROM pipeline_runs WHERE id='$E2E_RUN_ID'")
[[ "$lease_expired" == "t" ]] || {
  echo "lease_until not expired after TTL (worker killed, no DB fakery)" >&2
  exit 1
}

pkill -f musehub-pipeline-slow 2>/dev/null || true
export MUSEHUB_PIPELINE_HOLD_SECS=0
remote_runner_start_runnerd "$REF_BIN" "$INSTANCE_B"
RUNNER_PID_2=$RUNNER_PID
export RUNNER_PID_2

for _ in $(seq 1 60); do
  holder=$(psql "$DATABASE_URL" -Atc "SELECT lease_holder::text FROM pipeline_runs WHERE id='$E2E_RUN_ID'")
  if [[ "$holder" == "$INSTANCE_B" ]]; then
    break
  fi
  sleep 1
done
[[ "$(psql "$DATABASE_URL" -Atc "SELECT lease_holder::text FROM pipeline_runs WHERE id='$E2E_RUN_ID'")" == "$INSTANCE_B" ]] || {
  echo "expected second worker to hold lease" >&2
  exit 1
}

export MUSEHUB_TEST_DATABASE_URL="$DATABASE_URL"
cargo test -q -p musehub-app --test runner_reclaim_build_e2e -- --test-threads=1

echo "runner_kill_reclaim_subprocess_e2e=ok"
