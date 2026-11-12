#!/usr/bin/env bash
# Product Compose git-ssh: API + musehub-git-ssh with default volume permissions.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
# shellcheck source=lib/git-http-auth-sandbox.sh
source "$ROOT/scripts/lib/git-http-auth-sandbox.sh"

command -v docker >/dev/null || { echo "docker required" >&2; exit 1; }
command -v jq >/dev/null || { echo "jq required" >&2; exit 1; }
command -v ssh >/dev/null || { echo "ssh required" >&2; exit 1; }
command -v git >/dev/null || { echo "git required" >&2; exit 1; }

: "${COMPOSE_PROJECT_NAME:?}"
: "${BASE:?}"
: "${BOOTSTRAP_PW:?}"
: "${MUSEHUB_GATEWAY_HTTP_PORT:?}"
: "${MUSEHUB_GIT_SSH_PORT:=2222}"

FEATURE_BRANCH="feature/compose-roundtrip"
STORAGE_KEY=""
TOKEN_ID=""
COOKIE_JAR=""
KEY_DIR=""
GIT_DIR=""
HTTP_DIR=""
BAD_DIR=""

if [[ -n "${MUSEHUB_STACK_DC:-}" ]]; then
  dc() { eval "$MUSEHUB_STACK_DC $(printf '%q ' "$@")"; }
else
  dc() { docker compose -p "$COMPOSE_PROJECT_NAME" "$@"; }
fi

revoke_compose_pat() {
  if [[ -z "${TOKEN_ID:-}" || -z "${COOKIE_JAR:-}" ]]; then
    return 0
  fi
  if ! curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -X DELETE \
    "$BASE/api/v1/tokens/${TOKEN_ID}" >/dev/null; then
    echo "compose git-ssh: token revoke failed (credentials sandbox destroyed)" >&2
    return 1
  fi
  TOKEN_ID=""
  return 0
}

cleanup() {
  revoke_compose_pat || true
  git_http_auth_sandbox_end
  [[ -n "${KEY_DIR:-}" ]] && rm -rf "$KEY_DIR"
  [[ -n "${GIT_DIR:-}" ]] && rm -rf "$GIT_DIR" "${GIT_DIR}.bad" 2>/dev/null || true
  [[ -n "${HTTP_DIR:-}" ]] && rm -rf "$HTTP_DIR"
  [[ -n "${BAD_DIR:-}" ]] && rm -rf "$BAD_DIR"
  [[ -n "${COOKIE_JAR:-}" ]] && rm -f "$COOKIE_JAR"
}
trap cleanup EXIT

remote_ref_oid() {
  local ref="$1"
  dc exec -T musehub-server git --git-dir="/var/lib/musehub/git/${STORAGE_KEY}.git" rev-parse "$ref"
}

echo "=== compose git-ssh: wait published port ${MUSEHUB_GIT_SSH_PORT} ==="
wait_published_port() {
  if command -v ss >/dev/null 2>&1; then
    ss -tln 2>/dev/null | grep -qE ":${MUSEHUB_GIT_SSH_PORT}([[:space:]]|$)"
    return
  fi
  (echo >/dev/tcp/127.0.0.1/"${MUSEHUB_GIT_SSH_PORT}") 2>/dev/null
}
for _ in $(seq 1 120); do
  if wait_published_port; then
    break
  fi
  sleep 2
done
wait_published_port || {
  dc logs musehub-git-ssh --tail=80 >&2 || true
  exit 1
}

echo "=== compose git-ssh: run dir ACL ==="
SSH_RUN_DIR=/var/lib/musehub/git-ssh
dc exec -T musehub-git-ssh test -f "${SSH_RUN_DIR}/runtime.env"
run_dir_mode="$(dc exec -T musehub-git-ssh stat -c %a "${SSH_RUN_DIR}")"
run_dir_own="$(dc exec -T musehub-git-ssh stat -c %U:%G "${SSH_RUN_DIR}")"
run_dir_mode="${run_dir_mode//$'\r'/}"
run_dir_own="${run_dir_own//$'\r'/}"
[[ "$run_dir_mode" =~ ^(2750|0750)$ && "$run_dir_own" == root:git ]] || {
  echo "compose git-ssh: bad run dir ACL (want 2750|0750 root:git, got ${run_dir_mode} ${run_dir_own})" >&2
  exit 1
}
env_mode="$(dc exec -T musehub-git-ssh stat -c %a "${SSH_RUN_DIR}/runtime.env")"
env_own="$(dc exec -T musehub-git-ssh stat -c %U:%G "${SSH_RUN_DIR}/runtime.env")"
env_mode="${env_mode//$'\r'/}"
env_own="${env_own//$'\r'/}"
[[ "$env_mode" == 640 && "$env_own" == root:git ]] || {
  echo "compose git-ssh: bad runtime.env ACL (want 640 root:git, got ${env_mode} ${env_own})" >&2
  exit 1
}
dc exec -T -u git musehub-git-ssh test -x "${SSH_RUN_DIR}"
dc exec -T -u git musehub-git-ssh test -r "${SSH_RUN_DIR}/runtime.env"
if dc exec -T -u git musehub-git-ssh test -w "${SSH_RUN_DIR}" 2>/dev/null; then
  echo "compose git-ssh: git user must not have write on ${SSH_RUN_DIR}" >&2
  exit 1
