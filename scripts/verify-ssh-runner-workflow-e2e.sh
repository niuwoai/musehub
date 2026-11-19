#!/usr/bin/env bash
# Real SSH receive-pack -> PostgreSQL outbox -> workflow -> isolated remote Runner.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

for command in docker docker-compose colima curl jq git ssh-keygen ssh-keyscan grep uuidgen openssl df awk; do
  command -v "$command" >/dev/null || { echo "$command required" >&2; exit 1; }
done

DOCKER_CONTEXT_PREFIX="colima-musehub-ssh-runner"
DOCKER_CONTEXT_PREFIX="${DOCKER_CONTEXT_PREFIX}-e2e"
MIN_FREE_DISK_KB=10485760
ACTIVE_DOCKER_CONTEXT="$(docker context show)"
case "$ACTIVE_DOCKER_CONTEXT" in
  "$DOCKER_CONTEXT_PREFIX"|"$DOCKER_CONTEXT_PREFIX"-*) ;;
  *)
    printf 'Select a dedicated SSH/Runner acceptance Docker context (%s or a suffixed profile; active: %s)\n' \
      "$DOCKER_CONTEXT_PREFIX" "$ACTIVE_DOCKER_CONTEXT" >&2
    exit 1
    ;;
esac

COLIMA_PROFILE="${ACTIVE_DOCKER_CONTEXT#colima-}"
DOCKER_ENDPOINT="$(docker context inspect --format '{{ (index .Endpoints "docker").Host }}' "$ACTIVE_DOCKER_CONTEXT")"
EXPECTED_DOCKER_ENDPOINT="unix://${HOME}/.colima/${COLIMA_PROFILE}/docker.sock"
if [[ "$DOCKER_ENDPOINT" != "$EXPECTED_DOCKER_ENDPOINT" ]]; then
  printf 'Dedicated context %s must use its exact Colima profile socket (active endpoint: %s)\n' \
    "$ACTIVE_DOCKER_CONTEXT" "$DOCKER_ENDPOINT" >&2
  exit 1
fi
if ! colima list --json | jq -es --arg profile "$COLIMA_PROFILE" \
  'any(.[]; .name == $profile and .status == "Running" and .runtime == "docker")' >/dev/null; then
  printf 'Matching Colima Docker profile is not running: %s\n' "$COLIMA_PROFILE" >&2
  exit 1
fi

AVAILABLE_DISK_KB="$(df -Pk "$ROOT" | awk 'NR == 2 { print $4 }')"
if [[ ! "$AVAILABLE_DISK_KB" =~ ^[0-9]+$ ]]; then
  echo "Could not determine free disk space for the acceptance workspace" >&2
  exit 1
fi
if (( AVAILABLE_DISK_KB < MIN_FREE_DISK_KB )); then
  printf 'At least 10 GiB free on the workspace filesystem is required (available: %s KiB)\n' \
    "$AVAILABLE_DISK_KB" >&2
  exit 1
fi

echo "Checking that the acceptance uses an isolated Compose project and nested job daemon"
if docker-compose version --short | awk -F. '{ exit !($1 > 2 || ($1 == 2 && $2 >= 24)) }'; then
  :
else
  echo "Docker Compose 2.24+ required for !override isolation tags" >&2
  exit 1
fi

STACK_ID="musehub-ssh-runner-e2e-$(uuidgen | tr '[:upper:]' '[:lower:]')"

# Check for collisions before cleanup is allowed to remove anything.
if docker-compose ls --format json | jq -e --arg project "$STACK_ID" '.[] | select(.Name == $project)' >/dev/null; then
  echo "Compose project already exists: $STACK_ID" >&2
  exit 1
fi

SERVER_IMAGE="musehub-e2e-${STACK_ID}-server:local"
GIT_SSH_IMAGE="musehub-e2e-${STACK_ID}-git-ssh:local"
RUNNER_IMAGE="musehub-e2e-${STACK_ID}-runner:local"
for image in "$SERVER_IMAGE" "$GIT_SSH_IMAGE" "$RUNNER_IMAGE"; do
  if docker image inspect "$image" >/dev/null 2>&1; then
    echo "Acceptance image tag already exists: $image" >&2
    exit 1
  fi
done

ENV_FILE=""
COOKIE_JAR=""
KEY_DIR=""
GIT_DIR=""
SERVER_IMAGE_ID=""
GIT_SSH_IMAGE_ID=""
RUNNER_IMAGE_ID=""
OWNER_TOKEN=""
ALPINE_REGISTRY_TAG=""
ALPINE_REGISTRY_IMAGE_ID=""
ALPINE_SOURCE_DIGEST="alpine@sha256:bc21487101ae47172509130ee1092cc273a831968d64b617776d1f69c81e42ab"
ALPINE_DIGEST="$ALPINE_SOURCE_DIGEST"
T1_NODE_SOURCE_IMAGE="${MUSEHUB_T1_NODE_IMAGE:-}"
T1_NODE_REGISTRY_TAG=""
T1_NODE_REGISTRY_IMAGE_ID=""
T1_NODE_DIGEST=""
T1_BASELINE_OID=""
T1_FIXTURE_PATH=""
T1_BASELINE_RUN_ID=""
T1_ISSUE_ID=""
T1_PR_ID=""
if [[ -n "$T1_NODE_SOURCE_IMAGE" && ! "$T1_NODE_SOURCE_IMAGE" =~ ^[^[:space:]]+@sha256:[[:xdigit:]]{64}$ ]]; then
  echo "MUSEHUB_T1_NODE_IMAGE must be an image pinned by sha256 digest" >&2
  exit 2
fi
ALLOWED_IMAGES="$ALPINE_DIGEST"

