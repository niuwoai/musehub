#!/usr/bin/env bash
# Product stack: clean build + fresh volumes → gateway static web + full API slice →
# restart persistence, migrate idempotency, backup/restore, upgrade recreate.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

command -v docker >/dev/null || { echo "docker required" >&2; exit 1; }
command -v jq >/dev/null || { echo "jq required" >&2; exit 1; }
command -v curl >/dev/null || { echo "curl required" >&2; exit 1; }
command -v pg_dump >/dev/null || { echo "pg_dump required" >&2; exit 1; }
command -v pg_restore >/dev/null || { echo "pg_restore required" >&2; exit 1; }

STACK_ID="musehub-stack-ci-$(date +%s)"
export COMPOSE_PROJECT_NAME="$STACK_ID"
ENV_FILE="$ROOT/deploy/.env.ci"
COMPOSE_BASE=(docker compose -f "$ROOT/deploy/docker-compose.yml" -f "$ROOT/deploy/docker-compose.ci.yml" --env-file "$ENV_FILE")
dc() { "${COMPOSE_BASE[@]}" "$@"; }

POSTGRES_PASSWORD="$(openssl rand -hex 16)"
SESSION_SECRET="$(openssl rand -hex 24)"
BOOTSTRAP_PW="Stack-Bootstrap-Strong-$(openssl rand -hex 4)!"

cat >"$ENV_FILE" <<EOF
POSTGRES_PASSWORD=$POSTGRES_PASSWORD
MUSEHUB_SESSION_SECRET=$SESSION_SECRET
MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD=$BOOTSTRAP_PW
MUSEHUB_GATEWAY_HTTP_PORT=18080
MUSEHUB_MCP_PRODUCT=0
EOF
POSTGRES_PASSWORD="$(grep '^POSTGRES_PASSWORD=' "$ENV_FILE" | cut -d= -f2-)"
SESSION_SECRET="$(grep '^MUSEHUB_SESSION_SECRET=' "$ENV_FILE" | cut -d= -f2-)"
BOOTSTRAP_PW="$(grep '^MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD=' "$ENV_FILE" | cut -d= -f2-)"
MUSEHUB_GATEWAY_HTTP_PORT="$(grep '^MUSEHUB_GATEWAY_HTTP_PORT=' "$ENV_FILE" | cut -d= -f2-)"
export POSTGRES_PASSWORD SESSION_SECRET BOOTSTRAP_PW MUSEHUB_GATEWAY_HTTP_PORT

cleanup() {
  dc down -v --remove-orphans 2>/dev/null || true
  rm -f "$ENV_FILE"
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
DATABASE_URL="postgres://musehub:${POSTGRES_PASSWORD}@127.0.0.1:55432/musehub"

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
curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" \
  -H 'Content-Type: application/json' \
  -d "$(jq -n --arg h admin --arg p "$BOOTSTRAP_PW" '{handle:$h,password:$p}')" \
  "$BASE/api/v1/auth/login" >/dev/null

api() {
  local method=$1 uri=$2 data=${3:-}
  if [[ -n "$data" ]]; then
    curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -X "$method" \
      -H 'Content-Type: application/json' -d "$data" "$BASE$uri"
  else
    curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -X "$method" "$BASE$uri"
  fi
}

echo "=== vertical slice via gateway ==="
api POST /api/v1/orgs/demo/repos '{"slug":"stack-slice","description":"product stack"}' >/dev/null
issue=$(api POST /api/v1/orgs/demo/repos/stack-slice/issues '{"title":"stack issue"}')
issue_id=$(echo "$issue" | jq -r .id)
issue_n=$(echo "$issue" | jq -r .number)
api POST "/api/v1/orgs/demo/repos/stack-slice/issues/${issue_n}/plans" '{"body":"frozen plan"}' >/dev/null
curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -X PATCH \
  "$BASE/api/v1/orgs/demo/repos/stack-slice/issues/${issue_n}/plans" >/dev/null

agent=$(api POST /api/v1/orgs/demo/repos/stack-slice/agent-runs \
  "$(jq -n --arg id "$issue_id" '{issue_id:$id,message:"stack agent"}')")
run_id=$(echo "$agent" | jq -r .id)
[[ "$(echo "$agent" | jq -r .adapter_kind)" == "reference" ]]

for _ in $(seq 1 120); do
  st=$(api GET "/api/v1/agent-runs/${run_id}")
  state=$(echo "$st" | jq -r .state)
  [[ "$state" == "succeeded" ]] && break
  [[ "$state" == "failed" ]] && { echo "$st" >&2; exit 1; }
  sleep 0.25
done
merge_commit=$(echo "$st" | jq -r .result_commit_oid)
[[ ${#merge_commit} -eq 40 ]]

api POST /api/v1/orgs/demo/members \
  "$(jq -n --arg p 'Stack-Reviewer-1!' '{handle:"stack-rev",password:$p,role:"member"}')" >/dev/null
api POST /api/v1/orgs/demo/members \
  "$(jq -n --arg p 'Stack-CI-1!' '{handle:"stack-ci",password:$p,role:"admin"}')" >/dev/null

CI_JAR=$(mktemp)
REV_JAR=$(mktemp)
curl -fsS -c "$REV_JAR" -b "$REV_JAR" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg h stack-rev --arg p 'Stack-Reviewer-1!' '{handle:$h,password:$p}')" \
  "$BASE/api/v1/auth/login" >/dev/null
curl -fsS -c "$CI_JAR" -b "$CI_JAR" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg h stack-ci --arg p 'Stack-CI-1!' '{handle:$h,password:$p}')" \
  "$BASE/api/v1/auth/login" >/dev/null

branch="agent/${run_id}"
pr=$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg t "stack pr" --arg sb "$branch" --arg iid "$issue_id" --arg ar "$run_id" \
    '{title:$t,source_branch:$sb,issue_id:$iid,agent_run_id:$ar}')" \
  "$BASE/api/v1/orgs/demo/repos/stack-slice/pull-requests")
pr_n=$(echo "$pr" | jq -r .number)
pr_id=$(echo "$pr" | jq -r .id)

curl -fsS -c "$REV_JAR" -b "$REV_JAR" -H 'Content-Type: application/json' \
  -d '{"decision":"approve"}' \
  "$BASE/api/v1/orgs/demo/repos/stack-slice/pull-requests/${pr_n}/reviews" >/dev/null
attempt_id=$(cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen)
curl -fsS -c "$CI_JAR" -b "$CI_JAR" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg a "$attempt_id" '{state:"success",attempt_id:$a}')" \
  "$BASE/api/v1/orgs/demo/repos/stack-slice/pull-requests/${pr_n}/checks/product-ci" >/dev/null
merged=$(curl -fsS -c "$REV_JAR" -b "$REV_JAR" -X POST \
  "$BASE/api/v1/orgs/demo/repos/stack-slice/pull-requests/${pr_n}/merge")
merge_oid=$(echo "$merged" | jq -r .head_oid)

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
curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -X POST \
  "$BASE/api/v1/orgs/demo/repos/stack-slice/releases/${rel_id}/publish" >/dev/null
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
