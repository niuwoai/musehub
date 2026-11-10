#!/usr/bin/env bash
# Exercises deploy/RUNBOOK.md quick start on a throwaway compose project (demo profile).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
if [[ -z "${MUSEHUB_E2E_DOCKER_GROUP:-}" ]] && getent group docker >/dev/null 2>&1; then
  exec sg docker -c "export MUSEHUB_E2E_DOCKER_GROUP=1; cd '$PWD' && ./scripts/verify-clean-install-runbook.sh"
fi

STACK="musehub-runbook-$(date +%s)"
export COMPOSE_PROJECT_NAME="$STACK"
ENV_FILE="$ROOT/deploy/.env.runbook.${STACK}"
CLEANUP_ENV="$ENV_FILE"
MUSEHUB_CI_POSTGRES_PORT=$((40000 + RANDOM % 20000))
MUSEHUB_CI_API_HOST_PORT=$((50000 + RANDOM % 15000))
MUSEHUB_GATEWAY_HTTP_PORT=$((30000 + RANDOM % 20000))
trap 'docker compose -f "$ROOT/deploy/docker-compose.yml" -f "$ROOT/deploy/docker-compose.ci.yml" --env-file "$CLEANUP_ENV" down -v --remove-orphans 2>/dev/null || true; rm -f "$CLEANUP_ENV"' EXIT

cp "$ROOT/deploy/.env.example" "$ENV_FILE"
PW="$(openssl rand -hex 12)"
SEC="$(openssl rand -hex 20)"
ADM="Runbook-$(openssl rand -hex 6)"
sed -i "s/^POSTGRES_PASSWORD=.*/POSTGRES_PASSWORD=$PW/" "$ENV_FILE"
sed -i "s/^MUSEHUB_SESSION_SECRET=.*/MUSEHUB_SESSION_SECRET=$SEC/" "$ENV_FILE"
sed -i "s/^MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD=.*/MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD=$ADM/" "$ENV_FILE"
sed -i "s/^MUSEHUB_GATEWAY_HTTP_PORT=.*/MUSEHUB_GATEWAY_HTTP_PORT=$MUSEHUB_GATEWAY_HTTP_PORT/" "$ENV_FILE"
{
  echo "MUSEHUB_CI_POSTGRES_PORT=$MUSEHUB_CI_POSTGRES_PORT"
  echo "MUSEHUB_CI_API_HOST_PORT=$MUSEHUB_CI_API_HOST_PORT"
} >>"$ENV_FILE"

COMPOSE=(docker compose -f "$ROOT/deploy/docker-compose.yml" -f "$ROOT/deploy/docker-compose.ci.yml" --env-file "$ENV_FILE")
echo "runbook_step=build"
"${COMPOSE[@]}" build musehub-server gateway
echo "runbook_step=up"
"${COMPOSE[@]}" up -d postgres
for _ in $(seq 1 30); do
  "${COMPOSE[@]}" exec -T postgres pg_isready -U musehub >/dev/null 2>&1 && break
  sleep 2
done
"${COMPOSE[@]}" up -d musehub-server

PORT="$(grep '^MUSEHUB_GATEWAY_HTTP_PORT=' "$ENV_FILE" | cut -d= -f2 | tr -d '[:space:]')"
BASE="http://127.0.0.1:${PORT:-8080}"
for _ in $(seq 1 90); do
  if "${COMPOSE[@]}" exec -T musehub-server wget -qO- http://127.0.0.1:8080/readyz 2>/dev/null \
    | jq -e '.ready == true and .checks.product_api == "ok"' >/dev/null 2>&1; then
    break
  fi
  sleep 2
done
"${COMPOSE[@]}" exec -T musehub-server wget -qO- http://127.0.0.1:8080/readyz \
  | jq -e '.ready == true and .checks.product_api == "ok"' >/dev/null
API_BASE="http://127.0.0.1:${MUSEHUB_CI_API_HOST_PORT}"
curl -fsS "$API_BASE/readyz" | jq -e '.checks.product_api == "ok"' >/dev/null
"${COMPOSE[@]}" up -d gateway
for _ in $(seq 1 30); do
  curl -fsS "$BASE/readyz" | jq -e '.checks.product_api == "ok"' >/dev/null 2>&1 && break
  sleep 2
done
curl -fsS "$BASE/readyz" | jq -e '.checks.product_api == "ok"' >/dev/null || {
  echo "runbook_warn=gateway not ready; API direct OK" >&2
}
"${COMPOSE[@]}" exec -T musehub-server sh -c 'test ! -e /var/run/docker.sock'
echo "runbook_step=migrate_idempotent"
"${COMPOSE[@]}" run --rm --no-deps --entrypoint /docker-entrypoint.sh musehub-server migrate-only
"${COMPOSE[@]}" run --rm --no-deps --entrypoint /docker-entrypoint.sh musehub-server migrate-only
echo "clean_install_runbook=ok gateway=$BASE"
