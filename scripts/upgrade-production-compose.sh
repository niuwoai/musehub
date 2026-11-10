#!/usr/bin/env bash
# In-place image upgrade for an existing production Compose install (volumes retained).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT/deploy"
if [[ -z "${MUSEHUB_E2E_DOCKER_GROUP:-}" ]] && getent group docker >/dev/null 2>&1; then
  exec sg docker -c "export MUSEHUB_E2E_DOCKER_GROUP=1; cd '$PWD' && '$ROOT/scripts/upgrade-production-compose.sh'"
fi

[[ -f .env ]] || { echo "upgrade=fail missing deploy/.env" >&2; exit 1; }
COMPOSE=(docker compose -f docker-compose.yml -f docker-compose.production.yml --env-file .env)

echo "upgrade=hint run ./scripts/backup-musehub-state.sh before major upgrades"
echo "upgrade=phase build"
"${COMPOSE[@]}" build musehub-server gateway musehub-runner

echo "upgrade=phase migrate"
"${COMPOSE[@]}" run --rm --no-deps --entrypoint /docker-entrypoint.sh musehub-server migrate-only

echo "upgrade=phase recreate control plane"
"${COMPOSE[@]}" up -d --force-recreate postgres musehub-server gateway

GW_PORT="$(grep '^MUSEHUB_GATEWAY_HTTP_PORT=' .env | cut -d= -f2 | tr -d '\r')"
GW_PORT="${GW_PORT:-8080}"
BASE="http://127.0.0.1:${GW_PORT}"
for _ in $(seq 1 90); do
  if curl -fsS "$BASE/readyz" | jq -e '.ready == true and .checks.product_api == "ok"' >/dev/null 2>&1; then
    echo "upgrade=ready gateway=$BASE"
    break
  fi
  sleep 2
done
curl -fsS "$BASE/readyz" | jq -e '.ready == true and .checks.product_api == "ok"' >/dev/null

RUNNER_PAT=$(grep -E '^MUSEHUB_RUNNER_PAT=' .env | cut -d= -f2- | tr -d '\r' || true)
if [[ -n "$RUNNER_PAT" && "$RUNNER_PAT" != placeholder-set-after-bootstrap ]]; then
  echo "upgrade=phase recreate musehub-runner"
  "${COMPOSE[@]}" up -d --force-recreate musehub-runner
fi
echo "upgrade_production_compose=ok"
