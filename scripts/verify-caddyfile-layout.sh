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

if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
  docker run --rm \
    -v "$ROOT/deploy/Caddyfile:/etc/caddy/Caddyfile:ro" \
    caddy:2.9-alpine \
    caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
else
  echo "caddy validate skipped (docker unavailable)"
fi

echo "caddyfile_layout=ok"