cleanup() {
  local status=$?
  local tag image_id current_id current_owner resource
  trap - EXIT
  if [[ -n "$OWNER_TOKEN" ]]; then
    if [[ "$status" -ne 0 ]]; then
      echo "SSH-to-Runner acceptance failed; collecting isolated Compose diagnostics before cleanup" >&2
      dc ps -a >&2 || true
      dc logs --no-color --timestamps --tail=200 musehub-server musehub-runner docker-job-daemon musehub-git-ssh >&2 || true
      echo "Cleaning only resources labeled for this run" >&2
    fi
    while IFS= read -r resource; do
      [[ -n "$resource" ]] || continue
      docker rm -f "$resource" >/dev/null 2>&1 || true
    done < <(docker ps -aq --filter "label=musehub.e2e.owner=$OWNER_TOKEN" 2>/dev/null || true)
    while IFS= read -r resource; do
      [[ -n "$resource" ]] || continue
      docker volume rm "$resource" >/dev/null 2>&1 || true
    done < <(docker volume ls -q --filter "label=musehub.e2e.owner=$OWNER_TOKEN" 2>/dev/null || true)
    while IFS= read -r resource; do
      [[ -n "$resource" ]] || continue
      docker network rm "$resource" >/dev/null 2>&1 || true
    done < <(docker network ls -q --filter "label=musehub.e2e.owner=$OWNER_TOKEN" 2>/dev/null || true)
  fi
  if [[ -n "$ALPINE_REGISTRY_TAG" && -n "$ALPINE_REGISTRY_IMAGE_ID" ]]; then
    current_id="$(docker image inspect --format '{{.Id}}' "$ALPINE_REGISTRY_TAG" 2>/dev/null || true)"
    if [[ "$current_id" == "$ALPINE_REGISTRY_IMAGE_ID" ]]; then
      docker image rm "$ALPINE_REGISTRY_TAG" >/dev/null 2>&1 || true
    fi
  fi
  if [[ -n "$T1_NODE_REGISTRY_TAG" && -n "$T1_NODE_REGISTRY_IMAGE_ID" ]]; then
    current_id="$(docker image inspect --format '{{.Id}}' "$T1_NODE_REGISTRY_TAG" 2>/dev/null || true)"
    if [[ "$current_id" == "$T1_NODE_REGISTRY_IMAGE_ID" ]]; then
      docker image rm "$T1_NODE_REGISTRY_TAG" >/dev/null 2>&1 || true
    fi
  fi
  for tag in "$SERVER_IMAGE" "$GIT_SSH_IMAGE" "$RUNNER_IMAGE"; do
    case "$tag" in
      "$SERVER_IMAGE") image_id="$SERVER_IMAGE_ID" ;;
      "$GIT_SSH_IMAGE") image_id="$GIT_SSH_IMAGE_ID" ;;
      "$RUNNER_IMAGE") image_id="$RUNNER_IMAGE_ID" ;;
    esac
    current_id="$(docker image inspect --format '{{.Id}}' "$tag" 2>/dev/null || true)"
    current_owner="$(docker image inspect --format '{{ index .Config.Labels "musehub.e2e.owner" }}' "$tag" 2>/dev/null || true)"
    if [[ "$current_owner" == "$OWNER_TOKEN" ]] && { [[ -z "$image_id" ]] || [[ "$current_id" == "$image_id" ]]; }; then
      docker image rm "$tag" >/dev/null 2>&1 || true
    fi
  done
  if [[ -n "$ENV_FILE" ]]; then rm -f "$ENV_FILE"; fi
  if [[ -n "$COOKIE_JAR" ]]; then rm -f "$COOKIE_JAR"; fi
  if [[ -n "$KEY_DIR" ]]; then rm -rf "$KEY_DIR"; fi
  if [[ -n "$GIT_DIR" ]]; then rm -rf "$GIT_DIR"; fi
  if [[ -n "$T1_FIXTURE_PATH" && "$T1_FIXTURE_PATH" == /tmp/musehub-t1-pagination.* ]]; then rm -rf "$T1_FIXTURE_PATH"; fi
  exit "$status"
}
trap cleanup EXIT
trap 'printf "acceptance_error_line=%s\\n" "$LINENO" >&2' ERR
trap 'exit 130' INT
trap 'exit 143' TERM

ENV_FILE="$(mktemp)"
COOKIE_JAR="$(mktemp)"
KEY_DIR="$(mktemp -d)"
GIT_DIR="$(mktemp -d)"
POSTGRES_PASSWORD="$(openssl rand -hex 20)"
SESSION_SECRET="$(openssl rand -hex 32)"
BOOTSTRAP_PASSWORD="Mhub$(openssl rand -hex 12)"
RUNNER_ID="$(uuidgen | tr '[:upper:]' '[:lower:]')"
OWNER_TOKEN="$(openssl rand -hex 32)"

cat >"$ENV_FILE" <<EOF
POSTGRES_PASSWORD=$POSTGRES_PASSWORD
MUSEHUB_SESSION_SECRET=$SESSION_SECRET
MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD=$BOOTSTRAP_PASSWORD
MUSEHUB_MCP_PRODUCT=0
MUSEHUB_REQUIRE_PIPELINE_OCI=1
MUSEHUB_RUNNER_PAT=not-started-yet
MUSEHUB_RUNNER_INSTANCE_ID=$RUNNER_ID
MUSEHUB_JOB_ALLOWED_IMAGES=$ALLOWED_IMAGES
MUSEHUB_E2E_SERVER_IMAGE=$SERVER_IMAGE
MUSEHUB_E2E_GIT_SSH_IMAGE=$GIT_SSH_IMAGE
MUSEHUB_E2E_RUNNER_IMAGE=$RUNNER_IMAGE
MUSEHUB_E2E_OWNER=$OWNER_TOKEN
MUSEHUB_PUBLIC_HTTP_ORIGIN=http://127.0.0.1
MUSEHUB_PUBLIC_SSH_HOST=127.0.0.1
MUSEHUB_GIT_SSH_PORT=2222
EOF

COMPOSE=(docker-compose -p "$STACK_ID" --env-file "$ENV_FILE"
  -f deploy/docker-compose.yml
  -f deploy/docker-compose.production.yml
  -f deploy/docker-compose.ssh-runner-e2e.yml)
dc() { "${COMPOSE[@]}" "$@"; }
BUILD_PROXY_ARGS=()
if [[ -n "${MUSEHUB_E2E_BUILD_PROXY:-}" ]]; then
  BUILD_PROXY_ARGS=(
    --build-arg "HTTP_PROXY=$MUSEHUB_E2E_BUILD_PROXY"
    --build-arg "HTTPS_PROXY=$MUSEHUB_E2E_BUILD_PROXY"
  )
fi

dc build "${BUILD_PROXY_ARGS[@]}" musehub-server musehub-git-ssh musehub-runner >&2
SERVER_IMAGE_ID="$(docker image inspect --format '{{.Id}}' "$SERVER_IMAGE")"
GIT_SSH_IMAGE_ID="$(docker image inspect --format '{{.Id}}' "$GIT_SSH_IMAGE")"
RUNNER_IMAGE_ID="$(docker image inspect --format '{{.Id}}' "$RUNNER_IMAGE")"
dc up -d postgres musehub-server musehub-git-ssh e2e-registry
printf 'acceptance_project=%s\n' "$STACK_ID"

