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
[[ -n "$NGINX_IMAGE" ]] || { echo "could not resolve RepoDigests for $NGINX_TAG" >&2; exit 1; }

PORT=$((19000 + RANDOM % 1000))
PROJECT="musehub-ci-compose-$PORT"
TARGET="http://127.0.0.1:${PORT}/"

cleanup() {
  docker compose -p "$PROJECT" down -v --remove-orphans 2>/dev/null || true
}
trap cleanup EXIT

echo "=== compose-runner success path (port=$PORT) ==="
export MUSEHUB_COMPOSE_LEAVE_STACK=1
export MUSEHUB_COMPOSE_PROJECT="$PROJECT"
export MUSEHUB_COMPOSE_PUBLISH_PORT="$PORT"
export MUSEHUB_DEPLOY_HEALTH_TARGET="$TARGET"
export MUSEHUB_NGINX_IMAGE_DIGEST="$NGINX_IMAGE"
OUT=$("$BIN")
echo "$OUT"
echo "$OUT" | grep -q '"outcome":"succeeded"' || { echo "runner did not succeed" >&2; exit 1; }

CID=$(docker compose -p "$PROJECT" ps -q web)
[[ -n "$CID" ]] || { echo "container web missing after up" >&2; exit 1; }

PRIV=$(docker inspect "$CID" --format '{{.HostConfig.Privileged}}')
BINDS=$(docker inspect "$CID" --format '{{json .HostConfig.Binds}}')
CAP_DROP=$(docker inspect "$CID" --format '{{json .HostConfig.CapDrop}}')
MEM=$(docker inspect "$CID" --format '{{.HostConfig.Memory}}')
NETMODE=$(docker inspect "$CID" --format '{{.HostConfig.NetworkMode}}')
IMAGE_ID=$(docker inspect "$CID" --format '{{.Image}}')

[[ "$PRIV" == "false" ]] || { echo "expected non-privileged, got $PRIV" >&2; exit 1; }
echo "$BINDS" | grep -qi docker.sock && { echo "docker.sock bind detected: $BINDS" >&2; exit 1; }
echo "$CAP_DROP" | grep -q '"ALL"' || { echo "expected cap_drop ALL: $CAP_DROP" >&2; exit 1; }
[[ "$MEM" == "134217728" ]] || [[ "$MEM" -gt 0 ]] || { echo "mem_limit missing: $MEM" >&2; exit 1; }
curl -sf --max-time 8 "$TARGET" >/dev/null

REPO_DIGEST=$(docker inspect "$IMAGE_ID" --format '{{index .RepoDigests 0}}')
echo "image_id=$IMAGE_ID repo_digest=$REPO_DIGEST nginx_ref=$NGINX_IMAGE"

docker compose -p "$PROJECT" down -v --remove-orphans

echo "=== unhealthy path + rollback ==="
PORT2=$((20000 + RANDOM % 1000))
PROJECT2="musehub-ci-compose-fail-$PORT2"
export MUSEHUB_COMPOSE_LEAVE_STACK=0
export MUSEHUB_COMPOSE_PROJECT="$PROJECT2"
export MUSEHUB_COMPOSE_PUBLISH_PORT="$PORT2"
export MUSEHUB_DEPLOY_HEALTH_TARGET="http://127.0.0.1:1/"
set +e
FAIL_OUT=$("$BIN" 2>&1)
FAIL_CODE=$?
set -e
[[ "$FAIL_CODE" -ne 0 ]] || { echo "expected failure exit, got 0" >&2; exit 1; }
echo "$FAIL_OUT" | grep -q unhealthy || true
RUNNING=$(docker compose -p "$PROJECT2" ps -q 2>/dev/null | wc -l | tr -d ' ')
[[ "$RUNNING" == "0" ]] || { echo "containers still up after rollback: $RUNNING" >&2; docker compose -p "$PROJECT2" ps; exit 1; }

echo "=== idempotent second deploy same project ref (new success) ==="
PORT3=$((21000 + RANDOM % 1000))
PROJECT3="musehub-ci-compose-idem-$PORT3"
export MUSEHUB_COMPOSE_PROJECT="$PROJECT3"
export MUSEHUB_COMPOSE_PUBLISH_PORT="$PORT3"
export MUSEHUB_DEPLOY_HEALTH_TARGET="http://127.0.0.1:${PORT3}/"
export MUSEHUB_COMPOSE_LEAVE_STACK=0
"$BIN" >/dev/null
"$BIN" >/dev/null
RUNNING3=$(docker compose -p "$PROJECT3" ps -q 2>/dev/null | wc -l | tr -d ' ')
[[ "$RUNNING3" == "0" ]] || docker compose -p "$PROJECT3" down -v --remove-orphans

echo "compose_runner_e2e=ok port=$PORT net_mode=$NETMODE"
