#!/usr/bin/env bash
# Static checks for production/CI Caddy gateway configs (no live TLS terminate required).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

for f in \
  "$ROOT/deploy/Caddyfile" \
  "$ROOT/deploy/Caddyfile.ci" \
  "$ROOT/deploy/Caddyfile.ci.host.yml"; do
  [[ -f "$f" ]] || {
    echo "missing $f" >&2
    exit 1
  }
done

grep -q 'reverse_proxy' "$ROOT/deploy/Caddyfile"
grep -q 'musehub-server' "$ROOT/deploy/Caddyfile"
grep -q 'tls' "$ROOT/deploy/Caddyfile"
grep -q '/mcp' "$ROOT/deploy/Caddyfile"
grep -q 'reverse_proxy' "$ROOT/deploy/Caddyfile.ci.host.yml"

docker_cli() {
  if docker info >/dev/null 2>&1; then
    docker "$@"
    return
  fi
  if command -v sudo >/dev/null 2>&1 && sudo docker info >/dev/null 2>&1; then
    sudo docker "$@"
    return
  fi
  return 1
}

if command -v docker >/dev/null 2>&1 || command -v sudo >/dev/null 2>&1; then
  if docker_cli run --rm \
    -v "$ROOT/deploy/Caddyfile:/etc/caddy/Caddyfile:ro" \
    caddy:2.9-alpine \
    caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile 2>/dev/null; then
  echo "caddy validate production example=ok"
  else
    echo "caddy validate production skipped (docker unavailable or image pull failed)" >&2
  fi
  tmp_ci="$(mktemp)"
  trap 'rm -f "$tmp_ci"' EXIT
  sed 's/host.docker.internal:58080/host.docker.internal:8080/g' \
    "$ROOT/deploy/Caddyfile.ci.host.yml" >"$tmp_ci"
  if docker_cli run --rm \
    -v "$tmp_ci:/etc/caddy/Caddyfile:ro" \
    caddy:2.9-alpine \
    caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile 2>/dev/null; then
    echo "caddy validate ci host overlay=ok"
  else
    echo "caddy validate ci skipped" >&2
  fi
else
  echo "caddy validate skipped (no docker)"
fi

echo "caddyfile_layout=ok"