fi
if dc exec -T -u git musehub-git-ssh sh -c "echo x >> '${SSH_RUN_DIR}/runtime.env'" 2>/dev/null; then
  echo "compose git-ssh: git user must not append runtime.env" >&2
  exit 1
fi

COOKIE_JAR=$(mktemp)
curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg h admin --arg p "$BOOTSTRAP_PW" '{handle:$h,password:$p}')" \
  "$BASE/api/v1/auth/login" | jq -e '.ok == true' >/dev/null

KEY_DIR=$(mktemp -d)
chmod 0700 "$KEY_DIR"
ssh-keygen -t ed25519 -f "$KEY_DIR/id_ed25519" -N "" -q
PUB=$(cat "$KEY_DIR/id_ed25519.pub")
curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg k "$PUB" '{name:"compose-ssh",public_key:$k}')" \
  "$BASE/api/v1/ssh-keys" | jq -e '.id' >/dev/null

REPO_SLUG="compose-ssh-$(date +%s)"
REPO_JSON=$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg s "$REPO_SLUG" '{slug:$s}')" \
  "$BASE/api/v1/orgs/demo/repos")
STORAGE_KEY=$(echo "$REPO_JSON" | jq -r .storage_key)
REPO_ID=$(echo "$REPO_JSON" | jq -r .id)
[[ -n "$STORAGE_KEY" && "$STORAGE_KEY" != null ]]
[[ -n "$REPO_ID" && "$REPO_ID" != null ]]

PAT_JSON=$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg id "$REPO_ID" '{name:"compose-git",org:"demo",scope_mode:"selected",scopes:["repo:read","repo:write"],repo_ids:[$id]}')" \
  "$BASE/api/v1/tokens")