REGISTRY_PORT="$(dc port e2e-registry 5000 | awk -F: '{ print $NF }')"
for _ in $(seq 1 30); do
  if curl -fsS "http://127.0.0.1:${REGISTRY_PORT}/v2/" >/dev/null; then break; fi
  sleep 1
done
curl -fsS "http://127.0.0.1:${REGISTRY_PORT}/v2/" >/dev/null
if ! docker image inspect "$ALPINE_SOURCE_DIGEST" >/dev/null 2>&1; then
  echo "Pulling the pinned Alpine source image into the dedicated Docker profile" >&2
  docker pull "$ALPINE_SOURCE_DIGEST" >&2
fi
docker image inspect "$ALPINE_SOURCE_DIGEST" >/dev/null
REGISTRY_TAG_CANDIDATE="127.0.0.1:${REGISTRY_PORT}/alpine:${STACK_ID##*-}"
if docker image inspect "$REGISTRY_TAG_CANDIDATE" >/dev/null 2>&1; then
  printf 'Refusing to overwrite existing registry seed tag: %s\n' "$REGISTRY_TAG_CANDIDATE" >&2
  exit 1
fi
ALPINE_REGISTRY_TAG="$REGISTRY_TAG_CANDIDATE"
docker image tag "$ALPINE_SOURCE_DIGEST" "$ALPINE_REGISTRY_TAG"
ALPINE_REGISTRY_IMAGE_ID="$(docker image inspect --format '{{.Id}}' "$ALPINE_REGISTRY_TAG")"
REGISTRY_PUSH_OUTPUT="$(docker push "$ALPINE_REGISTRY_TAG" 2>&1)" || {
  printf '%s\n' "$REGISTRY_PUSH_OUTPUT" >&2
  exit 1
}
printf '%s\n' "$REGISTRY_PUSH_OUTPUT" >&2
REGISTRY_DIGEST="$(curl -fsSI \
  -H 'Accept: application/vnd.oci.image.index.v1+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.docker.distribution.manifest.v2+json' \
  "http://127.0.0.1:${REGISTRY_PORT}/v2/alpine/manifests/${STACK_ID##*-}" \
  | awk 'tolower($1) == "docker-content-digest:" { gsub("\\r", "", $2); digest = $2 } END { print digest }')"
[[ "$REGISTRY_DIGEST" =~ ^sha256:[[:xdigit:]]{64}$ ]]
ALPINE_DIGEST="e2e-registry:5000/alpine@${REGISTRY_DIGEST}"
ALLOWED_IMAGES="$ALPINE_DIGEST"
printf 'workflow_image_digest=%s\n' "$REGISTRY_DIGEST"

if [[ -n "$T1_NODE_SOURCE_IMAGE" ]]; then
  if ! docker image inspect "$T1_NODE_SOURCE_IMAGE" >/dev/null 2>&1; then
    echo "Pulling the explicitly pinned Node source image into the dedicated Docker profile" >&2
    docker pull "$T1_NODE_SOURCE_IMAGE" >&2
  fi
  docker image inspect "$T1_NODE_SOURCE_IMAGE" >/dev/null
  NODE_TAG_CANDIDATE="127.0.0.1:${REGISTRY_PORT}/t1-node:${STACK_ID##*-}"
  if docker image inspect "$NODE_TAG_CANDIDATE" >/dev/null 2>&1; then
    printf 'Refusing to overwrite existing T1 image tag: %s\n' "$NODE_TAG_CANDIDATE" >&2
    exit 1
  fi
  T1_NODE_REGISTRY_TAG="$NODE_TAG_CANDIDATE"
  docker image tag "$T1_NODE_SOURCE_IMAGE" "$T1_NODE_REGISTRY_TAG"
  T1_NODE_REGISTRY_IMAGE_ID="$(docker image inspect --format '{{.Id}}' "$T1_NODE_REGISTRY_TAG")"
  NODE_PUSH_OUTPUT="$(docker push "$T1_NODE_REGISTRY_TAG" 2>&1)" || {
    printf '%s\n' "$NODE_PUSH_OUTPUT" >&2
    exit 1
  }
  printf '%s\n' "$NODE_PUSH_OUTPUT" >&2
  NODE_REGISTRY_DIGEST="$(curl -fsSI \
    -H 'Accept: application/vnd.oci.image.index.v1+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.docker.distribution.manifest.v2+json' \
    "http://127.0.0.1:${REGISTRY_PORT}/v2/t1-node/manifests/${STACK_ID##*-}" \
    | awk 'tolower($1) == "docker-content-digest:" { gsub("\\r", "", $2); digest = $2 } END { print digest }')"
  [[ "$NODE_REGISTRY_DIGEST" =~ ^sha256:[[:xdigit:]]{64}$ ]]
  T1_NODE_DIGEST="e2e-registry:5000/t1-node@${NODE_REGISTRY_DIGEST}"
  ALLOWED_IMAGES="$ALPINE_DIGEST,$T1_NODE_DIGEST"
  printf 't1_workflow_image_digest=%s\n' "$NODE_REGISTRY_DIGEST"
  cat >"$ENV_FILE" <<EOF
POSTGRES_PASSWORD=$POSTGRES_PASSWORD
MUSEHUB_SESSION_SECRET=$SESSION_SECRET
MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD=$BOOTSTRAP_PASSWORD
MUSEHUB_MCP_PRODUCT=0
MUSEHUB_REQUIRE_PIPELINE_OCI=1
MUSEHUB_RUNNER_PAT=not-started-yet
MUSEHUB_RUNNER_INSTANCE_ID=$RUNNER_ID
MUSEHUB_JOB_ALLOWED_IMAGES=$ALLOWED_IMAGES
MUSEHUB_E2E_SERVER_IMAGE=$SERVER_IMAGE
MUSEHUB_E2E_GIT_SSH_IMAGE=$GIT_SSH_IMAGE
MUSEHUB_E2E_RUNNER_IMAGE=$RUNNER_IMAGE
MUSEHUB_E2E_OWNER=$OWNER_TOKEN
MUSEHUB_PUBLIC_HTTP_ORIGIN=http://127.0.0.1
MUSEHUB_PUBLIC_SSH_HOST=127.0.0.1
MUSEHUB_GIT_SSH_PORT=2222
EOF
  dc up -d --force-recreate musehub-server
  API_PORT="$(dc port musehub-server 8080 | awk -F: '{ print $NF }')"
  BASE="http://127.0.0.1:${API_PORT}"
  for _ in $(seq 1 180); do
    if curl -fsS "http://127.0.0.1:$(dc port musehub-server 8080 | awk -F: '{print $NF}')/readyz" \
      | jq -e '.ready == true and .checks.product_api == "ok"' >/dev/null 2>&1; then break; fi
    sleep 2
  done
