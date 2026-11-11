#!/usr/bin/env bash
# Product Compose git-ssh: API + musehub-git-ssh with default volume permissions.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

command -v docker >/dev/null || { echo "docker required" >&2; exit 1; }
command -v jq >/dev/null || { echo "jq required" >&2; exit 1; }
command -v ssh >/dev/null || { echo "ssh required" >&2; exit 1; }
command -v git >/dev/null || { echo "git required" >&2; exit 1; }

: "${COMPOSE_PROJECT_NAME:?}"
: "${BASE:?}"
: "${BOOTSTRAP_PW:?}"
: "${MUSEHUB_GIT_SSH_PORT:=2222}"

if [[ -n "${MUSEHUB_STACK_DC:-}" ]]; then
  # shellcheck disable=SC2128
  dc() { eval "$MUSEHUB_STACK_DC" "$@"; }
else
  dc() { docker compose -p "$COMPOSE_PROJECT_NAME" "$@"; }
fi

echo "=== compose git-ssh: wait listen 2222 ==="
for _ in $(seq 1 120); do
  if dc exec -T musehub-git-ssh sh -c 'ss -tln | grep -q ":2222 "' 2>/dev/null; then
    break
  fi
  sleep 2
done
dc exec -T musehub-git-ssh sh -c 'ss -tln | grep -q ":2222 "' || {
  dc logs musehub-git-ssh --tail=80 >&2 || true
  exit 1
}

echo "=== compose git-ssh: run dir ACL ==="
dc exec -T musehub-git-ssh sh -c '
  d=/var/lib/musehub/git-ssh
  test -f "$d/runtime.env"
  stat -c "%a %U:%G" "$d" | grep -qE "^(2750|0750) root:git$"
  stat -c "%a %U:%G" "$d/runtime.env" | grep -qE "^640 root:git$"
  sudo -u git test -x "$d"
  sudo -u git test -r "$d/runtime.env"
  ! sudo -u git test -w "$d"
  ! sudo -u git sh -c "echo x >> \"$d/runtime.env\"" 2>/dev/null
'

COOKIE_JAR=$(mktemp)
cleanup() { rm -f "$COOKIE_JAR"; }
trap cleanup EXIT

curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg h admin --arg p "$BOOTSTRAP_PW" '{handle:$h,password:$p}')" \
  "$BASE/api/v1/auth/login" | jq -e '.ok == true' >/dev/null

KEY_DIR=$(mktemp -d)
trap 'rm -rf "$KEY_DIR" "$COOKIE_JAR"' EXIT
ssh-keygen -t ed25519 -f "$KEY_DIR/id_ed25519" -N "" -q
PUB=$(cat "$KEY_DIR/id_ed25519.pub")
curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg k "$PUB" '{name:"compose-ssh",public_key:$k}')" \
  "$BASE/api/v1/ssh-keys" | jq -e '.id' >/dev/null

REPO_SLUG="compose-ssh-$(date +%s)"
curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg s "$REPO_SLUG" '{slug:$s}')" \
  "$BASE/api/v1/orgs/demo/repos" | jq -e '.slug' >/dev/null

PAT_JSON=$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -H 'Content-Type: application/json' \
  -d '{"name":"compose-git","scopes":["repo:read","repo:write"]}' \
  "$BASE/api/v1/tokens")
PAT=$(echo "$PAT_JSON" | jq -r .token)
[[ ${#PAT} -gt 10 ]]

SSH_URL="ssh://git@127.0.0.1:${MUSEHUB_GIT_SSH_PORT}/demo/${REPO_SLUG}.git"
HTTP_URL="${BASE}/git/demo/${REPO_SLUG}.git"
ENC_PAT=$(python3 -c "import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=''))" "$PAT")
HTTP_AUTH="http://admin:${ENC_PAT}@127.0.0.1:${MUSEHUB_GATEWAY_HTTP_PORT}/git/demo/${REPO_SLUG}.git"

GIT_DIR=$(mktemp -d)
export GIT_SSH_COMMAND="ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -i $KEY_DIR/id_ed25519 -p ${MUSEHUB_GIT_SSH_PORT}"
git clone "$SSH_URL" "$GIT_DIR"
git -C "$GIT_DIR" config user.email ci@musehub.test
git -C "$GIT_DIR" config user.name ci
echo ssh-first >"$GIT_DIR/roundtrip.txt"
git -C "$GIT_DIR" add roundtrip.txt
git -C "$GIT_DIR" commit -m ssh-first
git -C "$GIT_DIR" push -u origin main

HTTP_DIR=$(mktemp -d)
git clone "$HTTP_AUTH" "$HTTP_DIR"
git -C "$HTTP_DIR" config user.email ci@musehub.test
git -C "$HTTP_DIR" config user.name ci
echo http-second >>"$HTTP_DIR/roundtrip.txt"
git -C "$HTTP_DIR" add roundtrip.txt
git -C "$HTTP_DIR" commit -m http-second
git -C "$HTTP_DIR" push

git -C "$GIT_DIR" pull --rebase
echo ssh-third >>"$GIT_DIR/roundtrip.txt"
git -C "$GIT_DIR" add roundtrip.txt
git -C "$GIT_DIR" commit -m ssh-third
git -C "$GIT_DIR" push

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
