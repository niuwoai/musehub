#!/usr/bin/env bash
# Production compose: API enqueue-only (no docker.sock); worker runs pipeline/agent/deploy.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
if [[ -z "${MUSEHUB_E2E_DOCKER_GROUP:-}" ]] && getent group docker >/dev/null 2>&1; then
  exec sg docker -c "export MUSEHUB_E2E_DOCKER_GROUP=1; cd '$PWD' && ./scripts/verify-product-production-stack-e2e.sh"
fi
export DOCKER_BUILDKIT="${DOCKER_BUILDKIT:-0}"
export COMPOSE_DOCKER_CLI_BUILD="${COMPOSE_DOCKER_CLI_BUILD:-0}"
export MUSEHUB_OCI_REGISTRY_PORT="${MUSEHUB_OCI_REGISTRY_PORT:-5000}"

command -v docker >/dev/null || { echo "docker required" >&2; exit 1; }
command -v jq >/dev/null || { echo "jq required" >&2; exit 1; }
command -v curl >/dev/null || { echo "curl required" >&2; exit 1; }

if [[ "${MUSEHUB_SKIP_CI_REGISTRY:-}" != "1" ]]; then
  chmod +x ./scripts/start-ci-registry.sh
  ./scripts/start-ci-registry.sh
fi
export MUSEHUB_OCI_REGISTRY="${MUSEHUB_OCI_REGISTRY:-127.0.0.1:${MUSEHUB_OCI_REGISTRY_PORT}}"

STACK_ID="musehub-prod-$(date +%s)"
export COMPOSE_PROJECT_NAME="$STACK_ID"
ENV_FILE="$ROOT/deploy/.env.production.${STACK_ID}"
CADDY_CI_HOST="$ROOT/deploy/.caddy.prod.${STACK_ID}.yml"
MUSEHUB_CI_POSTGRES_PORT=$((40000 + RANDOM % 20000))
MUSEHUB_CI_API_HOST_PORT=$((50000 + RANDOM % 15000))
MUSEHUB_GATEWAY_HTTP_PORT=$((30000 + RANDOM % 20000))
DOCKER_GID="$(getent group docker | cut -d: -f3 || echo 999)"
HOST_DOCKER_BIN="$(command -v docker)"
HOST_COMPOSE_PLUGIN=""
for _c in /usr/libexec/docker/cli-plugins/docker-compose \
  /usr/lib/docker/cli-plugins/docker-compose \
  /usr/local/lib/docker/cli-plugins/docker-compose; do
  [[ -x "$_c" ]] && HOST_COMPOSE_PLUGIN="$_c" && break
done
export MUSEHUB_CI_POSTGRES_PORT MUSEHUB_CI_API_HOST_PORT MUSEHUB_GATEWAY_HTTP_PORT DOCKER_GID
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
    network_mode: host
    volumes:
      - ${HOST_DOCKER_BIN}:/usr/bin/docker:ro
$([ -n "$HOST_COMPOSE_PLUGIN" ] && printf '      - %s:/usr/local/lib/docker/cli-plugins/docker-compose:ro\n' "$HOST_COMPOSE_PLUGIN")
    group_add:
      - "${DOCKER_GID}"
    environment:
      MUSEHUB_COMPOSE_ADAPTER_MODE: runner
      MUSEHUB_API_BASE: http://127.0.0.1:${MUSEHUB_CI_API_HOST_PORT}
      MUSEHUB_OCI_REGISTRY: 127.0.0.1:${MUSEHUB_OCI_REGISTRY_PORT}
      DATABASE_URL: postgres://musehub:\${POSTGRES_PASSWORD}@127.0.0.1:${MUSEHUB_CI_POSTGRES_PORT}/musehub?sslmode=disable
      MUSEHUB_DATABASE_URL: postgres://musehub:\${POSTGRES_PASSWORD}@127.0.0.1:${MUSEHUB_CI_POSTGRES_PORT}/musehub?sslmode=disable
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