fi
export MUSEHUB_JOB_ALLOWED_IMAGES="$ALLOWED_IMAGES"

wait_ready() {
  for _ in $(seq 1 180); do
    if curl -fsS "$BASE/readyz" | jq -e '.ready == true and .checks.product_api == "ok"' >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  echo "control plane readiness timed out" >&2
  return 1
}

API_PORT="$(dc port musehub-server 8080 | awk -F: '{print $NF}')"
SSH_PORT="$(dc port musehub-git-ssh 2222 | awk -F: '{print $NF}')"
BASE="http://127.0.0.1:${API_PORT}"
wait_ready

for _ in $(seq 1 90); do
  if ssh-keyscan -p "$SSH_PORT" -T 2 127.0.0.1 2>/dev/null | grep -q 'ssh-ed25519'; then break; fi
  sleep 1
done
ssh-keyscan -p "$SSH_PORT" -T 2 127.0.0.1 2>/dev/null | grep -q 'ssh-ed25519' || {
  echo "isolated sshd did not publish its host key" >&2; exit 1;
}

login="$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg p "$BOOTSTRAP_PASSWORD" '{handle:"admin",password:$p}')" \
  "$BASE/api/v1/auth/login")"
echo "$login" | jq -e '.ok == true' >/dev/null

RUNNER_HANDLE="e2e-runner-${STACK_ID##*-}"
RUNNER_PASSWORD="Runner$(openssl rand -hex 12)"
curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg h "$RUNNER_HANDLE" --arg p "$RUNNER_PASSWORD" \
    '{handle:$h,password:$p,role:"member"}')" \
  "$BASE/api/v1/orgs/demo/members" >/dev/null

REPO_SLUG="ssh-runner-${STACK_ID##*-}"
repo="$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg slug "$REPO_SLUG" '{slug:$slug,description:"isolated SSH Runner E2E"}')" \
  "$BASE/api/v1/orgs/demo/repos")"
REPO_ID="$(jq -r .id <<<"$repo")"
[[ "$REPO_ID" != null && -n "$REPO_ID" ]]

curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -X POST -H 'Content-Type: application/json' \
  -d "$(jq -n --arg h "$RUNNER_HANDLE" '{handle:$h,role:"developer"}')" \
  "$BASE/api/v1/orgs/demo/repos/$REPO_SLUG/collaborators" >/dev/null

if [[ -n "$T1_NODE_SOURCE_IMAGE" ]]; then
  ISSUE_JSON="$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg title 'Fix 1-based pagination boundary and add regression tests' \
      --arg body 'paginate(items, page, pageSize) currently skips the first page because page is 1-based but the slice offset is 0-based. Make page=1 return the first page, preserve the final partial page, and add regression tests for page 1, page 2, and the final partial page. Do not change unrelated files.' \
      '{title:$title,body:$body}')" \
    "$BASE/api/v1/orgs/demo/repos/$REPO_SLUG/issues")"
  T1_ISSUE_ID="$(jq -r .id <<<"$ISSUE_JSON")"
  [[ "$T1_ISSUE_ID" != null && -n "$T1_ISSUE_ID" ]]
fi

ssh-keygen -q -t ed25519 -N '' -f "$KEY_DIR/id_ed25519"
chmod 0700 "$KEY_DIR"
pubkey="$(<"$KEY_DIR/id_ed25519.pub")"
SSH_KEY_RESPONSE="$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -X POST -H 'Content-Type: application/json' \
  -d "$(jq -n --arg key "$pubkey" '{name:"isolated-e2e",public_key:$key}')" \
  "$BASE/api/v1/ssh-keys")"
CLIENT_KEY_FP="$(ssh-keygen -lf "$KEY_DIR/id_ed25519.pub" | awk '{print $2}')"
REGISTERED_KEY_FP="$(jq -r .fingerprint <<<"$SSH_KEY_RESPONSE")"
[[ -n "$CLIENT_KEY_FP" && "$CLIENT_KEY_FP" == "$REGISTERED_KEY_FP" ]] || {
  echo "SSH public key registration fingerprint mismatch (client=$CLIENT_KEY_FP registered=$REGISTERED_KEY_FP)" >&2
  exit 1
}
printf 'ssh_key_fingerprint=%s\n' "$CLIENT_KEY_FP"

RUNNER_PAT_JSON="$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -X POST -H 'Content-Type: application/json' \
  -d "$(jq -n --arg id "$REPO_ID" --arg h "$RUNNER_HANDLE" \
    '{name:"isolated-ssh-runner",org:"demo",scope_mode:"selected",repo_ids:[$id],runner_handle:$h,scopes:["pipeline:execute","repo:read"]}')" \
  "$BASE/api/v1/tokens")"
