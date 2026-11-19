#!/usr/bin/env bash
# OIDC identity-link browser acceptance against a disposable local Keycloak.
set -euo pipefail
cd "$(dirname "$0")/.."

: "${MUSEHUB_TEST_DATABASE_URL:?set this to a dedicated disposable acceptance database}"
if [[ "${MUSEHUB_ACCEPTANCE_ALLOW_DATABASE_RESET:-}" != "1" ]]; then
  echo "set MUSEHUB_ACCEPTANCE_ALLOW_DATABASE_RESET=1 to authorize migration down/up" >&2
  exit 1
fi
for command in docker curl openssl; do
  command -v "$command" >/dev/null || { echo "missing required command: $command" >&2; exit 1; }
done

SERVER_PORT=18180
WEB_PORT=18173
REALM=musehub-oidc-e2e
CLIENT_ID=musehub-oidc-e2e
ISSUER_USERNAME=oidc-link-user
KEYCLOAK_CONTAINER="musehub-oidc-e2e-$$"
TEST_PASSWORD="$(openssl rand -hex 24)"

cleanup() {
  docker rm -f "$KEYCLOAK_CONTAINER" >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

docker run --rm -d \
  --name "$KEYCLOAK_CONTAINER" \
  -e KC_BOOTSTRAP_ADMIN_USERNAME=admin \
  -e "KC_BOOTSTRAP_ADMIN_PASSWORD=$TEST_PASSWORD" \
  -p 127.0.0.1::8080 \
  quay.io/keycloak/keycloak:26.4.2 \
  start-dev --http-enabled=true --hostname-strict=false >/dev/null

KEYCLOAK_PORT="$(docker port "$KEYCLOAK_CONTAINER" 8080/tcp | awk -F: '{print $NF}')"
ISSUER_URL="http://127.0.0.1:${KEYCLOAK_PORT}/realms/${REALM}"
REDIRECT_URI="http://127.0.0.1:${WEB_PORT}/api/v1/auth/oidc/callback"

ready=0
for _ in $(seq 1 120); do
  if curl -fsS "http://127.0.0.1:${KEYCLOAK_PORT}/realms/master/.well-known/openid-configuration" >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 1
done
if [[ "$ready" != "1" ]]; then
  echo "keycloak_error=readiness_timeout" >&2
  exit 1
fi

KCADM=/opt/keycloak/bin/kcadm.sh
docker exec "$KEYCLOAK_CONTAINER" "$KCADM" config credentials \
  --server http://localhost:8080 --realm master --user admin \
  --password "$TEST_PASSWORD" >/dev/null
docker exec "$KEYCLOAK_CONTAINER" "$KCADM" create realms \
  -s "realm=$REALM" -s enabled=true >/dev/null
docker exec "$KEYCLOAK_CONTAINER" "$KCADM" create clients -r "$REALM" \
  -s "clientId=$CLIENT_ID" -s enabled=true -s publicClient=true \
  -s standardFlowEnabled=true \
  -s "redirectUris=[\"$REDIRECT_URI\"]" \
  -s "webOrigins=[\"http://127.0.0.1:${WEB_PORT}\"]" >/dev/null
docker exec "$KEYCLOAK_CONTAINER" "$KCADM" create users -r "$REALM" \
  -s "username=$ISSUER_USERNAME" -s enabled=true \
  -s "email=${ISSUER_USERNAME}@example.invalid" -s emailVerified=true \
  -s firstName=OIDC -s lastName=Acceptance >/dev/null
docker exec "$KEYCLOAK_CONTAINER" "$KCADM" set-password -r "$REALM" \
  --username "$ISSUER_USERNAME" --new-password "$TEST_PASSWORD" >/dev/null

export MUSEHUB_ACCEPTANCE_ALLOW_DATABASE_RESET=1
export MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD="$TEST_PASSWORD"
export MUSEHUB_OIDC_ISSUER_URL="$ISSUER_URL"
export MUSEHUB_OIDC_CLIENT_ID="$CLIENT_ID"
export MUSEHUB_OIDC_CLIENT_SECRET=
export MUSEHUB_OIDC_REDIRECT_URI="$REDIRECT_URI"
export MUSEHUB_OIDC_E2E_USERNAME="$ISSUER_USERNAME"
export MUSEHUB_OIDC_E2E_PASSWORD="$TEST_PASSWORD"
export MUSEHUB_E2E_SERVER_PORT="$SERVER_PORT"
export MUSEHUB_E2E_WEB_PORT="$WEB_PORT"
export MUSEHUB_E2E_ENABLE_DOCKER=0
export MUSEHUB_E2E_SKIP_INSTALL=1
export MUSEHUB_E2E_BROWSER_CHANNEL=chrome
export MUSEHUB_E2E_ALLOW_PLAYWRIGHT_ARGS=1
export PLAYWRIGHT_ARGS=e2e/oidc-identity-link.spec.ts

./scripts/run-web-e2e-isolated.sh
