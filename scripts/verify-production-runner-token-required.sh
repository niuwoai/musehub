#!/usr/bin/env bash
# Production compose: starting runner without PAT must fail clearly (not silent).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
if [[ -z "${MUSEHUB_E2E_DOCKER_GROUP:-}" ]] && getent group docker >/dev/null 2>&1; then
  exec sg docker -c "export MUSEHUB_E2E_DOCKER_GROUP=1; cd '$PWD' && ./scripts/verify-production-runner-token-required.sh"
fi

ENV_FILE=$(mktemp)
cat >"$ENV_FILE" <<EOF
POSTGRES_PASSWORD=test
MUSEHUB_SESSION_SECRET=$(openssl rand -hex 16)
MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD=RunnerTokTest1!
MUSEHUB_GATEWAY_HTTP_PORT=38081
MUSEHUB_MCP_PRODUCT=0
MUSEHUB_REQUIRE_PIPELINE_OCI=1
MUSEHUB_OCI_REGISTRY=127.0.0.1:5000
MUSEHUB_CI_POSTGRES_PORT=55433
MUSEHUB_CI_API_HOST_PORT=58081
EOF
trap 'rm -f "$ENV_FILE"' EXIT

set +e
out=$(docker compose -f "$ROOT/deploy/docker-compose.yml" \
  -f "$ROOT/deploy/docker-compose.production.yml" --env-file "$ENV_FILE" \
  config musehub-runner 2>&1)
ec=$?
set -e
echo "$out" | grep -qi 'MUSEHUB_RUNNER_PAT\|required\|variable' || {
  echo "expected compose to require MUSEHUB_RUNNER_PAT, ec=$ec" >&2
  echo "$out" >&2
  exit 1
}
echo "production_runner_pat_required=ok"