RUNNER_PAT="$(jq -r .token <<<"$RUNNER_PAT_JSON")"
[[ ${#RUNNER_PAT} -gt 20 ]]

cat >"$ENV_FILE" <<EOF
POSTGRES_PASSWORD=$POSTGRES_PASSWORD
MUSEHUB_SESSION_SECRET=$SESSION_SECRET
MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD=$BOOTSTRAP_PASSWORD
MUSEHUB_MCP_PRODUCT=0
MUSEHUB_REQUIRE_PIPELINE_OCI=1
MUSEHUB_RUNNER_PAT=$RUNNER_PAT
MUSEHUB_RUNNER_INSTANCE_ID=$RUNNER_ID
MUSEHUB_JOB_ALLOWED_IMAGES=$ALLOWED_IMAGES
MUSEHUB_E2E_SERVER_IMAGE=$SERVER_IMAGE
MUSEHUB_E2E_GIT_SSH_IMAGE=$GIT_SSH_IMAGE
MUSEHUB_E2E_RUNNER_IMAGE=$RUNNER_IMAGE
MUSEHUB_E2E_OWNER=$OWNER_TOKEN
MUSEHUB_PUBLIC_HTTP_ORIGIN=http://127.0.0.1
MUSEHUB_PUBLIC_SSH_HOST=127.0.0.1
MUSEHUB_GIT_SSH_PORT=2222
EOF

STORAGE_KEY="$(dc exec -T -e PGPASSWORD="$POSTGRES_PASSWORD" postgres psql -U musehub -d musehub -Atc \
  "SELECT storage_key::text FROM repositories WHERE id='$REPO_ID'" | tr -d '\r\n')"
BARE_REPO="/var/lib/musehub/git/${STORAGE_KEY}.git"

GIT_SSH_COMMAND="ssh -i $KEY_DIR/id_ed25519 -p $SSH_PORT -o BatchMode=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o IdentitiesOnly=yes"
export GIT_SSH_COMMAND GIT_TERMINAL_PROMPT=0
GIT_URL="ssh://git@127.0.0.1:${SSH_PORT}/demo/${REPO_SLUG}.git"
if ! git clone "$GIT_URL" "$GIT_DIR"; then
  echo "SSH clone failed; retrying read-only authentication with verbose client diagnostics" >&2
  GIT_SSH_COMMAND="$GIT_SSH_COMMAND -vvv" git ls-remote "$GIT_URL" >&2 || true
  dc logs --no-color --timestamps --tail=200 musehub-git-ssh >&2 || true
  exit 1
fi
git -C "$GIT_DIR" config user.name acceptance
git -C "$GIT_DIR" config user.email acceptance@musehub.test

if [[ -n "$T1_NODE_SOURCE_IMAGE" ]]; then
  T1_FIXTURE_OUTPUT="$(bash "$ROOT/scripts/verify-t1-pagination-fixture.sh")"
  printf '%s\n' "$T1_FIXTURE_OUTPUT"
  T1_FIXTURE_PATH="$(awk -F= '$1 == "t1_fixture_path" { print substr($0, index($0, "=") + 1) }' <<<"$T1_FIXTURE_OUTPUT")"
  T1_FIXTURE_BASE_OID="$(awk -F= '$1 == "t1_base_commit" { print $2 }' <<<"$T1_FIXTURE_OUTPUT")"
  T1_FIX_OID="$(awk -F= '$1 == "t1_fix_commit" { print $2 }' <<<"$T1_FIXTURE_OUTPUT")"
  [[ "$T1_FIXTURE_PATH" == /tmp/musehub-t1-pagination.* && -n "$T1_FIXTURE_BASE_OID" && -n "$T1_FIX_OID" ]]
  git -C "$T1_FIXTURE_PATH" remote add musehub "$GIT_URL"
  git -C "$T1_FIXTURE_PATH" push --mirror musehub
  git -C "$GIT_DIR" fetch origin +refs/heads/main:refs/remotes/origin/main
  T1_BASELINE_OID="$(git -C "$GIT_DIR" rev-parse refs/remotes/origin/main)"
  [[ "$T1_BASELINE_OID" == "$T1_FIXTURE_BASE_OID" ]] || {
    printf 'T1 imported baseline OID mismatch (expected=%s actual=%s)\n' \
      "$T1_FIXTURE_BASE_OID" "$T1_BASELINE_OID" >&2
    exit 1
  }

  INTERNAL_REF="refs/musehub/acceptance-check"
  dc exec -T musehub-server git --git-dir="$BARE_REPO" update-ref "$INTERNAL_REF" "$T1_FIX_OID"
  ADVERTISED_REFS="$(git ls-remote "$GIT_URL")"
  if grep -q 'refs/musehub/' <<<"$ADVERTISED_REFS"; then
    echo "Git SSH advertised a platform-internal ref" >&2
    exit 1
  fi
  git -C "$T1_FIXTURE_PATH" update-ref "$INTERNAL_REF" "$T1_FIXTURE_BASE_OID"
  if git -C "$T1_FIXTURE_PATH" push musehub "$INTERNAL_REF:$INTERNAL_REF"; then
    echo "Git SSH accepted a client write to a platform-internal ref" >&2
    exit 1
  fi
  SERVER_INTERNAL_OID="$(dc exec -T musehub-server git --git-dir="$BARE_REPO" rev-parse "$INTERNAL_REF" | tr -d '\\r\\n')"
  [[ "$SERVER_INTERNAL_OID" == "$T1_FIX_OID" ]] || {
    printf 'platform-internal ref changed (expected=%s actual=%s)\\n' \\
      "$T1_FIX_OID" "$SERVER_INTERNAL_OID" >&2
    exit 1
  }
  git -C "$T1_FIXTURE_PATH" update-ref -d "$INTERNAL_REF"

  git -C "$GIT_DIR" switch --force-create main refs/remotes/origin/main
  printf 'must not reach the protected default branch\n' >"$GIT_DIR/protected-default-branch.txt"
  git -C "$GIT_DIR" add protected-default-branch.txt
  git -C "$GIT_DIR" commit -m "Attempt protected default branch write" --no-gpg-sign
  if git -C "$GIT_DIR" push origin HEAD:refs/heads/main; then
    echo "Git SSH accepted a post-import default-branch update" >&2
    exit 1
  fi
  SERVER_MAIN_OID="$(dc exec -T musehub-server git --git-dir="$BARE_REPO" rev-parse refs/heads/main | tr -d '\\r\\n')"
  [[ "$SERVER_MAIN_OID" == "$T1_FIXTURE_BASE_OID" ]] || {
    printf 'protected default branch changed (expected=%s actual=%s)\n' \
      "$T1_FIXTURE_BASE_OID" "$SERVER_MAIN_OID" >&2
    exit 1
  }
  git -C "$GIT_DIR" switch --force-create main refs/remotes/origin/main
  git -C "$GIT_DIR" switch -c t1/pagination-baseline
  mkdir -p "$GIT_DIR/.musehub/workflows"
  cat >"$GIT_DIR/.musehub/workflows/ci.yaml" <<EOF
version: 1
on: [pull_request, manual]
jobs:
  - id: t1_pagination
    image: $T1_NODE_DIGEST
    command: ["npm", "test"]
  - id: cache_seed
    image: $ALPINE_DIGEST
    command: ["sh", "-c", "mkdir -p .musehub-cache output && printf cache-payload-v1 > .musehub-cache/value && printf artifact-seed-v1 > output/seed-proof.txt"]
    artifacts:
      - name: seed-proof.txt
        path: output/seed-proof.txt
    cache:
      key: cache-key-v1
      lockfile: .musehub/workflows/ci.yaml
      paths: [.musehub-cache]
  - id: cache_verify
    needs: [cache_seed]
    image: $ALPINE_DIGEST
    command: ["sh", "-c", "mkdir -p output; if [ -f .musehub-cache/value ]; then cp .musehub-cache/value output/cache-proof.txt; else printf cache-missing > output/cache-proof.txt; fi; test \"\$(cat .musehub-cache/value)\" = cache-payload-v1"]
    artifacts:
      - name: cache-proof.txt
        path: output/cache-proof.txt
    cache:
      key: cache-key-v1
      lockfile: .musehub/workflows/ci.yaml
      paths: [.musehub-cache]
EOF
  git -C "$GIT_DIR" add .musehub/workflows/ci.yaml
  git -C "$GIT_DIR" commit -m "Add T1 pagination workflow" --no-gpg-sign
  T1_BASELINE_OID="$(git -C "$GIT_DIR" rev-parse HEAD)"
  git -C "$GIT_DIR" push origin HEAD:refs/heads/t1/pagination-baseline
  git -C "$GIT_DIR" fetch origin +refs/heads/t1/pagination-fix:refs/remotes/origin/t1/pagination-fix
  T1_REMOTE_FIX_OID="$(git -C "$GIT_DIR" rev-parse refs/remotes/origin/t1/pagination-fix)"
  [[ "$T1_REMOTE_FIX_OID" == "$T1_FIX_OID" ]] || {
    printf 'T1 imported fix OID mismatch (expected=%s actual=%s)\n' \
      "$T1_FIX_OID" "$T1_REMOTE_FIX_OID" >&2
    exit 1
  }
  git -C "$GIT_DIR" switch -c t1/pagination-fix refs/remotes/origin/t1/pagination-fix
  git -C "$GIT_DIR" checkout t1/pagination-baseline -- .musehub/workflows/ci.yaml
  git -C "$GIT_DIR" add .musehub/workflows/ci.yaml
  git -C "$GIT_DIR" commit -m "Add T1 pagination workflow to fix branch" --no-gpg-sign
  SOURCE_OID="$(git -C "$GIT_DIR" rev-parse HEAD)"
  SOURCE_REF="refs/heads/t1/pagination-fix"
  git -C "$GIT_DIR" push origin "HEAD:$SOURCE_REF"
  PR_JSON="$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg issue "$T1_ISSUE_ID" '{title:"Fix 1-based pagination boundary",source_branch:"t1/pagination-fix",target_branch:"main",issue_id:$issue}')" \
    "$BASE/api/v1/orgs/demo/repos/$REPO_SLUG/pull-requests")"
  T1_PR_ID="$(jq -r .id <<<"$PR_JSON")"
  [[ "$T1_PR_ID" != null && -n "$T1_PR_ID" && "$(jq -r .head_oid <<<"$PR_JSON")" == "$SOURCE_OID" ]]
  BASELINE_RUN_JSON="$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg oid "$T1_BASELINE_OID" '{source_oid:$oid}')" \
    "$BASE/api/v1/orgs/demo/repos/$REPO_SLUG/workflow-runs")"
  T1_BASELINE_RUN_ID="$(jq -r .id <<<"$BASELINE_RUN_JSON")"
  [[ "$T1_BASELINE_RUN_ID" != null && -n "$T1_BASELINE_RUN_ID" ]]
else
  mkdir -p "$GIT_DIR/.musehub/workflows"
  cat >"$GIT_DIR/.musehub/workflows/ci.yaml" <<EOF
version: 1
on: [push]
jobs:
  - id: cache_seed
    image: $ALPINE_DIGEST
    command: ["sh", "-c", "mkdir -p .musehub-cache output && printf cache-payload-v1 > .musehub-cache/value && printf artifact-seed-v1 > output/seed-proof.txt"]
    artifacts:
      - name: seed-proof.txt
        path: output/seed-proof.txt
    cache:
      key: cache-key-v1
      lockfile: .musehub/workflows/ci.yaml
      paths: [.musehub-cache]
  - id: cache_verify
    needs: [cache_seed]
    image: $ALPINE_DIGEST
    command: ["sh", "-c", "mkdir -p output; if [ -f .musehub-cache/value ]; then cp .musehub-cache/value output/cache-proof.txt; else printf cache-missing > output/cache-proof.txt; fi; test \"\$(cat .musehub-cache/value)\" = cache-payload-v1"]
    artifacts:
      - name: cache-proof.txt
        path: output/cache-proof.txt
    cache:
      key: cache-key-v1
      lockfile: .musehub/workflows/ci.yaml
      paths: [.musehub-cache]
EOF
  git -C "$GIT_DIR" add .musehub/workflows/ci.yaml
  git -C "$GIT_DIR" commit -m "Add SSH triggered workflow" --no-gpg-sign
  SOURCE_OID="$(git -C "$GIT_DIR" rev-parse HEAD)"
  SOURCE_REF="refs/heads/acceptance/ssh-runner"
  git -C "$GIT_DIR" push origin "HEAD:$SOURCE_REF"
fi

PUSHED_OID="$(dc exec -T musehub-server git --git-dir="$BARE_REPO" rev-parse "$SOURCE_REF" | tr -d '\r\n')"
[[ "$PUSHED_OID" == "$SOURCE_OID" ]]

read_db() {
  dc exec -T -e PGPASSWORD="$POSTGRES_PASSWORD" postgres psql -U musehub -d musehub -Atc "$1" | tr -d '\r\n'
}

OUTBOX_ID=""
OUTBOX_STATE="not-applicable"
RUN_ID=""
if [[ -n "$T1_NODE_SOURCE_IMAGE" ]]; then
  for _ in $(seq 1 90); do
    RUN_ID="$(read_db "SELECT id FROM pipeline_runs WHERE repo_id='$REPO_ID' AND pr_id='$T1_PR_ID' AND source_oid='$SOURCE_OID' AND run_kind='pr_validation' ORDER BY created_at DESC LIMIT 1")"
    [[ -n "$RUN_ID" ]] && break
    sleep 1
  done
  [[ -n "$RUN_ID" ]] || { echo "PR creation did not produce a product validation run" >&2; exit 1; }
  printf 't1_issue_id=%s\nt1_pr_id=%s\nt1_common_base_oid=%s\nt1_baseline_oid=%s\nt1_source_oid=%s\nt1_baseline_run_id=%s\nt1_pr_run_id=%s\n' \
    "$T1_ISSUE_ID" "$T1_PR_ID" "$T1_FIXTURE_BASE_OID" "$T1_BASELINE_OID" "$SOURCE_OID" "$T1_BASELINE_RUN_ID" "$RUN_ID"
else
  for _ in $(seq 1 90); do
    OUTBOX_ID="$(read_db "SELECT id FROM git_ref_trigger_outbox WHERE repo_id='$REPO_ID' AND git_ref='$SOURCE_REF' AND source_oid='$SOURCE_OID' ORDER BY created_at DESC LIMIT 1")"
    if [[ -n "$OUTBOX_ID" ]]; then
      OUTBOX_STATE="$(read_db "SELECT state FROM git_ref_trigger_outbox WHERE id='$OUTBOX_ID'")"
      RUN_ID="$(read_db "SELECT id FROM pipeline_runs WHERE repo_id='$REPO_ID' AND source_oid='$SOURCE_OID' AND run_kind='workflow' ORDER BY created_at DESC LIMIT 1")"
      [[ "$OUTBOX_STATE" == completed && -n "$RUN_ID" ]] && break
    fi
    sleep 1
  done
  [[ "${OUTBOX_STATE:-}" == completed && -n "$RUN_ID" ]] || {
    echo "outbox did not produce a workflow run (outbox=${OUTBOX_STATE:-missing}, run=${RUN_ID:-missing})" >&2
    exit 1
  }
  printf 'ssh_push_oid=%s\noutbox_id=%s\noutbox_state=%s\nworkflow_run_id=%s\n' \
    "$SOURCE_OID" "$OUTBOX_ID" "$OUTBOX_STATE" "$RUN_ID"
fi

dc up -d docker-job-daemon
for _ in $(seq 1 90); do
  if dc exec -T docker-job-daemon docker info >/dev/null 2>&1; then break; fi
  sleep 1
done
dc exec -T docker-job-daemon docker info >/dev/null
dc exec -T docker-job-daemon docker pull "$ALPINE_DIGEST" >&2
if [[ -n "$T1_NODE_SOURCE_IMAGE" ]]; then
  dc exec -T docker-job-daemon docker pull "$T1_NODE_DIGEST" >&2
fi

echo "Verifying Runner-to-DinD Docker API connectivity"
dc run --rm --no-deps --entrypoint /usr/bin/timeout musehub-runner \
  10 /usr/local/bin/docker info >/dev/null

dc up -d musehub-runner
RUNNER_CONTAINER_ID="$(dc ps -q musehub-runner)"
RUNNER_STATE="$(docker inspect --format '{{.State.Status}}' "$RUNNER_CONTAINER_ID" 2>/dev/null || true)"
if [[ "$RUNNER_STATE" != running ]]; then
  echo "Runner container did not stay running (state=${RUNNER_STATE:-missing})" >&2
  dc ps -a >&2 || true
  dc logs --no-color --timestamps --tail=200 musehub-runner docker-job-daemon >&2 || true
  exit 1
fi

echo "Runner container is running; waiting for the remote workflow job"

report_workflow_failure() {
  local reason="$1"
  echo "remote workflow failed: $reason run=$RUN_STATE job=$JOB_STATE attempt=$ATTEMPT_STATE" >&2
  echo "workflow job, attempt, and persisted log diagnostics:" >&2
  dc exec -T -e PGPASSWORD="$POSTGRES_PASSWORD" postgres psql -U musehub -d musehub -P pager=off -c \
    "SELECT j.job_key, j.state AS job_state, j.error_code AS job_error,
            a.attempt_number, a.state AS attempt_state, a.error_code AS attempt_error,
            l.chunk_index, left(l.text, 4000) AS log_text
     FROM pipeline_workflow_jobs j
     LEFT JOIN pipeline_workflow_job_attempts a ON a.workflow_job_id = j.id
     LEFT JOIN pipeline_workflow_job_logs l
       ON l.workflow_job_id = j.id AND l.attempt_id = a.id
     WHERE j.pipeline_run_id = '$RUN_ID'
     ORDER BY a.attempt_number DESC NULLS LAST, l.chunk_index
     LIMIT 20" >&2 || true
  echo "Parent pipeline executor result and logs:" >&2
  dc exec -T -e PGPASSWORD="$POSTGRES_PASSWORD" postgres psql -U musehub -d musehub -P pager=off -c \
    "SELECT r.state AS run_state, r.error_code AS run_error,
            j.job_key, j.state AS executor_job_state,
            l.chunk_index, left(l.text, 4000) AS executor_log
     FROM pipeline_runs r
     LEFT JOIN pipeline_jobs j ON j.pipeline_run_id = r.id
     LEFT JOIN pipeline_job_logs l ON l.job_id = j.id
     WHERE r.id = '$RUN_ID'
     ORDER BY l.chunk_index
     LIMIT 20" >&2 || true
  echo "Compose services and Runner/daemon logs:" >&2
  dc ps -a >&2 || true
  dc logs --no-color --timestamps --tail=200 musehub-runner docker-job-daemon >&2 || true
}

unexpected_terminal_state() {
  local state entry
  local -a entries
  state="$1"
  case "$state" in
    ""|queued|running|succeeded) return 1 ;;
    *=*)
      IFS=',' read -r -a entries <<<"$state"
      for entry in "${entries[@]}"; do
        case "${entry#*=}" in
          queued|running|succeeded) ;;
          *) return 0 ;;
        esac
      done
      return 1
      ;;
    *) return 0 ;;
  esac
}

