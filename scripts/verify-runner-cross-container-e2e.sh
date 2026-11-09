#!/usr/bin/env bash
# Cross-container (not dual-host): shared DB + git/artifact volumes; API and runnerd in separate containers.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export DATABASE_URL="${MUSEHUB_TEST_DATABASE_URL:-${DATABASE_URL:-}}"
test -n "$DATABASE_URL" || { echo "MUSEHUB_TEST_DATABASE_URL or DATABASE_URL required" >&2; exit 1; }

if ! command -v docker >/dev/null 2>&1; then
  echo "docker not available; skip cross-container runner e2e" >&2
  exit 77
fi

RUN_DOCKER() {
  if groups | grep -qw docker; then docker "$@"; else sg docker -c "docker $*"; fi
}

command -v jq >/dev/null || { echo "jq required" >&2; exit 1; }
cargo build -q -p musehub-server -p musehub-runnerd -p musehub-agent-adapter
cargo run -q -p musehub-migrate -- up migrations "$DATABASE_URL" >/dev/null

NET="musehub-xc-${RANDOM}"
VOL="$(mktemp -d)"
GIT_ROOT="$VOL/git"
ARTIFACT_ROOT="$VOL/artifacts"
mkdir -p "$GIT_ROOT" "$ARTIFACT_ROOT"

BIN_DIR="${CARGO_TARGET_DIR:-$ROOT/target}/debug"
API_PORT=$((53000 + RANDOM % 5000))
export MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD="${MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD:-Xc-Runner-E2E-Strong-1!}"
export MUSEHUB_SESSION_SECRET="${MUSEHUB_SESSION_SECRET:-$(openssl rand -hex 32)}"

DB_CONTAINER="${DATABASE_URL/localhost/host.docker.internal}"
DB_CONTAINER="${DB_CONTAINER/127.0.0.1/host.docker.internal}"

cleanup() {
  RUN_DOCKER rm -f "${API_CNAME:-}" "${RUN_CNAME:-}" 2>/dev/null || true
  RUN_DOCKER network rm "$NET" 2>/dev/null || true
  rm -rf "$VOL"
}
trap cleanup EXIT

RUN_DOCKER network create "$NET" >/dev/null
API_CNAME="mh-api-xc-${RANDOM}"
RUN_CNAME="mh-run-xc-${RANDOM}"
API_URL_CONTAINER="http://${API_CNAME}:${API_PORT}"

RUN_DOCKER run -d --name "$API_CNAME" --network "$NET" \
  -p "127.0.0.1:${API_PORT}:${API_PORT}" \
  --add-host=host.docker.internal:host-gateway \
  -v "$BIN_DIR:/opt/musehub:ro" \
  -v "$GIT_ROOT:/git" \
  -v "$ARTIFACT_ROOT:/artifacts" \
  -e DATABASE_URL="$DB_CONTAINER" \
  -e MUSEHUB_GIT_ROOT=/git \
  -e MUSEHUB_ARTIFACT_ROOT=/artifacts \
  -e MUSEHUB_SESSION_SECRET="$MUSEHUB_SESSION_SECRET" \
  -e MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD="$MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD" \
  -e MUSEHUB_PIPELINE_EXECUTOR=remote \
  -e MUSEHUB_BIND="0.0.0.0:${API_PORT}" \
  ubuntu:24.04 /opt/musehub/musehub-server >/dev/null

for _ in $(seq 1 80); do
  curl -sf "http://127.0.0.1:${API_PORT}/readyz" | jq -e '.checks.product_api == "ok"' >/dev/null && break
  sleep 0.5
done
curl -sf "http://127.0.0.1:${API_PORT}/readyz" | jq -e '.checks.product_api == "ok"' >/dev/null

export MUSEHUB_GIT_ROOT="$GIT_ROOT" MUSEHUB_ARTIFACT_ROOT="$ARTIFACT_ROOT" MUSEHUB_API_PORT="$API_PORT"
# shellcheck source=lib/remote-runner-e2e-common.sh
source "$ROOT/scripts/lib/remote-runner-e2e-common.sh"
export MUSEHUB_API_BASE="http://127.0.0.1:${API_PORT}"

COOKIE=$(remote_runner_login_cookie)
export COOKIE
remote_runner_bootstrap_repo "xc-pipe" "xc-runner" "Xc-Runner-Member-1!"

RUN_DOCKER run -d --name "$RUN_CNAME" --network "$NET" \
  --add-host=host.docker.internal:host-gateway \
  -v "$BIN_DIR:/opt/musehub:ro" \
  -v "$GIT_ROOT:/git" \
  -v "$ARTIFACT_ROOT:/artifacts" \
  -e MUSEHUB_API_BASE="$API_URL_CONTAINER" \
  -e DATABASE_URL="$DB_CONTAINER" \
  -e MUSEHUB_GIT_ROOT=/git \
  -e MUSEHUB_ARTIFACT_ROOT=/artifacts \
  -e MUSEHUB_PIPELINE_ADAPTER_BIN=/opt/musehub/musehub-pipeline-reference \
  -e MUSEHUB_RUNNER_PAT="$RUNNER_PAT" \
  -e MUSEHUB_RUNNER_POLL_SECS=1 \
  ubuntu:24.04 /opt/musehub/musehub-runnerd >/dev/null

remote_runner_enqueue_validation

for _ in $(seq 1 120); do
  st=$(remote_runner_poll_state "$E2E_RUN_ID")
  if [[ "$st" == "succeeded" ]]; then
    echo "runner_cross_container_e2e=ok (topology: two Docker containers + shared DB/volumes; not true dual-host)"
    exit 0
  fi
  sleep 1
done
echo "cross-container run did not succeed: $(remote_runner_poll_state "$E2E_RUN_ID")" >&2
exit 1
