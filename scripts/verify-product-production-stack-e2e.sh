#!/usr/bin/env bash
# Production compose profile: API enqueue-only (no docker.sock), worker executes adapters.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
if [[ -z "${MUSEHUB_E2E_DOCKER_GROUP:-}" ]] && getent group docker >/dev/null 2>&1; then
  exec sg docker -c "export MUSEHUB_E2E_DOCKER_GROUP=1; cd '$PWD' && ./scripts/verify-product-production-stack-e2e.sh"
fi
export DOCKER_BUILDKIT="${DOCKER_BUILDKIT:-0}"
export COMPOSE_DOCKER_CLI_BUILD="${COMPOSE_DOCKER_CLI_BUILD:-0}"

command -v docker >/dev/null || { echo "docker required" >&2; exit 1; }
command -v jq >/dev/null || { echo "jq required" >&2; exit 1; }
command -v curl >/dev/null || { echo "curl required" >&2; exit 1; }

if [[ "${MUSEHUB_SKIP_CI_REGISTRY:-}" != "1" ]]; then
  chmod +x ./scripts/start-ci-registry.sh
  ./scripts/start-ci-registry.sh
fi
export MUSEHUB_OCI_REGISTRY="${MUSEHUB_OCI_REGISTRY:-127.0.0.1:5000}"

STACK_ID="musehub-prod-$(date +%s)"
export COMPOSE_PROJECT_NAME="$STACK_ID"
ENV_FILE="$ROOT/deploy/.env.production.${STACK_ID}"
CADDY_CI_HOST="$ROOT/deploy/.caddy.prod.${STACK_ID}.yml"
MUSEHUB_CI_POSTGRES_PORT=$((40000 + RANDOM % 20000))
MUSEHUB_CI_API_HOST_PORT=$((50000 + RANDOM % 15000))
MUSEHUB_GATEWAY_HTTP_PORT=$((30000 + RANDOM % 20000))
export MUSEHUB_CI_POSTGRES_PORT MUSEHUB_CI_API_HOST_PORT MUSEHUB_GATEWAY_HTTP_PORT
unset MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD MUSEHUB_SESSION_SECRET 2>/dev/null || true
sed "s/host.docker.internal:58080/host.docker.internal:${MUSEHUB_CI_API_HOST_PORT}/g" \
  "$ROOT/deploy/Caddyfile.ci.host.yml" >"$CADDY_CI_HOST"
COMPOSE_CI_EXTRA="$ROOT/deploy/.compose.prod.${STACK_ID}.yml"
cat >"$COMPOSE_CI_EXTRA" <<EOF
services:
  gateway:
    volumes:
      - ${CADDY_CI_HOST}:/etc/caddy/Caddyfile:ro
  musehub-runner:
    extra_hosts:
      - "host.docker.internal:host-gateway"
EOF

POSTGRES_PASSWORD="$(openssl rand -hex 16)"
SESSION_SECRET="$(openssl rand -hex 24)"
BOOTSTRAP_PW="ProdStack$(openssl rand -hex 8)"
cat >"$ENV_FILE" <<EOF
POSTGRES_PASSWORD=$POSTGRES_PASSWORD
MUSEHUB_SESSION_SECRET=$SESSION_SECRET
MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD=$BOOTSTRAP_PW
MUSEHUB_GATEWAY_HTTP_PORT=$MUSEHUB_GATEWAY_HTTP_PORT
MUSEHUB_MCP_PRODUCT=0
MUSEHUB_REQUIRE_PIPELINE_OCI=1
MUSEHUB_OCI_REGISTRY=$MUSEHUB_OCI_REGISTRY
MUSEHUB_CI_POSTGRES_PORT=$MUSEHUB_CI_POSTGRES_PORT
MUSEHUB_CI_API_HOST_PORT=$MUSEHUB_CI_API_HOST_PORT
MUSEHUB_RUNNER_PAT=placeholder-will-replace
EOF

COMPOSE_BASE=(docker compose -f "$ROOT/deploy/docker-compose.yml" -f "$ROOT/deploy/docker-compose.ci.yml"
  -f "$ROOT/deploy/docker-compose.production.yml" -f "$COMPOSE_CI_EXTRA" --env-file "$ENV_FILE")
dc() { "${COMPOSE_BASE[@]}" "$@"; }

cleanup() {
  dc down -v --remove-orphans 2>/dev/null || true
  rm -f "$ENV_FILE" "${CADDY_CI_HOST:-}" "${COMPOSE_CI_EXTRA:-}"
}
trap cleanup EXIT