echo "Waiting for workflow run and remote job attempt to succeed"
EXPECTED_JOB_COUNT=2
[[ -n "$T1_NODE_SOURCE_IMAGE" ]] && EXPECTED_JOB_COUNT=3
RUN_STATE=""
JOB_STATE=""
ATTEMPT_STATE=""
for _ in $(seq 1 300); do
  RUN_STATE="$(read_db "SELECT state FROM pipeline_runs WHERE id='$RUN_ID'")"
  JOB_STATE="$(read_db "SELECT CASE WHEN COUNT(*) = $EXPECTED_JOB_COUNT AND bool_and(state = 'succeeded') THEN 'succeeded' ELSE string_agg(job_key || '=' || state, ',') END FROM pipeline_workflow_jobs WHERE pipeline_run_id='$RUN_ID'")"
  ATTEMPT_STATE="$(read_db "SELECT CASE WHEN COUNT(*) = $EXPECTED_JOB_COUNT AND bool_and(a.state = 'succeeded') THEN 'succeeded' ELSE string_agg(j.job_key || '=' || a.state, ',') END FROM pipeline_workflow_job_attempts a JOIN pipeline_workflow_jobs j ON j.id=a.workflow_job_id WHERE j.pipeline_run_id='$RUN_ID'")"
  [[ "$RUN_STATE" == succeeded && "$JOB_STATE" == succeeded && "$ATTEMPT_STATE" == succeeded ]] && break
  if unexpected_terminal_state "$RUN_STATE" || unexpected_terminal_state "$JOB_STATE" || unexpected_terminal_state "$ATTEMPT_STATE"; then
    report_workflow_failure "unexpected terminal state"
    exit 1
  fi
  sleep 1
