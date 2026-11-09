#!/usr/bin/env bash
# Subprocess: server + runnerd + slow adapter; HTTP cancel kills process group; no ready artifact.
set -euo pipefail
unset MUSEHUB_GIT_ROOT MUSEHUB_ARTIFACT_ROOT || true
# shellcheck source=lib/remote-runner-e2e-common.sh
source "$(dirname "$0")/lib/remote-runner-e2e-common.sh"
remote_runner_e2e_init
trap remote_runner_e2e_cleanup EXIT

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SLOW_BIN="${CARGO_TARGET_DIR:-$ROOT/target}/debug/musehub-pipeline-slow"
export MUSEHUB_PIPELINE_HOLD_SECS="${MUSEHUB_PIPELINE_HOLD_SECS:-45}"

remote_runner_start_server
remote_runner_bootstrap_repo "cancel-pipe-$(e2e_uuid | cut -c1-8)" "cancel-runner-$(e2e_uuid | cut -c1-8)" "Cancel-Runner-Member-1!"

remote_runner_start_runnerd "$SLOW_BIN"
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
  echo "expected running, got $(remote_runner_poll_state "$E2E_RUN_ID")" >&2
  exit 1
}

ADAPTER_PID=""
for _ in $(seq 1 60); do
  ADAPTER_PID=$(remote_runner_adapter_pid "$E2E_RUN_ID" || true)
  [[ -n "$ADAPTER_PID" ]] && break
  sleep 0.5
done
[[ -n "$ADAPTER_PID" ]] || { echo "slow adapter not found for run $E2E_RUN_ID" >&2; exit 1; }
kill -0 "$ADAPTER_PID"

curl -sf -X POST "http://127.0.0.1:${API_PORT}/api/v1/orgs/demo/repos/${REPO_SLUG}/pipeline-runs/$E2E_RUN_ID/cancel" \
  -H "Cookie: musehub_session=$COOKIE" >/dev/null

for _ in $(seq 1 60); do
  st=$(remote_runner_poll_state "$E2E_RUN_ID")
  if [[ "$st" == "canceled" ]]; then
    break
  fi
  sleep 0.5
done
[[ "$(remote_runner_poll_state "$E2E_RUN_ID")" == "canceled" ]] || {
  echo "expected canceled, got $(remote_runner_poll_state "$E2E_RUN_ID")" >&2
  exit 1
}

sleep 1
if kill -0 "$ADAPTER_PID" 2>/dev/null; then
  echo "adapter pid $ADAPTER_PID still alive after cancel" >&2
  kill -9 "$ADAPTER_PID" || true
  exit 1
fi

ready_count=$(remote_runner_count_ready_artifacts "$E2E_RUN_ID")
[[ "$ready_count" == "0" ]] || {
  echo "expected 0 ready artifacts, got $ready_count" >&2
  exit 1
}

rel_count=$(psql "$DATABASE_URL" -Atc "SELECT COUNT(*) FROM releases WHERE pipeline_run_id='$E2E_RUN_ID'")
[[ "$rel_count" == "0" ]] || { echo "unexpected release rows: $rel_count" >&2; exit 1; }

dep_count=$(psql "$DATABASE_URL" -Atc "SELECT COUNT(*) FROM deployments d JOIN releases r ON d.release_id = r.id WHERE r.pipeline_run_id='$E2E_RUN_ID'")
[[ "$dep_count" == "0" ]] || { echo "unexpected deployment rows: $dep_count" >&2; exit 1; }

echo "runner_cancel_subprocess_e2e=ok"