diagnose_production_failure() {
  local reason=${1:-unknown}
  echo "=== diagnose production stack ($reason) ===" >&2
  echo "gateway_base=${BASE:-unset} api_direct=${API_DIRECT:-unset}" >&2
  echo "gateway_port=${MUSEHUB_GATEWAY_HTTP_PORT:-unset} api_host_port=${MUSEHUB_CI_API_HOST_PORT:-unset}" >&2
  echo "compose_project=$COMPOSE_PROJECT_NAME env_file=$ENV_FILE" >&2
  ss -tlnp 2>/dev/null | grep ":${MUSEHUB_GATEWAY_HTTP_PORT:-0} " >&2 || true
  dc ps 2>/dev/null || true
  echo "--- gateway logs ---" >&2
  dc logs gateway --tail=30 2>/dev/null >&2 || true
  echo "--- musehub-server logs ---" >&2
  dc logs musehub-server --tail=40 2>/dev/null >&2 || true
  echo "--- musehub-runner logs ---" >&2
  dc logs musehub-runner --tail=20 2>/dev/null >&2 || true
}

cleanup() {
  dc down -v --remove-orphans 2>/dev/null || true
  rm -f "$ENV_FILE" "${CADDY_CI_HOST:-}" "${COMPOSE_CI_EXTRA:-}"
}
trap cleanup EXIT

if grep -v '^[[:space:]]*#' "$ROOT/deploy/docker-compose.yml" | grep -q 'docker\.sock'; then
  echo "base compose must not mount docker.sock on API" >&2
  exit 1
fi

_postenv() { grep "^$1=" "$ENV_FILE" | cut -d= -f2- | tr -d '\r'; }

if [[ "${MUSEHUB_PROD_STACK_SKIP_BUILD:-}" != "1" ]]; then
  echo "=== production stack build ==="
  dc build --pull musehub-server gateway musehub-runner
else
  echo "=== production stack build (server/gateway skipped; runner only) ==="
  dc build musehub-runner
fi

echo "=== phase 1: control plane (no worker until PAT) ==="
dc up -d postgres musehub-server gateway
sleep 5
BOOTSTRAP_PW="$(_postenv MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD)"
MUSEHUB_GATEWAY_HTTP_PORT="$(_postenv MUSEHUB_GATEWAY_HTTP_PORT)"
MUSEHUB_CI_API_HOST_PORT="$(_postenv MUSEHUB_CI_API_HOST_PORT)"
BASE="http://127.0.0.1:${MUSEHUB_GATEWAY_HTTP_PORT}"
API_DIRECT="http://127.0.0.1:${MUSEHUB_CI_API_HOST_PORT}"
echo "gateway_base=$BASE api_direct=$API_DIRECT"

wait_ready() {
  for _ in $(seq 1 90); do
    if curl -fsS "$BASE/readyz" | jq -e '.ready == true and .checks.product_api == "ok"' >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  diagnose_production_failure "readyz timeout"
  exit 1
}
wait_ready

echo "=== API container: no docker.sock; no docker CLI ==="
dc exec -T musehub-server sh -c 'test ! -e /var/run/docker.sock'
dc exec -T musehub-server sh -c 'command -v docker >/dev/null 2>&1 && exit 1 || exit 0'

COOKIE_JAR=$(mktemp)
LOGIN_BODY=$(jq -c -n --arg h admin --arg p "$BOOTSTRAP_PW" '{handle:$h,password:$p}')
LOGIN_URL="${BASE}/api/v1/auth/login"
echo "login_cmd=curl POST $LOGIN_URL body_len=${#LOGIN_BODY}"
login_json=$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -H 'Content-Type: application/json' -d "$LOGIN_BODY" "$LOGIN_URL") || {
  diagnose_production_failure "login"
  exit 1
}
echo "$login_json" | jq -e '.ok == true' >/dev/null
me_json=$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" "$BASE/api/v1/me")
echo "$me_json" | jq -e '.handle == "admin"' >/dev/null

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

