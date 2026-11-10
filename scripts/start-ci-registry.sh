#!/usr/bin/env bash
# Ephemeral local OCI registry for CI / E2E (no external accounts).
set -euo pipefail
REGISTRY_NAME="${MUSEHUB_CI_REGISTRY_NAME:-musehub-ci-registry}"
REGISTRY_PORT="${MUSEHUB_OCI_REGISTRY_PORT:-5000}"
if docker ps --format '{{.Names}}' | grep -qx "$REGISTRY_NAME"; then
  echo "ci_registry=already_running name=$REGISTRY_NAME port=$REGISTRY_PORT"
  exit 0
fi
if docker ps -a --format '{{.Names}}' | grep -qx "$REGISTRY_NAME"; then
  docker rm -f "$REGISTRY_NAME" >/dev/null
fi
docker run -d --name "$REGISTRY_NAME" -p "127.0.0.1:${REGISTRY_PORT}:5000" registry:2 >/dev/null
for _ in $(seq 1 30); do
  if curl -sf "http://127.0.0.1:${REGISTRY_PORT}/v2/" >/dev/null 2>&1; then
    echo "ci_registry=ready host=127.0.0.1:${REGISTRY_PORT}"
    exit 0
  fi
  sleep 1
done
echo "ci_registry=fail registry did not become ready" >&2
exit 1