if grep -v '^[[:space:]]*#' "$ROOT/deploy/docker-compose.yml" | grep -q 'docker\.sock'; then
  echo "base compose must not mount docker.sock on API" >&2
  exit 1
fi

echo "=== production stack build ==="
dc build --pull musehub-server gateway musehub-runner

echo "=== phase 1: control plane only (no worker PAT yet) ==="
dc up -d postgres musehub-server gateway
sleep 5
_postenv() { grep "^$1=" "$ENV_FILE" | cut -d= -f2- | tr -d '\r'; }
POSTGRES_PASSWORD="$(_postenv POSTGRES_PASSWORD)"
SESSION_SECRET="$(_postenv MUSEHUB_SESSION_SECRET)"
BOOTSTRAP_PW="$(_postenv MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD)"
MUSEHUB_GATEWAY_HTTP_PORT="$(_postenv MUSEHUB_GATEWAY_HTTP_PORT)"
export POSTGRES_PASSWORD SESSION_SECRET BOOTSTRAP_PW MUSEHUB_GATEWAY_HTTP_PORT
BASE="http://127.0.0.1:${MUSEHUB_GATEWAY_HTTP_PORT}"
for _ in $(seq 1 90); do
  if curl -fsS "$BASE/readyz" | jq -e '.ready == true and .checks.product_api == "ok"' >/dev/null 2>&1; then
    break
  fi
  sleep 2
done
curl -fsS "$BASE/readyz" | jq -e '.ready == true' >/dev/null

echo "=== API container must not expose docker.sock or run docker ==="
dc exec -T musehub-server sh -c 'test ! -e /var/run/docker.sock'
dc exec -T musehub-server sh -c 'command -v docker >/dev/null 2>&1 && exit 1 || exit 0'

COOKIE_JAR=$(mktemp)
LOGIN_BODY=$(jq -c -n --arg h admin --arg p "$BOOTSTRAP_PW" '{handle:$h,password:$p}')
LOGIN_URL="${BASE}/api/v1/auth/login"
[[ -n "$LOGIN_URL" && "$LOGIN_URL" == http* ]] || {
  echo "invalid login url base=${BASE} port=${MUSEHUB_GATEWAY_HTTP_PORT}" >&2
  exit 78
}
login_json=$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -H 'Content-Type: application/json' -d "$LOGIN_BODY" "$LOGIN_URL") || {
  echo "login failed url=$LOGIN_URL" >&2
  exit 1
}
echo "$login_json" | jq -e '.ok == true' >/dev/null

dc exec -T musehub-server wget -qO- http://127.0.0.1:8080/api/v1/runner/capabilities \
  | jq -e \
  '.pipeline_executor == "remote" and .agent_executor == "remote" and .deploy_executor == "remote"' >/dev/null

RUNNER_MEMBER="prod-worker-$(date +%s)"
RUNNER_PASS="Prod-Worker-Member-1!"
curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -X POST -H 'Content-Type: application/json' \
  -d "$(jq -n --arg h "$RUNNER_MEMBER" --arg p "$RUNNER_PASS" '{handle:$h,password:$p,role:"member"}')" \
  "$BASE/api/v1/orgs/demo/members" >/dev/null

curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -X POST -H 'Content-Type: application/json' \
  -d '{"slug":"prod-slice","description":"production acceptance"}' \
  "$BASE/api/v1/orgs/demo/repos" >/dev/null

REPO_ID=$(dc exec -T postgres psql -U musehub -d musehub -Atc "SELECT id FROM repositories WHERE slug='prod-slice'")
curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -X POST -H 'Content-Type: application/json' \
  -d "$(jq -n --arg h "$RUNNER_MEMBER" '{handle:$h,role:"developer"}')" \
  "$BASE/api/v1/orgs/demo/repos/prod-slice/collaborators" >/dev/null

PAT_JSON=$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -X POST -H 'Content-Type: application/json' \
  -d "$(jq -n --arg org demo --arg h "$RUNNER_MEMBER" --arg id "$REPO_ID" \
    '{name:"prod-worker",org:$org,scope_mode:"selected",repo_ids:[$id],runner_handle:$h,
      scopes:["pipeline:execute","agent:execute","deploy:execute","checks:write","repo:read"]}')")
