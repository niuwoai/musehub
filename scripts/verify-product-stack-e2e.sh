#!/usr/bin/env bash
# Product stack: clean build + fresh volumes → gateway static web + full API slice →
# restart persistence, migrate idempotency, backup/restore, upgrade recreate.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
# Legacy builder avoids buildx lock issues on shared CI/VM runners.
export DOCKER_BUILDKIT="${DOCKER_BUILDKIT:-0}"
export COMPOSE_DOCKER_CLI_BUILD="${COMPOSE_DOCKER_CLI_BUILD:-0}"

command -v docker >/dev/null || { echo "docker required" >&2; exit 1; }
command -v jq >/dev/null || { echo "jq required" >&2; exit 1; }
command -v curl >/dev/null || { echo "curl required" >&2; exit 1; }
command -v pg_dump >/dev/null || { echo "pg_dump required" >&2; exit 1; }
command -v pg_restore >/dev/null || { echo "pg_restore required" >&2; exit 1; }

STACK_ID="musehub-stack-ci-$(date +%s)"
export COMPOSE_PROJECT_NAME="$STACK_ID"
ENV_FILE="$ROOT/deploy/.env.ci"
CADDY_CI_HOST="$ROOT/deploy/.caddy.ci.host.${STACK_ID}.yml"
MUSEHUB_CI_POSTGRES_PORT=$((40000 + RANDOM % 20000))
MUSEHUB_CI_API_HOST_PORT=$((50000 + RANDOM % 15000))
export MUSEHUB_CI_POSTGRES_PORT MUSEHUB_CI_API_HOST_PORT
sed "s/host.docker.internal:58080/host.docker.internal:${MUSEHUB_CI_API_HOST_PORT}/g" \
  "$ROOT/deploy/Caddyfile.ci.host.yml" >"$CADDY_CI_HOST"
COMPOSE_CI_EXTRA="$ROOT/deploy/.compose.ci.${STACK_ID}.yml"
cat >"$COMPOSE_CI_EXTRA" <<EOF
services:
  gateway:
    volumes:
      - ${CADDY_CI_HOST}:/etc/caddy/Caddyfile:ro
EOF
COMPOSE_BASE=(docker compose -f "$ROOT/deploy/docker-compose.yml" -f "$ROOT/deploy/docker-compose.ci.yml" -f "$COMPOSE_CI_EXTRA" --env-file "$ENV_FILE")
dc() { "${COMPOSE_BASE[@]}" "$@"; }

POSTGRES_PASSWORD="$(openssl rand -hex 16)"
SESSION_SECRET="$(openssl rand -hex 24)"
BOOTSTRAP_PW="StackBootstrap$(openssl rand -hex 8)"

cat >"$ENV_FILE" <<EOF
POSTGRES_PASSWORD=$POSTGRES_PASSWORD
MUSEHUB_SESSION_SECRET=$SESSION_SECRET
MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD=$BOOTSTRAP_PW
MUSEHUB_GATEWAY_HTTP_PORT=18080
MUSEHUB_MCP_PRODUCT=0
EOF
{
  echo "MUSEHUB_CI_POSTGRES_PORT=$MUSEHUB_CI_POSTGRES_PORT"
  echo "MUSEHUB_CI_API_HOST_PORT=$MUSEHUB_CI_API_HOST_PORT"
} >>"$ENV_FILE"
POSTGRES_PASSWORD="$(grep '^POSTGRES_PASSWORD=' "$ENV_FILE" | cut -d= -f2-)"
SESSION_SECRET="$(grep '^MUSEHUB_SESSION_SECRET=' "$ENV_FILE" | cut -d= -f2-)"
BOOTSTRAP_PW="$(grep '^MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD=' "$ENV_FILE" | cut -d= -f2-)"
MUSEHUB_GATEWAY_HTTP_PORT="$(grep '^MUSEHUB_GATEWAY_HTTP_PORT=' "$ENV_FILE" | cut -d= -f2-)"
export POSTGRES_PASSWORD SESSION_SECRET BOOTSTRAP_PW MUSEHUB_GATEWAY_HTTP_PORT

