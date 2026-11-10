#!/usr/bin/env bash
# Subprocess acceptance: API (remote deploy executor) + musehub-runnerd deployment claim.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export DATABASE_URL="${MUSEHUB_TEST_DATABASE_URL:-${DATABASE_URL:-}}"
test -n "$DATABASE_URL" || { echo "MUSEHUB_TEST_DATABASE_URL or DATABASE_URL required" >&2; exit 1; }

# shellcheck source=lib/remote-runner-e2e-common.sh
source "$ROOT/scripts/lib/remote-runner-e2e-common.sh"
export MUSEHUB_DEPLOY_EXECUTOR=remote
export MUSEHUB_COMPOSE_ADAPTER_MODE=mock
export MUSEHUB_RUNNER_WORK_KINDS=all

COMPOSE_MOCK="${CARGO_TARGET_DIR:-$ROOT/target}/debug/musehub-compose-mock"
COMPOSE_RUNNER="${CARGO_TARGET_DIR:-$ROOT/target}/debug/musehub-compose-runner"
export MUSEHUB_COMPOSE_ADAPTER_BIN="$COMPOSE_MOCK"
export MUSEHUB_COMPOSE_RUNNER_BIN="$COMPOSE_RUNNER"

remote_runner_e2e_init
trap 'remote_runner_e2e_cleanup' EXIT

remote_runner_start_server
COOKIE=$(remote_runner_login_cookie)
export COOKIE
RUNNER_MEMBER="deploy-runner-$(e2e_uuid | cut -c1-8)"
remote_runner_bootstrap_repo "deploy-pipe-$(e2e_uuid | cut -c1-8)" "$RUNNER_MEMBER" "Deploy-Runner-Member-1!"

PAT_JSON=$(curl -sf -X POST "http://127.0.0.1:${API_PORT}/api/v1/tokens" \
  -H "Content-Type: application/json" -H "Cookie: musehub_session=$COOKIE" \
  -d "{\"name\":\"deploy\",\"org\":\"demo\",\"scopes\":[\"deploy:execute\",\"repo:read\"],\"scope_mode\":\"selected\",\"repo_ids\":[\"$REPO_ID\"],\"runner_handle\":\"$RUNNER_MEMBER\"}")
RUNNER_PAT=$(echo "$PAT_JSON" | jq -r .token)

ADMIN_ID=$(psql "$DATABASE_URL" -Atc "SELECT id FROM principals WHERE handle_normalized='admin'")
ORG_ID=$(psql "$DATABASE_URL" -Atc "SELECT org_id FROM repositories WHERE id='$REPO_ID'")
REL_ID=$(e2e_uuid)
PIPE_ID=$(e2e_uuid)
ART_DIR="$GIT_ROOT/artifacts/$ORG_ID/$REPO_ID/$PIPE_ID"
mkdir -p "$ART_DIR"
echo bundle >"$ART_DIR/bundle.tar"
MANIFEST=$(printf 'a%.0s' {1..64})
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 <<-SQL
  INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state, requested_by, run_attempt_id, run_kind)
    VALUES ('$PIPE_ID', '$ORG_ID', '$REPO_ID', '$(printf 'b%.0s' {1..40})', 'product-ci', 'succeeded', '$ADMIN_ID', '$(e2e_uuid)', 'release_build');
  INSERT INTO artifacts (id, org_id, repo_id, pipeline_run_id, name, sha256, byte_length, storage_path, state)
    VALUES ('$(e2e_uuid)', '$ORG_ID', '$REPO_ID', '$PIPE_ID', 'bundle.tar', '$(sha256sum "$ART_DIR/bundle.tar" | awk '{print $1}')', 7, '$ART_DIR', 'ready');
  INSERT INTO releases (id, org_id, repo_id, version, tag_name, commit_oid, pipeline_run_id, state, manifest_sha256)
    VALUES ('$REL_ID', '$ORG_ID', '$REPO_ID', '0.0.1', 'v0.0.1', '$(printf 'c%.0s' {1..40})', '$PIPE_ID', 'published', '$MANIFEST');
SQL

curl -sf -b "musehub_session=$COOKIE" "http://127.0.0.1:${API_PORT}/api/v1/runner/capabilities" \
  | jq -e '.deploy_executor == "remote"' >/dev/null

DEP_CODE=$(curl -sS -b "musehub_session=$COOKIE" -o /tmp/dep.json -w "%{http_code}" \
  -X POST -H 'Content-Type: application/json' \
  -d "{\"release_id\":\"$REL_ID\",\"health_target\":\"mock://healthy\"}" \
  "http://127.0.0.1:${API_PORT}/api/v1/orgs/demo/repos/$REPO_SLUG/deployments")
[[ "$DEP_CODE" == "202" ]] || { cat /tmp/dep.json >&2; exit 1; }
DEP_ID=$(jq -r .id /tmp/dep.json)

export RUNNER_PAT
remote_runner_start_runnerd "$COMPOSE_MOCK"

for _ in $(seq 1 120); do
  st=$(psql "$DATABASE_URL" -Atc "SELECT state FROM deployments WHERE id='$DEP_ID'")
  if [[ "$st" == "succeeded" ]]; then
    echo "remote_deployment_subprocess_e2e=ok"
    exit 0
  fi
  [[ "$st" == "failed" ]] && break
  sleep 1
done
echo "deployment ended: $st" >&2
exit 1
