#!/usr/bin/env bash
# Local gate: Compose musehub-server (uid 10001) + musehub-git-ssh (uid 997) on shared musehub-git
# gitdata. No MUSEHUB_GIT_STORAGE_SUDO_FALLBACK in product containers.
set -euo pipefail
trap 'status=$?; echo "dual-uid interop failed (exit=$status)" >&2; exit "$status"' ERR
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
# shellcheck source=lib/ci-ephemeral-ports.sh
source "$ROOT/scripts/lib/ci-ephemeral-ports.sh"

command -v docker >/dev/null || { echo "docker required" >&2; exit 1; }
export DOCKER_BUILDKIT="${DOCKER_BUILDKIT:-0}"
export COMPOSE_DOCKER_CLI_BUILD="${COMPOSE_DOCKER_CLI_BUILD:-0}"

STACK_ID="musehub-dual-uid-$(date +%s)"
export COMPOSE_PROJECT_NAME="$STACK_ID"
ENV_FILE="$ROOT/deploy/.env.dual-uid.${STACK_ID}"
allocate_compose_ci_ports
GIT_SSH_HOST_PORT=$(pick_free_local_port)
while [[ "$GIT_SSH_HOST_PORT" == "$MUSEHUB_CI_POSTGRES_PORT" \
  || "$GIT_SSH_HOST_PORT" == "$MUSEHUB_CI_API_HOST_PORT" \
  || "$GIT_SSH_HOST_PORT" == "$MUSEHUB_GATEWAY_HTTP_PORT" ]]; do
  GIT_SSH_HOST_PORT=$(pick_free_local_port)
done
export MUSEHUB_GIT_SSH_PORT="$GIT_SSH_HOST_PORT"
export MUSEHUB_CI_POSTGRES_PORT MUSEHUB_CI_API_HOST_PORT MUSEHUB_GATEWAY_HTTP_PORT

POSTGRES_PASSWORD="$(openssl rand -hex 16)"
SESSION_SECRET="$(openssl rand -hex 24)"
BOOTSTRAP_PW="DualUid$(openssl rand -hex 8)"
cat >"$ENV_FILE" <<EOF
POSTGRES_PASSWORD=$POSTGRES_PASSWORD
MUSEHUB_SESSION_SECRET=$SESSION_SECRET
MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD=$BOOTSTRAP_PW
MUSEHUB_GATEWAY_HTTP_PORT=$MUSEHUB_GATEWAY_HTTP_PORT
MUSEHUB_CI_POSTGRES_PORT=$MUSEHUB_CI_POSTGRES_PORT
MUSEHUB_CI_API_HOST_PORT=$MUSEHUB_CI_API_HOST_PORT
MUSEHUB_MCP_PRODUCT=0
EOF
export POSTGRES_PASSWORD SESSION_SECRET BOOTSTRAP_PW
unset MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD MUSEHUB_SESSION_SECRET 2>/dev/null || true

CADDY_CI_HOST="$ROOT/deploy/.caddy.dual-uid.${STACK_ID}.yml"
sed "s/host.docker.internal:58080/host.docker.internal:${MUSEHUB_CI_API_HOST_PORT}/g" \
  "$ROOT/deploy/Caddyfile.ci.host.yml" >"$CADDY_CI_HOST"
COMPOSE_CI_EXTRA="$ROOT/deploy/.compose.dual-uid.${STACK_ID}.yml"
cat >"$COMPOSE_CI_EXTRA" <<EOF
services:
  gateway:
    volumes:
      - ${CADDY_CI_HOST}:/etc/caddy/Caddyfile:ro
  musehub-git-ssh:
    environment:
      MUSEHUB_GIT_SSH_PORT: "2222"
    ports: !reset
      - "${MUSEHUB_GIT_SSH_PORT}:2222"
EOF
compose_cmd=(docker compose -p "$COMPOSE_PROJECT_NAME" -f "$ROOT/deploy/docker-compose.yml" -f "$ROOT/deploy/docker-compose.ci.yml" -f "$COMPOSE_CI_EXTRA" --env-file "$ENV_FILE")
if docker info >/dev/null 2>&1; then
  dc() { "${compose_cmd[@]}" "$@"; }