cleanup() {
  dc down -v --remove-orphans 2>/dev/null || true
  rm -f "$ENV_FILE" "$CADDY_CI_HOST" "$COMPOSE_CI_EXTRA"
}
trap cleanup EXIT

if grep -v '^[[:space:]]*#' "$ROOT/deploy/docker-compose.yml" | grep -q 'docker\.sock'; then
  echo "product stack compose must not mount docker.sock" >&2
  exit 1
fi

echo "=== compose build (server + gateway) ==="
dc build --pull

echo "=== compose up ==="
dc up -d

BASE="http://127.0.0.1:${MUSEHUB_GATEWAY_HTTP_PORT}"
DATABASE_URL="postgres://musehub:${POSTGRES_PASSWORD}@127.0.0.1:${MUSEHUB_CI_POSTGRES_PORT}/musehub"

wait_ready() {
  for _ in $(seq 1 90); do
    if curl -fsS "$BASE/readyz" | jq -e '.ready == true' >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  dc logs --tail=80
  echo "gateway not ready" >&2
  exit 1
}
wait_ready

echo "=== static web (production build, not Vite dev) ==="
html=$(curl -fsS "$BASE/")
echo "$html" | grep -q 'id="root"'
echo "$html" | grep -qE '/assets/|MuseHub'
[[ "$html" != *"/src/main.tsx"* ]] || { echo "dev index leaked into gateway" >&2; exit 1; }

COOKIE_JAR=$(mktemp)
login_json=$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" \
  -H 'Content-Type: application/json' \
  -d "$(jq -n --arg h admin --arg p "$BOOTSTRAP_PW" '{handle:$h,password:$p}')" \
  "$BASE/api/v1/auth/login")
echo "$login_json" | jq -e '.ok == true' >/dev/null
me_json=$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" "$BASE/api/v1/me")
echo "$me_json" | jq -e '.handle == "admin"' >/dev/null

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

echo "=== vertical slice via gateway ==="
echo "step=create_repo"
api POST /api/v1/orgs/demo/repos '{"slug":"stack-slice","description":"product stack"}' >/dev/null
echo "step=create_issue"
issue=$(api POST /api/v1/orgs/demo/repos/stack-slice/issues '{"title":"stack issue"}')
issue_id=$(echo "$issue" | jq -r .id)
issue_n=$(echo "$issue" | jq -r .number)
echo "step=create_plan"
api POST "/api/v1/orgs/demo/repos/stack-slice/issues/${issue_n}/plans" '{"body":"frozen plan"}' >/dev/null
echo "step=accept_plan"
api PATCH "/api/v1/orgs/demo/repos/stack-slice/issues/${issue_n}/plans"

echo "step=agent_run"
agent=$(api POST /api/v1/orgs/demo/repos/stack-slice/agent-runs \
  "$(jq -n --arg id "$issue_id" '{issue_id:$id,message:"stack agent"}')")
run_id=$(echo "$agent" | jq -r .id)
[[ "$(echo "$agent" | jq -r .adapter_kind)" == "reference" ]]

final_state=""
for _ in $(seq 1 120); do
  st=$(api GET "/api/v1/agent-runs/${run_id}")
  final_state=$(echo "$st" | jq -r .state)
  [[ "$final_state" == "succeeded" ]] && break
  [[ "$final_state" == "failed" ]] && { echo "$st" >&2; exit 1; }
  sleep 0.25