done
if [[ "$RUN_STATE" != succeeded || "$JOB_STATE" != succeeded || "$ATTEMPT_STATE" != succeeded ]]; then
  report_workflow_failure "timed out waiting for success"
  exit 1
fi
if [[ -n "$OUTBOX_ID" ]]; then
  [[ "$(read_db "SELECT state FROM git_ref_trigger_outbox WHERE id='$OUTBOX_ID'")" == completed ]]
fi

if [[ -n "$T1_NODE_SOURCE_IMAGE" ]]; then
  BASELINE_STATE=""
  BASELINE_JOBS=""
  for _ in $(seq 1 300); do
    BASELINE_STATE="$(read_db "SELECT state FROM pipeline_runs WHERE id='$T1_BASELINE_RUN_ID'")"
    BASELINE_JOBS="$(read_db "SELECT CASE WHEN COUNT(*) = 3 AND bool_and(state IN ('succeeded','failed')) THEN string_agg(job_key || '=' || state, ',') ELSE string_agg(job_key || '=' || state, ',') END FROM pipeline_workflow_jobs WHERE pipeline_run_id='$T1_BASELINE_RUN_ID'")"
    [[ "$BASELINE_STATE" == failed && "$BASELINE_JOBS" == *"t1_pagination=failed"* ]] && break
    [[ "$BASELINE_STATE" == succeeded ]] && { echo "T1 buggy baseline unexpectedly passed on the product Runner" >&2; exit 1; }
    sleep 1
  done
  [[ "$BASELINE_STATE" == failed && "$BASELINE_JOBS" == *"t1_pagination=failed"* ]] || {
    printf 'T1 product baseline did not fail as expected (run=%s jobs=%s)\n' "$BASELINE_STATE" "$BASELINE_JOBS" >&2
    exit 1
  }
  T1_BASELINE_TEST_LOG="$(read_db "SELECT COALESCE(string_agg(l.text, '' ORDER BY l.chunk_index), '') FROM pipeline_workflow_job_logs l JOIN pipeline_workflow_jobs j ON j.id=l.workflow_job_id WHERE j.pipeline_run_id='$T1_BASELINE_RUN_ID' AND j.job_key='t1_pagination'")"
  [[ "$T1_BASELINE_TEST_LOG" == *"fail 3"* ]] || {
    echo "T1 product baseline log did not prove three failing tests" >&2
    exit 1
  }
  T1_TEST_LOG="$(read_db "SELECT COALESCE(string_agg(l.text, '' ORDER BY l.chunk_index), '') FROM pipeline_workflow_job_logs l JOIN pipeline_workflow_jobs j ON j.id=l.workflow_job_id WHERE j.pipeline_run_id='$RUN_ID' AND j.job_key='t1_pagination'")"
  [[ "$T1_TEST_LOG" == *"pass 3"* && "$T1_TEST_LOG" == *"fail 0"* ]] || {
    echo "T1 product Runner log did not prove three passing tests" >&2
    exit 1
  }
  T1_PR_CHECK_STATE="$(read_db "SELECT state FROM pr_check_runs WHERE pr_id='$T1_PR_ID' AND head_oid='$SOURCE_OID' AND check_name='product-ci'")"
  [[ "$T1_PR_CHECK_STATE" == success ]] || {
    printf 'T1 pull request check did not succeed (state=%s)\n' "$T1_PR_CHECK_STATE" >&2
    exit 1
  }
  printf 't1_baseline_trigger=manual\nt1_baseline_run_state=%s\nt1_baseline_job_states=%s\nt1_pr_trigger=pull_request\nt1_pr_check_state=%s\n' \
    "$BASELINE_STATE" "$BASELINE_JOBS" "$T1_PR_CHECK_STATE"
  printf 't1_runner_tests=3 passed, 0 failed on PR head %s\n' "$SOURCE_OID"
