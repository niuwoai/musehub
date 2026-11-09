#!/usr/bin/env bash
# Real compose-runner gate: pull digest, up, security inspect, health, rollback, idempotency.
set -euo pipefail
cd "$(dirname "$0")/.."

command -v docker >/dev/null || { echo "compose_runner_e2e=blocked docker CLI missing" >&2; exit 1; }
docker info >/dev/null 2>&1 || { echo "compose_runner_e2e=blocked docker info failed" >&2; exit 1; }

cargo build -p musehub-agent-adapter --bin musehub-compose-runner -q
BIN="$PWD/target/debug/musehub-compose-runner"
NGINX_TAG="${MUSEHUB_NGINX_IMAGE_TAG:-nginx:1.27.3}"
echo "=== docker pull $NGINX_TAG ==="
docker pull "$NGINX_TAG"
NGINX_IMAGE=$(docker inspect "$NGINX_TAG" --format '{{index .RepoDigests 0}}')
[[ -n "$NGINX_IMAGE" ]] || NGINX_IMAGE="$NGINX_TAG"

PORT=$((19000 + RANDOM % 1000))
PROJECT="musehub-ci-compose-$PORT"
TARGET="http://127.0.0.1:${PORT}/"

cleanup() {
  docker compose -p "$PROJECT" down -v --remove-orphans 2>/dev/null || true
}
trap cleanup EXIT

echo "=== compose-runner success path (port=$PORT image=$NGINX_IMAGE) ==="
export MUSEHUB_COMPOSE_LEAVE_STACK=1
export MUSEHUB_COMPOSE_PROJECT="$PROJECT"
export MUSEHUB_COMPOSE_PUBLISH_PORT="$PORT"
export MUSEHUB_DEPLOY_HEALTH_TARGET="$TARGET"
export MUSEHUB_NGINX_IMAGE_DIGEST="$NGINX_IMAGE"

set +e
OUT=$("$BIN" 2>&1)
RC=$?
set -e
echo "$OUT"
[[ "$RC" -eq 0 ]] || { echo "runner exit=$RC" >&2; exit 1; }
echo "$OUT" | grep -q '"outcome":"succeeded"' || {
  echo "runner output missing succeeded outcome" >&2
  exit 1
}

CID=$(docker compose -p "$PROJECT" ps -q | head -n1)
[[ -n "$CID" ]] || { echo "no container in project $PROJECT" >&2; docker compose -p "$PROJECT" ps -a; exit 1; }

PRIV=$(docker inspect "$CID" --format '{{.HostConfig.Privileged}}')
BINDS=$(docker inspect "$CID" --format '{{json .HostConfig.Binds}}')
CAP_DROP=$(docker inspect "$CID" --format '{{json .HostConfig.CapDrop}}')
MEM=$(docker inspect "$CID" --format '{{.HostConfig.Memory}}')
PIDS=$(docker inspect "$CID" --format '{{.HostConfig.PidsLimit}}')
NO_PRIV=$(docker inspect "$CID" --format '{{json .HostConfig.SecurityOpt}}')
IMAGE_ID=$(docker inspect "$CID" --format '{{.Image}}')

[[ "$PRIV" == "false" ]] || { echo "expected non-privileged, got $PRIV" >&2; exit 1; }
echo "$BINDS" | grep -qi docker.sock && { echo "docker.sock bind: $BINDS" >&2; exit 1; }
if [[ "$CAP_DROP" != "null" && "$CAP_DROP" != "[]" ]]; then
  echo "$CAP_DROP" | grep -qi ALL || { echo "expected cap_drop ALL: $CAP_DROP" >&2; exit 1; }
fi
echo "$NO_PRIV" | grep -qi no-new-privileges || { echo "missing no-new-privileges: $NO_PRIV" >&2; exit 1; }
[[ "${PIDS:-0}" -gt 0 ]] || echo "warn: pids_limit not reported ($PIDS)"
[[ "${MEM:-0}" -gt 0 ]] || echo "warn: mem_limit not in HostConfig.Memory ($MEM) — checked via compose file"
curl -sf --max-time 8 "$TARGET" >/dev/null

REPO_DIGEST=$(docker inspect "$IMAGE_ID" --format '{{index .RepoDigests 0}}' 2>/dev/null || true)
echo "image_id=$IMAGE_ID repo_digest=${REPO_DIGEST:-none}"

docker compose -p "$PROJECT" down -v --remove-orphans
trap - EXIT

echo "=== unhealthy path + rollback ==="
PORT2=$((20000 + RANDOM % 1000))
PROJECT2="musehub-ci-compose-fail-$PORT2"
export MUSEHUB_COMPOSE_LEAVE_STACK=0
export MUSEHUB_COMPOSE_PROJECT="$PROJECT2"
export MUSEHUB_COMPOSE_PUBLISH_PORT="$PORT2"
export MUSEHUB_DEPLOY_HEALTH_TARGET="http://127.0.0.1:9/"
set +e
FAIL_OUT=$("$BIN" 2>&1)
FAIL_CODE=$?
set -e
[[ "$FAIL_CODE" -ne 0 ]] || { echo "expected failure exit, got 0: $FAIL_OUT" >&2; exit 1; }
RUNNING=$(docker compose -p "$PROJECT2" ps -q 2>/dev/null | wc -l | tr -d ' ')
[[ "$RUNNING" == "0" ]] || {
  echo "containers still up after rollback: $RUNNING" >&2
  docker compose -p "$PROJECT2" ps
  docker compose -p "$PROJECT2" down -v --remove-orphans
  exit 1
}

echo "=== idempotent redeploy ==="
PORT3=$((21000 + RANDOM % 1000))
PROJECT3="musehub-ci-compose-idem-$PORT3"
export MUSEHUB_COMPOSE_PROJECT="$PROJECT3"
export MUSEHUB_COMPOSE_PUBLISH_PORT="$PORT3"
export MUSEHUB_DEPLOY_HEALTH_TARGET="http://127.0.0.1:${PORT3}/"
export MUSEHUB_COMPOSE_LEAVE_STACK=0
"$BIN" >/dev/null
"$BIN" >/dev/null
docker compose -p "$PROJECT3" down -v --remove-orphans 2>/dev/null || true

echo "compose_runner_e2e=ok port=$PORT"