done
[[ "$final_state" == "succeeded" ]] || {
  echo "agent run did not succeed: ${st:-unknown}" >&2
  exit 1
}
merge_commit=$(echo "$st" | jq -r .result_commit_oid)
[[ ${#merge_commit} -eq 40 ]]

echo "step=seed_members"
api POST /api/v1/orgs/demo/members \
  "$(jq -n --arg p 'Stack-Reviewer-12345678' '{handle:"stack-rev",password:$p,role:"member"}')" >/dev/null
api POST /api/v1/orgs/demo/members \
  "$(jq -n --arg p 'StackCiReporter12345678' '{handle:"stack-ci",password:$p,role:"admin"}')" >/dev/null

CI_JAR=$(mktemp)
REV_JAR=$(mktemp)
rev_login=$(curl -fsS -c "$REV_JAR" -b "$REV_JAR" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg h stack-rev --arg p 'Stack-Reviewer-12345678' '{handle:$h,password:$p}')" \
  "$BASE/api/v1/auth/login")
echo "$rev_login" | jq -e '.ok == true' >/dev/null
ci_login=$(curl -fsS -c "$CI_JAR" -b "$CI_JAR" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg h stack-ci --arg p 'StackCiReporter12345678' '{handle:$h,password:$p}')" \
  "$BASE/api/v1/auth/login")
echo "$ci_login" | jq -e '.ok == true' >/dev/null

echo "step=pr_collaborator"
api POST /api/v1/orgs/demo/repos/stack-slice/collaborators \
  '{"handle":"stack-rev","role":"maintainer"}' >/dev/null

branch="agent/${run_id}"
echo "step=open_pr"
pr=$(api POST /api/v1/orgs/demo/repos/stack-slice/pull-requests \
  "$(jq -n --arg t "stack pr" --arg sb "$branch" --arg iid "$issue_id" --arg ar "$run_id" \
    '{title:$t,source_branch:$sb,issue_id:$iid,agent_run_id:$ar}')")
pr_n=$(echo "$pr" | jq -r .number)
pr_id=$(echo "$pr" | jq -r .id)

echo "step=review_pr"
rev_tmp=$(mktemp)
rev_code=$(curl -sS -c "$REV_JAR" -b "$REV_JAR" -H 'Content-Type: application/json' \
  -d '{"decision":"approve"}' -o "$rev_tmp" -w "%{http_code}" \
  "$BASE/api/v1/orgs/demo/repos/stack-slice/pull-requests/${pr_n}/reviews")
[[ "$rev_code" =~ ^2 ]] || {
  echo "review failed http=${rev_code} body=$(cat "$rev_tmp")" >&2
  exit 1
}
rm -f "$rev_tmp"

echo "step=ci_check"
attempt_id=$(cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen)
ci_chk=$(curl -sS -c "$CI_JAR" -b "$CI_JAR" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg a "$attempt_id" '{state:"success",attempt_id:$a}')" \
  -o /dev/null -w "%{http_code}" \
  "$BASE/api/v1/orgs/demo/repos/stack-slice/pull-requests/${pr_n}/checks/product-ci")
[[ "$ci_chk" =~ ^2 ]] || { echo "ci check failed http=$ci_chk" >&2; exit 1; }

echo "step=merge_pr"
merged=$(curl -sS -c "$REV_JAR" -b "$REV_JAR" -X POST -o /tmp/merge.json -w "%{http_code}" \
  "$BASE/api/v1/orgs/demo/repos/stack-slice/pull-requests/${pr_n}/merge")
[[ "$merged" == 2* ]] || { cat /tmp/merge.json >&2; exit 1; }
merge_oid=$(jq -r .head_oid /tmp/merge.json)

pipe=$(api POST /api/v1/orgs/demo/repos/stack-slice/pipeline-runs \
  "$(jq -n --arg o "$merge_oid" --arg p "$pr_id" --arg i "$issue_id" --arg a "$run_id" \
    '{source_oid:$o,pr_id:$p,issue_id:$i,agent_run_id:$a}')")
pipe_id=$(echo "$pipe" | jq -r .id)
for _ in $(seq 1 120); do
  pst=$(api GET "/api/v1/orgs/demo/repos/stack-slice/pipeline-runs/${pipe_id}")
  [[ "$(echo "$pst" | jq -r .state)" == "succeeded" ]] && break
  sleep 0.2
done

rel=$(api POST /api/v1/orgs/demo/repos/stack-slice/releases \
  "$(jq -n --arg o "$merge_oid" --arg p "$pipe_id" --arg pr "$pr_id" \
    '{version:"0.0.0-stack",tag_name:"v0.0.0-stack",commit_oid:$o,pipeline_run_id:$p,pr_id:$pr}')")
rel_id=$(echo "$rel" | jq -r .id)
api POST "/api/v1/orgs/demo/repos/stack-slice/releases/${rel_id}/publish"
dep=$(api POST /api/v1/orgs/demo/repos/stack-slice/deployments \
  "$(jq -n --arg r "$rel_id" '{release_id:$r,health_target:"mock://healthy"}')")
[[ "$(echo "$dep" | jq -r .state)" == "succeeded" ]]
dep_id=$(echo "$dep" | jq -r .id)
trace=$(api GET "/api/v1/orgs/demo/repos/stack-slice/trace/deployments/${dep_id}")
[[ "$(echo "$trace" | jq -r .pipeline_run_id)" == "$pipe_id" ]]
echo "product_stack_slice=ok deployment=$dep_id"

MARKER="stack-marker-$(date +%s)"
echo -n "$MARKER" | dc exec -T \
  musehub-server sh -c "mkdir -p /var/lib/musehub/artifacts/stack && cat > /var/lib/musehub/artifacts/stack/marker.txt"

echo "=== restart persistence ==="
dc restart musehub-server gateway
wait_ready
repos=$(api GET /api/v1/orgs/demo/repos)
echo "$repos" | jq -e '.[] | select(.slug=="stack-slice")' >/dev/null
marker_after=$(dc exec -T musehub-server \
  cat /var/lib/musehub/artifacts/stack/marker.txt)
[[ "$marker_after" == "$MARKER" ]]
echo "restart_persistence=ok"

echo "=== migrate idempotency ==="
dc run --rm --no-deps \
  --entrypoint /docker-entrypoint.sh musehub-server migrate-only
dc run --rm --no-deps \
  --entrypoint /docker-entrypoint.sh musehub-server migrate-only
echo "migrate_idempotent=ok"

echo "=== backup / wipe / restore ==="
BACKUP_DIR=$(mktemp -d)
GIT_EX=$(mktemp -d)
ART_EX=$(mktemp -d)
dc cp \
  musehub-server:/var/lib/musehub/git/. "$GIT_EX/git"
dc cp \
  musehub-server:/var/lib/musehub/artifacts/. "$ART_EX/artifacts"
export MUSEHUB_GIT_ROOT="$GIT_EX/git"
export MUSEHUB_ARTIFACT_ROOT="$ART_EX/artifacts"
chmod +x "$ROOT/scripts/backup-musehub-state.sh" "$ROOT/scripts/restore-musehub-state.sh"
"$ROOT/scripts/backup-musehub-state.sh" "$BACKUP_DIR"

dc stop musehub-server gateway
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -c "DROP SCHEMA public CASCADE; CREATE SCHEMA public;"
rm -rf "$GIT_EX/git" "$ART_EX/artifacts"
mkdir -p "$GIT_EX/git" "$ART_EX/artifacts"
"$ROOT/scripts/restore-musehub-state.sh" "$BACKUP_DIR"
GIT_VOL=$(docker volume ls -q | grep "${STACK_ID}.*gitdata" | head -1)
ART_VOL=$(docker volume ls -q | grep "${STACK_ID}.*artifacts" | head -1)
[[ -n "$GIT_VOL" && -n "$ART_VOL" ]] || { echo "missing compose volumes" >&2; exit 1; }
docker run --rm -v "${GIT_VOL}:/dest" -v "$GIT_EX/git:/src:ro" alpine:3.20 \
  sh -c 'rm -rf /dest/* && cp -a /src/. /dest/'
docker run --rm -v "${ART_VOL}:/dest" -v "$ART_EX/artifacts:/src:ro" alpine:3.20 \
  sh -c 'rm -rf /dest/* && cp -a /src/. /dest/'
dc start musehub-server gateway
wait_ready
repos2=$(api GET /api/v1/orgs/demo/repos)
echo "$repos2" | jq -e '.[] | select(.slug=="stack-slice")' >/dev/null
echo "backup_restore=ok"

echo "=== image upgrade (recreate server) ==="
dc build musehub-server
dc up -d --force-recreate musehub-server gateway
wait_ready
repos3=$(api GET /api/v1/orgs/demo/repos)
echo "$repos3" | jq -e '.[] | select(.slug=="stack-slice")' >/dev/null
echo "upgrade_persistence=ok"

CID=$(dc ps -q musehub-server)
BINDS=$(docker inspect "$CID" --format '{{json .HostConfig.Binds}}')
[[ "$BINDS" != *"docker.sock"* ]]
echo "product_stack_e2e=ok"