PAT=$(echo "$PAT_JSON" | jq -r .token)
TOKEN_ID=$(echo "$PAT_JSON" | jq -r .id)
[[ ${#PAT} -gt 10 ]]

git_http_auth_sandbox_begin "$PAT"

SSH_URL="ssh://git@127.0.0.1:${MUSEHUB_GIT_SSH_PORT}/demo/${REPO_SLUG}.git"
HTTP_URL="$(git_http_credential_free_url "http://127.0.0.1:${MUSEHUB_GATEWAY_HTTP_PORT}" demo "$REPO_SLUG")"

export GIT_SSH_COMMAND="ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -i $KEY_DIR/id_ed25519 -p ${MUSEHUB_GIT_SSH_PORT}"

GIT_DIR=$(mktemp -d)
git_http_run "" clone "$SSH_URL" "$GIT_DIR"
git -C "$GIT_DIR" config user.email ci@musehub.test
git -C "$GIT_DIR" config user.name ci

MAIN_BEFORE=$(remote_ref_oid refs/heads/main)

echo "=== compose git-ssh: protected main (direct / force / delete) ==="
git -C "$GIT_DIR" checkout -B "$FEATURE_BRANCH"
echo probe >"$GIT_DIR/probe.txt"
git -C "$GIT_DIR" add probe.txt
git -C "$GIT_DIR" commit -m probe-main-denial-test --no-gpg-sign

if git -C "$GIT_DIR" push origin "HEAD:refs/heads/main" 2>/tmp/git-ssh-main-push.err; then
  echo "direct main push must be rejected" >&2
  exit 1
fi
grep -q "protected branch" "$(redact_git_output "$(cat /tmp/git-ssh-main-push.err)")" || true
rm -f /tmp/git-ssh-main-push.err
[[ "$(remote_ref_oid refs/heads/main)" == "$MAIN_BEFORE" ]]

if git -C "$GIT_DIR" push --force origin "HEAD:refs/heads/main" 2>/tmp/git-ssh-force.err; then
  echo "force main push must be rejected" >&2
  exit 1
fi
rm -f /tmp/git-ssh-force.err
[[ "$(remote_ref_oid refs/heads/main)" == "$MAIN_BEFORE" ]]

if git -C "$GIT_DIR" push origin ":refs/heads/main" 2>/tmp/git-ssh-del.err; then
  echo "delete main must be rejected" >&2
  exit 1
fi
rm -f /tmp/git-ssh-del.err
[[ "$(remote_ref_oid refs/heads/main)" == "$MAIN_BEFORE" ]]

echo "=== compose git-ssh: SSH / HTTP alternation on ${FEATURE_BRANCH} ==="
git -C "$GIT_DIR" checkout -B "$FEATURE_BRANCH"
echo "ssh-1" >"$GIT_DIR/roundtrip.txt"
git -C "$GIT_DIR" add roundtrip.txt
git -C "$GIT_DIR" commit -m ssh-1 --no-gpg-sign
git_http_run "$GIT_DIR" push -u origin "HEAD:refs/heads/${FEATURE_BRANCH}"
OID_SSH_1=$(git -C "$GIT_DIR" rev-parse HEAD)
[[ "$(remote_ref_oid "refs/heads/${FEATURE_BRANCH}")" == "$OID_SSH_1" ]]

HTTP_DIR=$(mktemp -d)
git_http_run "" clone "$HTTP_URL" "$HTTP_DIR"
git -C "$HTTP_DIR" config user.email ci@musehub.test
git -C "$HTTP_DIR" config user.name ci
git -C "$HTTP_DIR" checkout "$FEATURE_BRANCH"
echo "http-2" >>"$HTTP_DIR/roundtrip.txt"
git -C "$HTTP_DIR" add roundtrip.txt
git -C "$HTTP_DIR" commit -m http-2 --no-gpg-sign
git_http_run "$HTTP_DIR" push origin "HEAD:refs/heads/${FEATURE_BRANCH}"
OID_HTTP_2=$(git -C "$HTTP_DIR" rev-parse HEAD)
[[ "$(remote_ref_oid "refs/heads/${FEATURE_BRANCH}")" == "$OID_HTTP_2" ]]
[[ "$OID_HTTP_2" != "$OID_SSH_1" ]]

git_http_run "$GIT_DIR" fetch origin "$FEATURE_BRANCH"
git_http_run "$GIT_DIR" checkout "$FEATURE_BRANCH"
git_http_run "$GIT_DIR" merge --ff-only "origin/${FEATURE_BRANCH}"
echo "ssh-3" >>"$GIT_DIR/roundtrip.txt"
git -C "$GIT_DIR" add roundtrip.txt
git -C "$GIT_DIR" commit -m ssh-3 --no-gpg-sign
git_http_run "$GIT_DIR" push origin "HEAD:refs/heads/${FEATURE_BRANCH}"
OID_SSH_3=$(git -C "$GIT_DIR" rev-parse HEAD)
[[ "$(remote_ref_oid "refs/heads/${FEATURE_BRANCH}")" == "$OID_SSH_3" ]]
[[ "$OID_SSH_3" != "$OID_HTTP_2" ]]

dc exec -T musehub-git-ssh sh -c '
  ! grep -q "DATABASE_URL=" /usr/local/lib/musehub/musehub-git-login.sh
  ! grep -q "postgres://" /var/lib/musehub/git-ssh/akc-wrap.sh
'

if ssh -o ExitOnForwardFailure=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
  -i "$KEY_DIR/id_ed25519" -p "${MUSEHUB_GIT_SSH_PORT}" -N -L "19999:127.0.0.1:22" git@127.0.0.1 2>/dev/null; then
  echo "port forward should have been rejected" >&2
  exit 1
fi

BAD_DIR=$(mktemp -d)
ssh-keygen -t ed25519 -f "$BAD_DIR/bad" -N "" -q
export GIT_SSH_COMMAND="ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -i $BAD_DIR/bad -p ${MUSEHUB_GIT_SSH_PORT}"
if git clone "$SSH_URL" "${GIT_DIR}.bad" 2>/dev/null; then
  echo "unregistered key must not clone" >&2
  exit 1
fi

echo "product_compose_git_ssh_e2e=ok"
