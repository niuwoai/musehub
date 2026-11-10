#!/usr/bin/env bash
# Clean-machine production profile: copy env, build, start control plane + worker boundary.
# Does not run full acceptance; use verify-product-production-stack-e2e.sh for that.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT/deploy"
if [[ -z "${MUSEHUB_E2E_DOCKER_GROUP:-}" ]] && getent group docker >/dev/null 2>&1; then
  exec sg docker -c "export MUSEHUB_E2E_DOCKER_GROUP=1; cd '$PWD' && '$ROOT/scripts/install-production-compose.sh'"
fi

if [[ ! -f .env ]]; then
  cp .env.example .env
  PW="$(openssl rand -hex 16)"
  SEC="$(openssl rand -hex 24)"
  ADM="Admin-$(openssl rand -hex 8)"
  sed -i "s/^POSTGRES_PASSWORD=.*/POSTGRES_PASSWORD=$PW/" .env
  sed -i "s/^MUSEHUB_SESSION_SECRET=.*/MUSEHUB_SESSION_SECRET=$SEC/" .env
  sed -i "s/^MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD=.*/MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD=$ADM/" .env
  GW_PORT="$(grep '^MUSEHUB_GATEWAY_HTTP_PORT=' .env | cut -d= -f2 | tr -d '\r')"
  GW_PORT="${GW_PORT:-8080}"
  sed -i "s|^MUSEHUB_PUBLIC_HTTP_ORIGIN=.*|MUSEHUB_PUBLIC_HTTP_ORIGIN=http://127.0.0.1:${GW_PORT}|" .env
  echo "install_production=generated_secrets env=deploy/.env admin_password=$ADM"
  echo "install_production=hint set MUSEHUB_PUBLIC_HTTP_ORIGIN in deploy/.env to your gateway hostname for Git clone"
fi

export MUSEHUB_RUNNER_PAT="${MUSEHUB_RUNNER_PAT:-placeholder-set-after-bootstrap}"
export MUSEHUB_OCI_REGISTRY="${MUSEHUB_OCI_REGISTRY:-host.docker.internal:5000}"

# shellcheck source=lib/production-compose.sh
source "$ROOT/scripts/lib/production-compose.sh"

production_compose build musehub-server gateway musehub-runner
echo "install_production=phase1_control_plane"
production_compose up -d postgres musehub-server gateway

export MUSEHUB_ENV_FILE="$ROOT/deploy/.env"
BASE="$(production_gateway_base_url)"
mapfile -t CURL_TLS < <(production_gateway_local_tls_curl_opts)
for _ in $(seq 1 90); do
  if curl -fsS "${CURL_TLS[@]}" "$BASE/readyz" | jq -e '.ready == true and .checks.product_api == "ok"' >/dev/null 2>&1; then
    echo "install_production=ready gateway=$BASE"
    echo "install_production=next ./scripts/configure-production-runner-pat.sh --start"
    echo "install_production=boot_persistence sudo ./scripts/install-musehub-systemd.sh && systemctl enable --now musehub-control-plane"
    echo "install_production=acceptance ./scripts/verify-product-production-stack-e2e.sh"
    exit 0
  fi
  sleep 2
done
echo "install_production=fail readyz" >&2
production_compose logs musehub-server --tail=40 >&2 || true
exit 1
