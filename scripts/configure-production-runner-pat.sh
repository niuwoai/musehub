#!/usr/bin/env bash
# Mint a selected-scopes runner PAT and persist MUSEHUB_RUNNER_PAT in deploy/.env.
# Run after install-production-compose.sh when control plane /readyz is green.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="$ROOT/deploy/.env"
START_RUNNER=0
FORCE=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --start) START_RUNNER=1 ;;
    --force) FORCE=1 ;;
    -h | --help)
      echo "usage: $0 [--start] [--force] [path/to/deploy/.env]" >&2
      exit 0
      ;;
    *)
      ENV_FILE="$1"
      ;;
  esac
  shift
done

command -v jq >/dev/null || { echo "jq is required" >&2; exit 1; }
command -v curl >/dev/null || { echo "curl is required" >&2; exit 1; }
[[ -f "$ENV_FILE" ]] || { echo "missing env file: $ENV_FILE" >&2; exit 1; }

# shellcheck disable=SC1090
set -a
# shellcheck source=/dev/null
source "$ENV_FILE"
set +a

GW_PORT="${MUSEHUB_GATEWAY_HTTP_PORT:-8080}"
BASE="http://127.0.0.1:${GW_PORT}"
BOOTSTRAP_PW="${MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD:?set MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD in $ENV_FILE}"

existing_pat=$(grep -E '^MUSEHUB_RUNNER_PAT=' "$ENV_FILE" | cut -d= -f2- | tr -d '\r' || true)
if [[ -n "$existing_pat" && "$existing_pat" != placeholder-set-after-bootstrap && "$FORCE" -eq 0 ]]; then
  echo "configure_runner_pat=skip existing_pat (use --force to replace)"
  exit 0
fi

curl -fsS "$BASE/readyz" | jq -e '.ready == true and .checks.product_api == "ok"' >/dev/null

COOKIE_JAR=$(mktemp)
trap 'rm -f "$COOKIE_JAR"' EXIT
curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg h admin --arg p "$BOOTSTRAP_PW" '{handle:$h,password:$p}')" \
  "$BASE/api/v1/auth/login" | jq -e '.ok == true' >/dev/null

RUNNER_MEMBER="${MUSEHUB_RUNNER_HANDLE:-musehub-worker}"
RUNNER_PASS="${MUSEHUB_RUNNER_MEMBER_PASSWORD:-}"
if [[ -z "$RUNNER_PASS" ]]; then
  RUNNER_PASS="Worker-$(openssl rand -hex 8)!"
  echo "configure_runner_pat=generated_worker_password handle=$RUNNER_MEMBER password=$RUNNER_PASS"
fi

curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -X POST -H 'Content-Type: application/json' \
  -d "$(jq -n --arg h "$RUNNER_MEMBER" --arg p "$RUNNER_PASS" '{handle:$h,password:$p,role:"member"}')" \
  "$BASE/api/v1/orgs/demo/members" >/dev/null 2>&1 || true

ANCHOR_SLUG="${MUSEHUB_RUNNER_ANCHOR_REPO:-runner-anchor}"
curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -X POST -H 'Content-Type: application/json' \
  -d "$(jq -n --arg s "$ANCHOR_SLUG" '{slug:$s,description:"runner PAT scope anchor"}')" \
  "$BASE/api/v1/orgs/demo/repos" >/dev/null 2>&1 || true

REPO_ID=$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" "$BASE/api/v1/orgs/demo/repos" \
  | jq -r --arg s "$ANCHOR_SLUG" '.[] | select(.slug == $s) | .id' | head -1)
if [[ -z "$REPO_ID" || "$REPO_ID" == null ]]; then
  echo "configure_runner_pat=fail anchor repo slug=$ANCHOR_SLUG not in org list (create may have failed; will not bind PAT to another repo)" >&2
  exit 1
fi

curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -X POST -H 'Content-Type: application/json' \
  -d "$(jq -n --arg h "$RUNNER_MEMBER" '{handle:$h,role:"developer"}')" \
  "$BASE/api/v1/orgs/demo/repos/$ANCHOR_SLUG/collaborators" >/dev/null 2>&1 || true

PAT_JSON=$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -X POST -H 'Content-Type: application/json' \
  -d "$(jq -n --arg org demo --arg h "$RUNNER_MEMBER" --arg id "$REPO_ID" \
    '{name:"compose-runner",org:$org,scope_mode:"selected",repo_ids:[$id],runner_handle:$h,
      scopes:["pipeline:execute","agent:execute","deploy:execute","checks:write","repo:read"]}')" \
  "$BASE/api/v1/tokens")
RUNNER_PAT=$(echo "$PAT_JSON" | jq -r .token)
[[ ${#RUNNER_PAT} -gt 20 ]] || { echo "configure_runner_pat=fail mint" >&2; exit 1; }

grep -v '^MUSEHUB_RUNNER_PAT=' "$ENV_FILE" >"${ENV_FILE}.new"
printf 'MUSEHUB_RUNNER_PAT=%s\n' "$RUNNER_PAT" >>"${ENV_FILE}.new"
mv "${ENV_FILE}.new" "$ENV_FILE"
echo "configure_runner_pat=ok env=$ENV_FILE runner_handle=$RUNNER_MEMBER"

if [[ "$START_RUNNER" -eq 1 ]]; then
  # shellcheck source=lib/production-compose.sh
  source "$ROOT/scripts/lib/production-compose.sh"
  export MUSEHUB_ENV_FILE="$ENV_FILE"
  if [[ -z "${MUSEHUB_E2E_DOCKER_GROUP:-}" ]] && getent group docker >/dev/null 2>&1; then
    exec sg docker -c "export MUSEHUB_E2E_DOCKER_GROUP=1 MUSEHUB_ENV_FILE='$ENV_FILE'; cd '$ROOT/deploy' && source '$ROOT/scripts/lib/production-compose.sh' && production_compose up -d musehub-runner"
  fi
  cd "$ROOT/deploy"
  production_compose up -d musehub-runner
  echo "configure_runner_pat=musehub-runner started"
fi