fi

for artifact_name in seed-proof.txt cache-proof.txt; do
  artifact_record="$(read_db "SELECT id::text || '|' || sha256 FROM artifacts WHERE pipeline_run_id='$RUN_ID' AND name='$artifact_name' AND state='ready'")"
  [[ -n "$artifact_record" ]] || { printf 'missing ready workflow artifact: %s\n' "$artifact_name" >&2; exit 1; }
  IFS='|' read -r artifact_id expected_sha256 <<<"$artifact_record"
  artifact_path="$KEY_DIR/$artifact_name"
  curl -fsS -b "$COOKIE_JAR" \
    "$BASE/api/v1/orgs/demo/repos/$REPO_SLUG/pipeline-runs/$RUN_ID/artifacts/$artifact_id" \
    -o "$artifact_path"
  actual_sha256="$(openssl dgst -sha256 "$artifact_path" | awk '{print $NF}')"
  [[ "$actual_sha256" == "$expected_sha256" ]] || {
    printf 'workflow artifact digest mismatch: %s\n' "$artifact_name" >&2
    exit 1
  }
done
[[ "$(<"$KEY_DIR/seed-proof.txt")" == artifact-seed-v1 ]]
[[ "$(<"$KEY_DIR/cache-proof.txt")" == cache-payload-v1 ]]
printf 'workflow_artifacts=2 downloaded and SHA-256 verified\n'
printf 'workflow_cache=hit proved by dependent job\n'

echo "workflow_run_state=$RUN_STATE"
echo "workflow_job_state=$JOB_STATE"
echo "workflow_attempt_state=$ATTEMPT_STATE"

echo "Verifying the full migration down/up round trip on a fresh isolated database"
ROUNDTRIP_DATABASE="musehub_migration_roundtrip"
dc exec -T -e PGPASSWORD="$POSTGRES_PASSWORD" postgres psql -U musehub -d postgres -v ON_ERROR_STOP=1 \
  -c "CREATE DATABASE $ROUNDTRIP_DATABASE" >/dev/null
ROUNDTRIP_DATABASE_URL="postgres://musehub:${POSTGRES_PASSWORD}@postgres:5432/${ROUNDTRIP_DATABASE}?sslmode=disable"
dc exec -T -e DATABASE_URL="$ROUNDTRIP_DATABASE_URL" musehub-server sh -c \
  'musehub-migrate up /migrations "$DATABASE_URL" && musehub-migrate down /migrations "$DATABASE_URL" && musehub-migrate up /migrations "$DATABASE_URL"'
echo "migration_round_trip=ok"
echo "ssh_outbox_runner_e2e=ok"