RUNNER_PAT=$(echo "$PAT_JSON" | jq -r .token)
[[ ${#RUNNER_PAT} -gt 20 ]]

sed -i "s/^MUSEHUB_RUNNER_PAT=.*/MUSEHUB_RUNNER_PAT=${RUNNER_PAT}/" "$ENV_FILE"

echo "=== phase 2: start musehub-runner with selected PAT ==="
dc up -d musehub-runner
sleep 3
dc ps musehub-runner | grep -q 'Up'

api() {
  local method=$1 uri=$2 data=${3:-}
  local tmp body code
  tmp=$(mktemp)
  if [[ -n "$data" ]]; then
    code=$(curl -sS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -X "$method" \
      -H 'Content-Type: application/json' -d "$data" -o "$tmp" -w "%{http_code}" "$BASE$uri")
  else
    code=$(curl -sS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -X "$method" \
      -o "$tmp" -w "%{http_code}" "$BASE$uri")
  fi
  body=$(cat "$tmp")
  rm -f "$tmp"
  if [[ "$code" -ge 400 ]]; then
    echo "api ${method} ${uri} failed http=${code} body=${body}" >&2
    exit 1
  fi
  printf '%s' "$body"
}

issue=$(api POST /api/v1/orgs/demo/repos/prod-slice/issues '{"title":"prod agent"}')
issue_id=$(echo "$issue" | jq -r .id)
issue_n=$(echo "$issue" | jq -r .number)
api POST "/api/v1/orgs/demo/repos/prod-slice/issues/${issue_n}/plans" '{"body":"plan"}' >/dev/null
api PATCH "/api/v1/orgs/demo/repos/prod-slice/issues/${issue_n}/plans" >/dev/null
agent=$(api POST /api/v1/orgs/demo/repos/prod-slice/agent-runs \
  "$(jq -n --arg id "$issue_id" '{issue_id:$id,message:"production remote agent"}')")
run_id=$(echo "$agent" | jq -r .id)
[[ "$(echo "$agent" | jq -r .state)" == "queued" ]]
for _ in $(seq 1 180); do
  st=$(api GET "/api/v1/agent-runs/${run_id}")
  final_state=$(echo "$st" | jq -r .state)
  [[ "$final_state" == "succeeded" ]] && break
  [[ "$final_state" == "failed" ]] && { echo "$st" >&2; exit 1; }
  sleep 1
done
[[ "$final_state" == "succeeded" ]]
merge_commit=$(echo "$st" | jq -r .result_commit_oid)

pipe=$(api POST /api/v1/orgs/demo/repos/prod-slice/pipeline-runs \
  "$(jq -n --arg o "$merge_commit" '{source_oid:$o,run_kind:"release_build"}')")
pipe_id=$(echo "$pipe" | jq -r .id)
for _ in $(seq 1 300); do
  pst=$(api GET "/api/v1/orgs/demo/repos/prod-slice/pipeline-runs/${pipe_id}")
  pstate=$(echo "$pst" | jq -r .state)
  [[ "$pstate" == "succeeded" ]] && break
  [[ "$pstate" == "failed" ]] && { echo "$pst" >&2; exit 1; }
  sleep 1
done
[[ "$pstate" == "succeeded" ]]

rel=$(api POST /api/v1/orgs/demo/repos/prod-slice/releases \
  "$(jq -n --arg o "$merge_commit" --arg p "$pipe_id" \
    '{version:"0.0.0-prod",tag_name:"v0.0.0-prod",commit_oid:$o,pipeline_run_id:$p}')")
rel_id=$(echo "$rel" | jq -r .id)
api POST "/api/v1/orgs/demo/repos/prod-slice/releases/${rel_id}/publish" >/dev/null

dep_code=$(curl -sS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -o /tmp/prod-dep.json -w "%{http_code}" \
  -X POST -H 'Content-Type: application/json' \
  -d "$(jq -n --arg r "$rel_id" '{release_id:$r}')" \
  "$BASE/api/v1/orgs/demo/repos/prod-slice/deployments")
[[ "$dep_code" == "202" ]] || { cat /tmp/prod-dep.json >&2; exit 1; }
dep_id=$(jq -r .id /tmp/prod-dep.json)
dep_state=""
for _ in $(seq 1 300); do
  dep=$(api GET "/api/v1/orgs/demo/repos/prod-slice/deployments/${dep_id}")
  dep_state=$(echo "$dep" | jq -r .state)
  [[ "$dep_state" == "succeeded" ]] && break
  [[ "$dep_state" == "failed" ]] && { echo "$dep" >&2; exit 1; }
  sleep 1
done
[[ "$dep_state" == "succeeded" ]]
trace=$(api GET "/api/v1/orgs/demo/repos/prod-slice/trace/deployments/${dep_id}")
echo "$trace" | jq -e ".pipeline_run_id == \"$pipe_id\"" >/dev/null

echo "product_production_stack_e2e=ok run_id=$run_id pipe_id=$pipe_id deployment=$dep_id"
