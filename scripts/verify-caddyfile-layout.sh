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

# TLS render layout (fixture certs only — no ACME / public DNS).
FIXTURE_DIR="$(mktemp -d)"
cleanup_verify() {
  rm -f "${tmp_ci:-}"
  rm -rf "$FIXTURE_DIR"
}
trap cleanup_verify EXIT
openssl req -x509 -newkey rsa:2048 -days 1 -nodes \
  -keyout "$FIXTURE_DIR/key.pem" -out "$FIXTURE_DIR/cert.pem" \
  -subj '/CN=musehub-fixture.test' 2>/dev/null

export MUSEHUB_DEPLOY_DIR="$FIXTURE_DIR"
export MUSEHUB_ENV_FILE="$FIXTURE_DIR/.env"
cat >"$MUSEHUB_ENV_FILE" <<EOF
MUSEHUB_GATEWAY_HTTP_PORT=8080
MUSEHUB_GATEWAY_TLS_MODE=manual
MUSEHUB_GATEWAY_TLS_DOMAIN=musehub-fixture.test
MUSEHUB_TLS_CERT_FILE=$FIXTURE_DIR/cert.pem
MUSEHUB_TLS_KEY_FILE=$FIXTURE_DIR/key.pem
EOF

if ! "$ROOT/scripts/render-gateway-caddyfile.sh" | grep -q 'mode=manual'; then
  echo "render_gateway_caddyfile manual=fail" >&2
  exit 1
fi
grep -q 'tls /etc/musehub/tls/cert.pem' "$FIXTURE_DIR/Caddyfile.live" || {
  echo "Caddyfile.live manual tls block missing" >&2
  exit 1
}

FAIL_ENV="$FIXTURE_DIR/fail.env"
cat >"$FAIL_ENV" <<EOF
MUSEHUB_GATEWAY_TLS_MODE=manual
MUSEHUB_GATEWAY_TLS_DOMAIN=change-me.example
MUSEHUB_TLS_CERT_FILE=$FIXTURE_DIR/cert.pem
MUSEHUB_TLS_KEY_FILE=$FIXTURE_DIR/key.pem
EOF
if MUSEHUB_DEPLOY_DIR="$FIXTURE_DIR" MUSEHUB_ENV_FILE="$FAIL_ENV" \
  "$ROOT/scripts/render-gateway-caddyfile.sh" 2>/dev/null; then
  echo "render_gateway_caddyfile should fail on placeholder domain" >&2
  exit 1
fi
echo "render_gateway_caddyfile fail_closed_placeholder=ok"

if command -v docker >/dev/null 2>&1 || command -v sudo >/dev/null 2>&1; then
  if docker_cli run --rm \
    -v "$FIXTURE_DIR/Caddyfile.live:/etc/caddy/Caddyfile:ro" \
    -v "$FIXTURE_DIR/cert.pem:/etc/musehub/tls/cert.pem:ro" \
    -v "$FIXTURE_DIR/key.pem:/etc/musehub/tls/key.pem:ro" \
    caddy:2.9-alpine \
    caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile 2>/dev/null; then
    echo "caddy validate tls manual fixture=ok"
  else
    echo "caddy validate tls manual skipped" >&2
  fi
fi

ACME_ENV="$FIXTURE_DIR/acme.env"
cat >"$ACME_ENV" <<EOF
MUSEHUB_GATEWAY_TLS_MODE=acme
MUSEHUB_GATEWAY_TLS_DOMAIN=musehub-acme-fixture.test
MUSEHUB_ACME_EMAIL=ops-acme-fixture@example.com
EOF
if ! MUSEHUB_DEPLOY_DIR="$FIXTURE_DIR" MUSEHUB_ENV_FILE="$ACME_ENV" \
  "$ROOT/scripts/render-gateway-caddyfile.sh" | grep -q 'mode=acme'; then
  echo "render_gateway_caddyfile acme=fail" >&2
  exit 1
fi
if grep -E '^:80[[:space:]]*\{' "$FIXTURE_DIR/Caddyfile.live"; then
  echo "ACME Caddyfile must not define catch-all :80 (breaks HTTP-01)" >&2
  exit 1
fi
if grep -q 'redir https://' "$FIXTURE_DIR/Caddyfile.live"; then
  echo "ACME Caddyfile must not hard-code HTTP redirect (Caddy handles ACME + redirect)" >&2
  exit 1
fi
echo "render_gateway_caddyfile acme_no_catchall_80=ok"

if command -v docker >/dev/null 2>&1 || command -v sudo >/dev/null 2>&1; then
  if docker_cli run --rm \
    -v "$FIXTURE_DIR/Caddyfile.live:/etc/caddy/Caddyfile:ro" \
    caddy:2.9-alpine \
    caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile 2>/dev/null; then
    echo "caddy validate tls acme fixture=ok"
  else
    echo "caddy validate tls acme skipped" >&2
  fi
fi

echo "caddyfile_layout=ok"