else
  dc() { sudo "${compose_cmd[@]}" "$@"; }
fi

cleanup() {
  dc down -v --remove-orphans 2>/dev/null || true
  rm -f "$ENV_FILE" "$CADDY_CI_HOST" "$COMPOSE_CI_EXTRA"
}
trap cleanup EXIT

echo "=== dual-uid interop: build (server + git-ssh + gateway) ==="
dc build musehub-server musehub-git-ssh gateway

echo "=== dual-uid interop: compose up ==="
dc up -d

GW_PORT="${MUSEHUB_GATEWAY_HTTP_PORT}"
if command -v docker >/dev/null 2>&1 || command -v sudo >/dev/null 2>&1; then
  live_gw="$(dc ps --format '{{.Ports}}' 2>/dev/null | grep -oE '0\.0\.0\.0:[0-9]+->8080' | head -1 | sed -E 's/0\.0\.0\.0:([0-9]+)->8080/\1/' || true)"
  [[ -n "$live_gw" ]] && GW_PORT="$live_gw"
fi
BASE="http://127.0.0.1:${GW_PORT}"
export BASE BOOTSTRAP_PW POSTGRES_PASSWORD MUSEHUB_GATEWAY_HTTP_PORT="$GW_PORT"
export COMPOSE_PROJECT_NAME
if docker info >/dev/null 2>&1; then
  MUSEHUB_STACK_DC=$(printf '%q ' "${compose_cmd[@]}")
else
  MUSEHUB_STACK_DC=$(printf '%q ' sudo "${compose_cmd[@]}")
fi
export MUSEHUB_STACK_DC

wait_ready() {
  for _ in $(seq 1 90); do
    if curl -fsS "$BASE/readyz" | jq -e '.ready == true and .checks.product_api == "ok"' >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  echo "gateway not ready" >&2
  dc ps >&2
  return 1
}
wait_ready
live_ssh="$(dc port musehub-git-ssh 2222/tcp 2>/dev/null | head -1 | sed -E 's/.*:([0-9]+)$/\1/' || true)"
[[ -n "$live_ssh" ]] && export MUSEHUB_GIT_SSH_PORT="$live_ssh"

echo "=== dual-uid interop: permission model evidence (no sudo fallback in containers) ==="
dc exec -T musehub-server id
dc exec -T musehub-git-ssh id git
dc exec -T musehub-git-ssh id musehub 2>/dev/null || true
dc exec -T musehub-server sh -c 'echo "server fallback=${MUSEHUB_GIT_STORAGE_SUDO_FALLBACK-<unset>}"'
dc exec -T musehub-git-ssh sh -c 'echo "git-ssh fallback=${MUSEHUB_GIT_STORAGE_SUDO_FALLBACK-<unset>}"'

chmod +x "$ROOT/scripts/verify-product-compose-git-ssh-e2e.sh"
echo "=== dual-uid interop: compose git-ssh e2e (SSH / HTTP alternation) ==="
bash "$ROOT/scripts/verify-product-compose-git-ssh-e2e.sh"

echo "=== dual-uid interop: post-alternation ref ownership (shared GID, no cross-chgrp) ==="
dc exec -T musehub-git-ssh sh -c '
  set -e
  bare=$(find /var/lib/musehub/git -mindepth 1 -maxdepth 1 -name "*.git" -type d | head -1)
  [[ -n "$bare" ]] || { echo "no bare repo on volume" >&2; exit 1; }
  echo "bare=$bare"
  ls -la "$bare/HEAD" "$bare/config" "$bare/refs/heads" 2>/dev/null | head -20
  ref=$(find "$bare/refs/heads" -type f | head -1)
  [[ -n "$ref" ]] && ls -la "$ref"
  owner=$(stat -c %u "$bare/HEAD")
  if [[ "$owner" == "10001" ]]; then
    if runuser -u git -- chgrp musehub-git "$bare/HEAD" 2>/dev/null; then
      echo "unexpected: git user chgrp musehub-owned HEAD succeeded" >&2
      exit 1
    fi
    echo "git uid cannot chgrp musehub-owned HEAD (expected fail-closed boundary)"
  fi
'

echo "dual_uid_git_interop=ok"
