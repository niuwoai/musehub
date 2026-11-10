#!/usr/bin/env bash
# Invoke production docker compose (base + production profile + optional TLS overlays).
production_compose() {
  local root deploy env_file tls_mode
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  deploy="${MUSEHUB_DEPLOY_DIR:-$root/deploy}"
  env_file="${MUSEHUB_ENV_FILE:-$deploy/.env}"

  if [[ -f "$env_file" ]]; then
    set -a
    # shellcheck source=/dev/null
    source "$env_file"
    set +a
  fi

  tls_mode="${MUSEHUB_GATEWAY_TLS_MODE:-off}"
  local -a cmd=(docker compose -f "$deploy/docker-compose.yml" -f "$deploy/docker-compose.production.yml")

  "$root/scripts/render-gateway-caddyfile.sh" >/dev/null

  if [[ "$tls_mode" != "off" ]]; then
    cmd+=(-f "$deploy/docker-compose.gateway-tls.yml")
    if [[ "$tls_mode" == "manual" ]]; then
      cmd+=(-f "$deploy/docker-compose.gateway-tls-manual.yml")
    fi
  fi

  cmd+=(--env-file "$env_file" "$@")
  "${cmd[@]}"
}

production_gateway_base_url() {
  local env_file="${MUSEHUB_ENV_FILE:-${MUSEHUB_DEPLOY_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)/deploy}/.env}"
  # shellcheck source=/dev/null
  [[ -f "$env_file" ]] && set -a && source "$env_file" && set +a
  local tls_mode="${MUSEHUB_GATEWAY_TLS_MODE:-off}"
  local domain="${MUSEHUB_GATEWAY_TLS_DOMAIN:-}"
  local port="${MUSEHUB_GATEWAY_HTTP_PORT:-8080}"
  if [[ "$tls_mode" != "off" ]]; then
    echo "https://${domain:-127.0.0.1}"
  else
    echo "http://127.0.0.1:${port}"
  fi
}
