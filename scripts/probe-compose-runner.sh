#!/usr/bin/env bash
# Records whether isolated compose-runner can execute on this host (evidence; non-fatal in CI).
set -euo pipefail
cd "$(dirname "$0")/.."

BIN="${CARGO_BIN_EXE_musehub-compose-runner:-target/debug/musehub-compose-runner}"
if [[ ! -x "$BIN" ]]; then
  cargo build -p musehub-agent-adapter --bin musehub-compose-runner -q
  BIN=target/debug/musehub-compose-runner
fi

if ! docker info >/dev/null 2>&1; then
  echo "compose_runner_probe=docker_unavailable"
  exit 0
fi

set +e
OUT=$(MUSEHUB_DEPLOY_HEALTH_TARGET=mock://healthy "$BIN" 2>&1)
CODE=$?
set -e
echo "compose_runner_probe exit=$CODE"
echo "$OUT" | head -20
if [[ "$CODE" -eq 0 ]]; then
  echo "compose_runner_probe=ok"
elif [[ "$CODE" -eq 2 ]]; then
  echo "compose_runner_probe=docker_unavailable"
else
  echo "compose_runner_probe=failed"
fi
