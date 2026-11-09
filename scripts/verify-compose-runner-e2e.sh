#!/usr/bin/env bash
# Real compose-runner gate: digest-pinned pull, up, strict security inspect, health, rollback.
set -euo pipefail
cd "$(dirname "$0")/.."

command -v docker >/dev/null || { echo "compose_runner_e2e=blocked docker CLI missing" >&2; exit 1; }
docker info >/dev/null 2>&1 || { echo "compose_runner_e2e=blocked docker info failed" >&2; exit 1; }

cargo build -p musehub-agent-adapter --bin musehub-compose-runner -q
BIN="$PWD/target/debug/musehub-compose-runner"

resolve_oci_image_ref() {
  local tag="$1"
  local pull_out
  pull_out=$(docker pull "$tag" 2>&1)
  echo "$pull_out" >&2
  local ref digest repo
  ref=$(docker image inspect "$tag" --format '{{index .RepoDigests 0}}' 2>/dev/null || true)
  ref=$(echo "$ref" | tr -d '[:space:]')
  digest=$(echo "$pull_out" | sed -n 's/^Digest: //p' | head -1 | tr -d '[:space:]')
  if [[ -z "$ref" || "$ref" != *"/"* ]]; then
    [[ "$digest" =~ ^sha256:[a-f0-9]{64}$ ]] || {
      echo "compose_runner_e2e=fail empty RepoDigests and no Digest line for tag=$tag" >&2
      exit 1
    }
  if [[ "$tag" == */* ]]; then
      repo="${tag%%@*}"
      repo="${repo%%:*}"
      if [[ "$repo" != *.*/* ]]; then
        repo="docker.io/$repo"
      fi
    else
      repo="docker.io/library/${tag%%:*}"
    fi
    ref="${repo}@${digest}"
  fi
  if [[ ! "$ref" =~ @sha256:[a-f0-9]{64}$ ]] || [[ "$ref" != *"/"* ]]; then
    echo "compose_runner_e2e=fail invalid oci ref: $ref" >&2
    exit 1
  fi
  printf '%s' "$ref"
}

# Unprivileged nginx listens on 8080 — required when cap_drop ALL (no CAP_NET_BIND_SERVICE on :80).
NGINX_TAG="${MUSEHUB_NGINX_IMAGE_TAG:-nginxinc/nginx-unprivileged:1.27-alpine}"
echo "=== docker pull $NGINX_TAG ==="
OCI_REF=$(resolve_oci_image_ref "$NGINX_TAG")
echo "oci_image_ref=$OCI_REF"

echo "=== fail-closed without MUSEHUB_OCI_IMAGE_REF ==="
set +e
MISS_OUT=$("$BIN" 2>&1)
MISS_RC=$?
set -e
[[ "$MISS_RC" -ne 0 ]] || { echo "expected missing oci exit 0: $MISS_OUT" >&2; exit 1; }
echo "$MISS_OUT" | grep -q 'oci_image_ref_missing' || {
  echo "expected oci_image_ref_missing: $MISS_OUT" >&2
  exit 1
}

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
export MUSEHUB_OCI_IMAGE_REF="$OCI_REF"
unset MUSEHUB_NGINX_IMAGE_DIGEST

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
echo "$OUT" | grep -q "\"applied_oci_digest\"" || {
  echo "runner output missing applied_oci_digest" >&2
  exit 1
}
REQ_DIGEST="${OCI_REF#*@}"
echo "$OUT" | grep -q "\"applied_oci_digest\":\"$REQ_DIGEST\"" || {
  echo "applied digest must match release oci ref ($REQ_DIGEST)" >&2
  exit 1
}

CID=$(docker compose -p "$PROJECT" ps -q | head -n1)
[[ -n "$CID" ]] || { echo "no container in project $PROJECT" >&2; docker compose -p "$PROJECT" ps -a; exit 1; }

PRIV=$(docker inspect "$CID" --format '{{.HostConfig.Privileged}}')
BINDS=$(docker inspect "$CID" --format '{{json .HostConfig.Binds}}')
CAP_DROP=$(docker inspect "$CID" --format '{{json .HostConfig.CapDrop}}')
MEM=$(docker inspect "$CID" --format '{{.HostConfig.Memory}}')
PIDS=$(docker inspect "$CID" --format '{{.HostConfig.PidsLimit}}')
RO=$(docker inspect "$CID" --format '{{.HostConfig.ReadonlyRootfs}}')
NO_PRIV=$(docker inspect "$CID" --format '{{json .HostConfig.SecurityOpt}}')
PORTS=$(docker inspect "$CID" --format '{{json .NetworkSettings.Ports}}')
IMAGE_ID=$(docker inspect "$CID" --format '{{.Image}}')

[[ "$PRIV" == "false" ]] || { echo "expected non-privileged, got $PRIV" >&2; exit 1; }
[[ "$RO" == "true" ]] || { echo "expected ReadonlyRootfs=true, got $RO" >&2; exit 1; }
echo "$BINDS" | grep -qi docker.sock && { echo "docker.sock bind: $BINDS" >&2; exit 1; }
echo "$CAP_DROP" | grep -qi ALL || { echo "expected cap_drop ALL: $CAP_DROP" >&2; exit 1; }
echo "$NO_PRIV" | grep -qi no-new-privileges || { echo "missing no-new-privileges: $NO_PRIV" >&2; exit 1; }
[[ "$PIDS" == "128" ]] || { echo "expected PidsLimit=128, got $PIDS" >&2; exit 1; }
[[ "$MEM" == "134217728" ]] || { echo "expected Memory=134217728, got $MEM" >&2; exit 1; }
echo "$PORTS" | grep -q '127.0.0.1' || { echo "expected 127.0.0.1 publish binding, ports=$PORTS" >&2; exit 1; }
curl -sf --max-time 8 "$TARGET" >/dev/null

REPO_DIGEST=$(docker inspect "$IMAGE_ID" --format '{{index .RepoDigests 0}}' 2>/dev/null || true)
[[ -n "$REPO_DIGEST" && "$REPO_DIGEST" == *"@$REQ_DIGEST" ]] || {
  echo "container image digest must match pinned ref: $REPO_DIGEST vs $REQ_DIGEST" >&2
  exit 1
}

docker compose -p "$PROJECT" down -v --remove-orphans
trap - EXIT

echo "=== unhealthy path + rollback ==="
PORT2=$((20000 + RANDOM % 1000))
PROJECT2="musehub-ci-compose-fail-$PORT2"
export MUSEHUB_COMPOSE_LEAVE_STACK=0
export MUSEHUB_COMPOSE_PROJECT="$PROJECT2"
export MUSEHUB_COMPOSE_PUBLISH_PORT="$PORT2"
export MUSEHUB_DEPLOY_HEALTH_TARGET="http://127.0.0.1:9/"
export MUSEHUB_OCI_IMAGE_REF="$OCI_REF"
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
export MUSEHUB_OCI_IMAGE_REF="$OCI_REF"
"$BIN" >/dev/null
"$BIN" >/dev/null
docker compose -p "$PROJECT3" down -v --remove-orphans 2>/dev/null || true

echo "compose_runner_e2e=ok port=$PORT oci=$OCI_REF"
