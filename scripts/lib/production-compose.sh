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

production_gateway_env() {
  local root deploy env_file
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  deploy="${MUSEHUB_DEPLOY_DIR:-$root/deploy}"
  env_file="${MUSEHUB_ENV_FILE:-$deploy/.env}"
  if [[ -f "$env_file" ]]; then
    set -a
    # shellcheck source=/dev/null
    source "$env_file"
    set +a
  fi
}

# Public gateway base URL for readyz/UI (matches curl Host/SNI when TLS is on).
production_gateway_base_url() {
  production_gateway_env
  local tls_mode="${MUSEHUB_GATEWAY_TLS_MODE:-off}"
  local domain="${MUSEHUB_GATEWAY_TLS_DOMAIN:-}"
  local port="${MUSEHUB_GATEWAY_HTTP_PORT:-8080}"
  local https_port="${MUSEHUB_GATEWAY_HTTPS_PORT:-443}"
  if [[ "$tls_mode" != "off" ]]; then
    [[ -n "$domain" ]] || {
      echo "production_gateway_base_url=fail TLS enabled but MUSEHUB_GATEWAY_TLS_DOMAIN unset" >&2
      return 1
    }
    if [[ "$https_port" == "443" ]]; then
      echo "https://${domain}"
    else
      echo "https://${domain}:${https_port}"
    fi
  else
    echo "http://127.0.0.1:${port}"
  fi
}

# Local readyz probe extras: -k and --resolve to loopback (one token per line; empty when HTTP).
production_gateway_local_tls_curl_opts() {
  production_gateway_env
  local tls_mode="${MUSEHUB_GATEWAY_TLS_MODE:-off}"
  local domain="${MUSEHUB_GATEWAY_TLS_DOMAIN:-}"
  local https_port="${MUSEHUB_GATEWAY_HTTPS_PORT:-443}"
  if [[ "$tls_mode" != "off" && -n "$domain" ]]; then
    printf '%s\n' -k --resolve "${domain}:${https_port}:127.0.0.1"
  fi
}