REPO_ID=$(dc exec -T postgres psql -U musehub -d musehub -Atc "SELECT id FROM repositories WHERE slug='prod-slice'" | tr -d '[:space:]')
curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -X POST -H 'Content-Type: application/json' \
  -d "$(jq -n --arg h "$RUNNER_MEMBER" '{handle:$h,role:"developer"}')" \
  "$BASE/api/v1/orgs/demo/repos/prod-slice/collaborators" >/dev/null

PAT_JSON=$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -X POST -H 'Content-Type: application/json' \
  -d "$(jq -n --arg org demo --arg h "$RUNNER_MEMBER" --arg id "$REPO_ID" \
    '{name:"prod-worker",org:$org,scope_mode:"selected",repo_ids:[$id],runner_handle:$h,
      scopes:["pipeline:execute","agent:execute","deploy:execute","checks:write","repo:read"]}')" \
  "$BASE/api/v1/tokens")
RUNNER_PAT=$(echo "$PAT_JSON" | jq -r .token)
[[ ${#RUNNER_PAT} -gt 20 ]]

grep -v '^MUSEHUB_RUNNER_PAT=' "$ENV_FILE" >"${ENV_FILE}.pat"
printf 'MUSEHUB_RUNNER_PAT=%s\n' "$RUNNER_PAT" >>"${ENV_FILE}.pat"
mv "${ENV_FILE}.pat" "$ENV_FILE"

echo "=== phase 2: start musehub-runner (selected PAT) ==="
dc up -d musehub-runner
for _ in $(seq 1 60); do
  if dc logs musehub-runner --tail=20 2>/dev/null | grep -q 'musehub-runnerd starting'; then
    if dc exec -T musehub-runner wget -qO- \
      "http://127.0.0.1:${MUSEHUB_CI_API_HOST_PORT}/readyz" 2>/dev/null \
      | jq -e '.ready == true' >/dev/null 2>&1; then
      break
    fi
  fi
  sleep 1
done
dc ps musehub-runner | grep -q 'Up'
dc logs musehub-runner --tail=15

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
    diagnose_production_failure "api ${method} ${uri}"
    exit 1
  fi
  printf '%s' "$body"
}

echo "step=issue_plan_agent"
issue=$(api POST /api/v1/orgs/demo/repos/prod-slice/issues '{"title":"prod agent"}')
issue_id=$(echo "$issue" | jq -r .id)
issue_n=$(echo "$issue" | jq -r .number)
api POST "/api/v1/orgs/demo/repos/prod-slice/issues/${issue_n}/plans" '{"body":"plan"}' >/dev/null
api PATCH "/api/v1/orgs/demo/repos/prod-slice/issues/${issue_n}/plans" >/dev/null
agent=$(api POST /api/v1/orgs/demo/repos/prod-slice/agent-runs \
  "$(jq -n --arg id "$issue_id" '{issue_id:$id,message:"production remote agent"}')")
run_id=$(echo "$agent" | jq -r .id)
[[ "$(echo "$agent" | jq -r .state)" == "queued" ]]
for _ in $(seq 1 300); do
  st=$(api GET "/api/v1/agent-runs/${run_id}")
  final_state=$(echo "$st" | jq -r .state)
  [[ "$final_state" == "succeeded" ]] && break
  [[ "$final_state" == "failed" ]] && { echo "$st" >&2; dc logs musehub-runner --tail=40 >&2; exit 1; }
  sleep 1
done
[[ "$final_state" == "succeeded" ]] || {
  echo "agent run timeout state=${final_state:-unknown}" >&2
  dc logs musehub-runner --tail=40 >&2
  diagnose_production_failure "agent timeout"
  exit 1
}
merge_commit=$(echo "$st" | jq -r .result_commit_oid)
[[ ${#merge_commit} -eq 40 ]]

echo "step=members_pr_ci_merge"
api POST /api/v1/orgs/demo/members \
  "$(jq -n --arg p 'Prod-Reviewer-12345678' '{handle:"prod-rev",password:$p,role:"member"}')" >/dev/null
api POST /api/v1/orgs/demo/members \
  "$(jq -n --arg p 'ProdCiReporter12345678' '{handle:"prod-ci",password:$p,role:"admin"}')" >/dev/null
REV_JAR=$(mktemp)
CI_JAR=$(mktemp)
curl -fsS -c "$REV_JAR" -b "$REV_JAR" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg h prod-rev --arg p 'Prod-Reviewer-12345678' '{handle:$h,password:$p}')" \
  "$BASE/api/v1/auth/login" >/dev/null
curl -fsS -c "$CI_JAR" -b "$CI_JAR" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg h prod-ci --arg p 'ProdCiReporter12345678' '{handle:$h,password:$p}')" \
  "$BASE/api/v1/auth/login" >/dev/null
api POST /api/v1/orgs/demo/repos/prod-slice/collaborators \
  '{"handle":"prod-rev","role":"maintainer"}' >/dev/null
branch="agent/${run_id}"
pr=$(api POST /api/v1/orgs/demo/repos/prod-slice/pull-requests \
  "$(jq -n --arg t "prod pr" --arg sb "$branch" --arg iid "$issue_id" --arg ar "$run_id" \
    '{title:$t,source_branch:$sb,issue_id:$iid,agent_run_id:$ar}')")
pr_n=$(echo "$pr" | jq -r .number)
pr_id=$(echo "$pr" | jq -r .id)
rev_code=$(curl -sS -c "$REV_JAR" -b "$REV_JAR" -H 'Content-Type: application/json' \
  -d '{"decision":"approve"}' -o /dev/null -w "%{http_code}" \
  "$BASE/api/v1/orgs/demo/repos/prod-slice/pull-requests/${pr_n}/reviews")
[[ "$rev_code" =~ ^2 ]]
attempt_id=$(cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen)
ci_chk=$(curl -sS -c "$CI_JAR" -b "$CI_JAR" -X PATCH -H 'Content-Type: application/json' \
  -d "$(jq -n --arg a "$attempt_id" '{state:"success",attempt_id:$a}')" \
  -o /dev/null -w "%{http_code}" \
  "$BASE/api/v1/orgs/demo/repos/prod-slice/pull-requests/${pr_n}/checks/product-ci")
[[ "$ci_chk" =~ ^2 ]]
merged_code=$(curl -sS -c "$REV_JAR" -b "$REV_JAR" -X POST -o /tmp/prod-merge.json -w "%{http_code}" \
  "$BASE/api/v1/orgs/demo/repos/prod-slice/pull-requests/${pr_n}/merge")
[[ "$merged_code" == 2* ]]
merge_oid=$(jq -r .head_oid /tmp/prod-merge.json)

echo "step=pipeline_oci"
pipe=$(api POST /api/v1/orgs/demo/repos/prod-slice/pipeline-runs \
  "$(jq -n --arg o "$merge_oid" --arg p "$pr_id" --arg i "$issue_id" --arg a "$run_id" \
    '{source_oid:$o,pr_id:$p,issue_id:$i,agent_run_id:$a,run_kind:"release_build"}')")
pipe_id=$(echo "$pipe" | jq -r .id)
[[ "$(echo "$pipe" | jq -r .state)" == "queued" ]] || true
for _ in $(seq 1 600); do
  pst=$(api GET "/api/v1/orgs/demo/repos/prod-slice/pipeline-runs/${pipe_id}")
  pstate=$(echo "$pst" | jq -r .state)
  [[ "$pstate" == "succeeded" ]] && break
  [[ "$pstate" == "failed" ]] && { echo "$pst" >&2; dc logs musehub-runner --tail=50 >&2; exit 1; }
  sleep 1
done
[[ "$pstate" == "succeeded" ]]

echo "step=release_deploy_worker"
rel=$(api POST /api/v1/orgs/demo/repos/prod-slice/releases \
  "$(jq -n --arg o "$merge_oid" --arg p "$pipe_id" --arg pr "$pr_id" \
    '{version:"0.0.0-prod",tag_name:"v0.0.0-prod",commit_oid:$o,pipeline_run_id:$p,pr_id:$pr}')")
rel_id=$(echo "$rel" | jq -r .id)
api POST "/api/v1/orgs/demo/repos/prod-slice/releases/${rel_id}/publish" >/dev/null
rel_row=$(dc exec -T postgres psql -U musehub -d musehub -Atc \
  "SELECT coalesce(oci_image_ref,'') FROM releases WHERE id='$rel_id'")
[[ -n "$rel_row" ]] || { echo "release missing oci_image_ref" >&2; exit 1; }

dep_code=$(curl -sS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -o /tmp/prod-dep.json -w "%{http_code}" \
  -X POST -H 'Content-Type: application/json' \
  -d "$(jq -n --arg r "$rel_id" '{release_id:$r}')" \
  "$BASE/api/v1/orgs/demo/repos/prod-slice/deployments")
[[ "$dep_code" == "202" ]] || { cat /tmp/prod-dep.json >&2; exit 1; }
dep_id=$(jq -r .id /tmp/prod-dep.json)
dep_state=""
for _ in $(seq 1 600); do
  dep=$(api GET "/api/v1/orgs/demo/repos/prod-slice/deployments/${dep_id}")
  dep_state=$(echo "$dep" | jq -r .state)
  [[ "$dep_state" == "succeeded" ]] && break
  [[ "$dep_state" == "failed" ]] && { echo "$dep" >&2; dc logs musehub-runner --tail=30 >&2; exit 1; }
  sleep 1
done
[[ "$dep_state" == "succeeded" ]]

holder=$(dc exec -T postgres psql -U musehub -d musehub -Atc \
  "SELECT lease_holder IS NOT NULL FROM deployments WHERE id='$dep_id'" | tr -d '[:space:]')
[[ "$holder" == "t" ]] || { echo "deployment missing lease_holder (worker claim)" >&2; exit 1; }

trace=$(api GET "/api/v1/orgs/demo/repos/prod-slice/trace/deployments/${dep_id}")
echo "$trace" | jq -e ".pipeline_run_id == \"$pipe_id\"" >/dev/null
echo "$dep" | jq -e '.health_summary.status == "healthy"' >/dev/null

echo "step=rollback"
rb_code=$(curl -sS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -o /tmp/prod-rb.json -w "%{http_code}" \
  -X POST -H 'Content-Type: application/json' \
  -d "$(jq -n --arg t "$dep_id" '{target_deployment_id:$t,reason:"prod acceptance rollback"}')" \
  "$BASE/api/v1/orgs/demo/repos/prod-slice/deployments/rollback")
[[ "$rb_code" == "202" ]] || [[ "$rb_code" == "200" ]] || { cat /tmp/prod-rb.json >&2; exit 1; }
rb_id=$(jq -r .id /tmp/prod-rb.json)
for _ in $(seq 1 600); do
  rb=$(api GET "/api/v1/orgs/demo/repos/prod-slice/deployments/${rb_id}")
  rb_state=$(echo "$rb" | jq -r .state)
  [[ "$rb_state" == "succeeded" ]] && break
  [[ "$rb_state" == "failed" ]] && { echo "$rb" >&2; exit 1; }
  sleep 1
done
[[ "$rb_state" == "succeeded" ]]

echo "product_production_stack_e2e=ok run_id=$run_id pipe_id=$pipe_id deployment=$dep_id rollback=$rb_id"
