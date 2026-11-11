#!/usr/bin/env bash
# Ephemeral host ports for compose CI (bind :0 avoids collisions on warm/shared runners).
set -euo pipefail

pick_free_local_port() {
  if command -v python3 >/dev/null 2>&1; then
    python3 -c "import socket;s=socket.socket();s.bind(('127.0.0.1',0));print(s.getsockname()[1]);s.close()"
    return 0
  fi
  # Fallback: probe with bash /dev/tcp (no listen, only checks existing listeners).
  local min=${1:-40000}
  local max=${2:-59999}
  local span=$((max - min + 1))
  local attempt port
  for attempt in $(seq 1 80); do
    port=$((min + RANDOM % span))
    if command -v ss >/dev/null 2>&1; then
      if ! ss -tln 2>/dev/null | grep -qE ":${port}[[:space:]]"; then
        echo "$port"
        return 0
      fi
    elif ! (echo >/dev/tcp/127.0.0.1/"$port") 2>/dev/null; then
      echo "$port"
      return 0
    fi
  done
  echo "ci-ephemeral-ports: no free TCP port in ${min}-${max}" >&2
  return 1
}

allocate_compose_ci_ports() {
  local p api gw
  p=$(pick_free_local_port)
  api=$(pick_free_local_port)
  while [[ "$api" == "$p" ]]; do
    api=$(pick_free_local_port)
  done
  gw=$(pick_free_local_port)
  while [[ "$gw" == "$p" || "$gw" == "$api" ]]; do
    gw=$(pick_free_local_port)
  done
  export MUSEHUB_CI_POSTGRES_PORT="$p"
  export MUSEHUB_CI_API_HOST_PORT="$api"
  export MUSEHUB_GATEWAY_HTTP_PORT="$gw"
}
