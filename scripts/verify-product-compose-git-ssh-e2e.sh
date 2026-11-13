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
command -v /usr/bin/timeout >/dev/null || { echo "/usr/bin/timeout required" >&2; exit 1; }

: "${COMPOSE_PROJECT_NAME:?}"
: "${BASE:?}"
: "${BOOTSTRAP_PW:?}"
: "${POSTGRES_PASSWORD:?}"

COMPOSE_GIT_TIMEOUT_SEC="${COMPOSE_GIT_TIMEOUT_SEC:-180}"
# shellcheck source=lib/compose-dc-timeout.sh
source "$ROOT/scripts/lib/compose-dc-timeout.sh"

wait_gateway_ready() {
  for _ in $(seq 1 90); do
    if curl -fsS "$BASE/readyz" | jq -e '.ready == true and .checks.product_api == "ok"' >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  echo "compose git-ssh: gateway not ready ($BASE/readyz)" >&2
  return 1
}
: "${MUSEHUB_GATEWAY_HTTP_PORT:?}"
: "${MUSEHUB_GIT_SSH_PORT:=2222}"

FEATURE_BRANCH="feature/compose-roundtrip"
STORAGE_KEY=""
TOKEN_ID=""
KEY_DIR=""
GIT_DIR=""
HTTP_DIR=""
BAD_DIR=""
BARE_REPO_PATH=""
GIT_SSH_CLI_OPTS=()

if [[ -n "${MUSEHUB_STACK_DC:-}" ]]; then
  dc() { eval "$MUSEHUB_STACK_DC $(printf '%q ' "$@")"; }
else
  dc() { docker compose -p "$COMPOSE_PROJECT_NAME" "$@"; }
fi

compose_ssh_cli_base() {
  GIT_SSH_CLI_OPTS=(
    -o BatchMode=yes
    -o StrictHostKeyChecking=no
    -o UserKnownHostsFile=/dev/null
    -o ControlMaster=no
    -o ConnectTimeout=30
    -o ServerAliveInterval=15
    -o ServerAliveCountMax=4
    -i "$KEY_DIR/id_ed25519"
    -p "${MUSEHUB_GIT_SSH_PORT}"
  )
  export GIT_SSH_COMMAND="ssh ${GIT_SSH_CLI_OPTS[*]}"
}

git_ssh_fail() {
  local ctx="$1" status="$2" err_file="$3"
  local err
  err="$(redact_git_output "$(cat "$err_file")")"
  rm -f "$err_file"
  echo "compose git-ssh: ${ctx} failed (exit=${status})" >&2
  [[ -n "$err" ]] && echo "compose git-ssh: ${ctx} stderr: ${err}" >&2
  if [[ -n "${STORAGE_KEY:-}" ]]; then
    log_bare_repo_transport_state "${ctx}"
  fi
  return "$status"
}

git_ssh_git() {
  local work_dir="${1:-}"; shift || true
  local ctx="${1:-git}"; shift || true
  local err_file status
  err_file=$(mktemp)
  status=0
  if [[ -n "$work_dir" ]]; then
    (
      unset GIT_CONFIG_GLOBAL GIT_CONFIG_SYSTEM GIT_CONFIG_NOSYSTEM GIT_HTTP_SANDBOX_PAT
      export GIT_TERMINAL_PROMPT=0
      compose_git_timeout git -C "$work_dir" "$@"
    ) 2>"$err_file" || status=$?
  else
    (
      unset GIT_CONFIG_GLOBAL GIT_CONFIG_SYSTEM GIT_CONFIG_NOSYSTEM GIT_HTTP_SANDBOX_PAT
      export GIT_TERMINAL_PROMPT=0
      compose_git_timeout git "$@"
    ) 2>"$err_file" || status=$?
  fi
  if [[ "$status" -eq 124 ]]; then
    echo "compose git-ssh: ${ctx} timed out after ${COMPOSE_GIT_TIMEOUT_SEC}s" >&2
    git_ssh_fail "${ctx} (timeout)" "$status" "$err_file" || true
    return 124
  fi
  if [[ "$status" -ne 0 ]]; then
    git_ssh_fail "$ctx" "$status" "$err_file"
    return "$status"
  fi
  rm -f "$err_file"
  return 0
}

assert_push_rejected() {
  local label="$1" expected_fragment="$2"
  shift 2
  local err_file status err
  err_file=$(mktemp)
  status=0
  (
    unset GIT_CONFIG_GLOBAL GIT_CONFIG_SYSTEM GIT_CONFIG_NOSYSTEM GIT_HTTP_SANDBOX_PAT
    export GIT_TERMINAL_PROMPT=0
    compose_git_timeout git -C "$GIT_DIR" push "$@"
  ) 2>"$err_file" || status=$?
  err="$(redact_git_output "$(cat "$err_file")")"
  rm -f "$err_file"
  if [[ "$status" -eq 0 ]]; then
    echo "compose git-ssh: ${label} unexpectedly succeeded" >&2
    return 1
  fi
  if [[ "$status" -eq 124 ]]; then
    echo "compose git-ssh: ${label} timed out after ${COMPOSE_GIT_TIMEOUT_SEC}s" >&2
    return 1
  fi
  if ! grep -qi "$expected_fragment" <<<"$err"; then
    echo "compose git-ssh: ${label} want message matching ${expected_fragment}, exit=${status} stderr=${err}" >&2
    return 1
  fi
  return 0
}

log_bare_repo_transport_state() {
  local label="${1:-diagnostics}"
  local bare="${BARE_REPO_PATH:-/var/lib/musehub/git/${STORAGE_KEY}.git}"
  echo "=== compose git-ssh: bare transport diagnostics (${label}) ===" >&2
  dc exec -T musehub-server git --git-dir="$bare" config --get core.sharedRepository 2>&1 | sed 's/^/  sharedRepository=/' >&2 || true
  dc exec -T musehub-server stat -c '  bare %a %u:%g' "$bare" >&2 || true
  for sub in objects refs; do
    dc exec -T musehub-server stat -c "  ${sub} %a %u:%g" "$bare/$sub" >&2 || true
  done
  dc exec -T musehub-server sh -c "test -w '$bare' && echo '  musehub write bare=ok' || echo '  musehub write bare=denied'" >&2 || true
  dc exec -T musehub-git-ssh sh -c "id git; test -w '$bare' && echo '  git write bare=ok' || echo '  git write bare=denied'" >&2 || true
}

# shellcheck source=lib/forward-echo-probe.sh
source "$ROOT/scripts/lib/forward-echo-probe.sh"

assert_port_forward_denied() {
  local err_file pid port_local=19999 w=0 echo_ok=0 target_port target_pid target_log nonce
  command -v python3 >/dev/null 2>&1 || {
    echo "compose git-ssh: python3 required for forward echo probe" >&2
    return 1
  }
  target_log=$(mktemp)
  target_pid=$(forward_echo_start_target "$target_log")
  for _ in $(seq 1 30); do
    [[ -s "$target_log" ]] && break
    sleep 0.1
  done
  target_port="$(cat "$target_log")"
  [[ -n "$target_port" ]] || {
    echo "compose git-ssh: echo target failed to bind" >&2
    kill "$target_pid" 2>/dev/null || true
    wait "$target_pid" 2>/dev/null || true
    rm -f "$target_log"
    return 1
  }
  err_file=$(mktemp)
  nonce="musehub-compose-probe-$RANDOM"
  compose_git_timeout ssh "${GIT_SSH_CLI_OPTS[@]}" \
    -o ExitOnForwardFailure=yes \
    -L "127.0.0.1:${port_local}:127.0.0.1:${target_port}" \
    git@127.0.0.1 -N 2>"$err_file" &
  pid=$!
  while [[ "$w" -lt 8 ]]; do
    if ! kill -0 "$pid" 2>/dev/null; then
      break
    fi
    if forward_echo_probe "$port_local" "$nonce"; then
      echo_ok=1
      break
    fi
    sleep 1
    w=$((w + 1))
  done
  if kill -0 "$pid" 2>/dev/null; then
    kill -TERM "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  else
    wait "$pid" 2>/dev/null || true
  fi
  kill -TERM "$target_pid" 2>/dev/null || true
  wait "$target_pid" 2>/dev/null || true
  rm -f "$target_log"
  local msg
  msg="$(redact_git_output "$(cat "$err_file")")"
  rm -f "$err_file"
  if [[ "$echo_ok" -eq 1 ]]; then
    echo "compose git-ssh: forward echo roundtrip succeeded (channel opened)" >&2
    return 1
  fi
  if [[ -z "${msg// }" ]]; then
    echo "compose git-ssh: port forward negative requires ssh stderr after echo probe" >&2
    return 1
  fi
  if grep -qiE 'administratively prohibited|port forwarding disabled|forwarding disabled' <<<"$msg"; then
    echo "compose git-ssh: port forward denied as expected (echo_ok=${echo_ok})" >&2
    return 0
  fi
  echo "compose git-ssh: port forward negative inconclusive (sanitized): ${msg}" >&2
  return 1
}

assert_unknown_ssh_key_rejected() {
  local err_file dest st
  err_file=$(mktemp)
  dest=$(mktemp -d)
  BAD_DIR=$(mktemp -d)
  ssh-keygen -t ed25519 -f "$BAD_DIR/bad" -N "" -q
  local bad_cli="ssh -o BatchMode=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ControlMaster=no -o ConnectTimeout=30 -i $BAD_DIR/bad -p ${MUSEHUB_GIT_SSH_PORT}"
  st=0
  if compose_git_timeout git -c "core.sshCommand=${bad_cli}" \
    clone "$SSH_URL" "$dest" 2>"$err_file"; then
    st=1
  else
    st=$?
  fi
  local msg
  msg="$(redact_git_output "$(cat "$err_file")")"
  rm -f "$err_file"
  rm -rf "$dest" "$BAD_DIR"
  if [[ "$st" -eq 0 ]]; then
    echo "compose git-ssh: unregistered key must not clone" >&2
    return 1
  fi
  if [[ "$st" -eq 124 ]]; then
    echo "compose git-ssh: unregistered key auth check timed out" >&2
    return 1
  fi
  if grep -qiE 'Permission denied|publickey' <<<"$msg"; then
    echo "compose git-ssh: unregistered key rejected (auth evidence in stderr)" >&2
    return 0
  fi
  echo "compose git-ssh: want Permission denied or publickey in stderr, got exit=${st} stderr=${msg}" >&2
  return 1
}

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
  [[ -n "${GIT_DIR:-}" ]] && rm -rf "$GIT_DIR" 2>/dev/null || true
  [[ -n "${HTTP_DIR:-}" ]] && rm -rf "$HTTP_DIR"
  [[ -n "${BAD_DIR:-}" ]] && rm -rf "$BAD_DIR"
  if [[ -z "${MUSEHUB_STACK_DC:-}" ]] && [[ -n "${COOKIE_JAR:-}" ]]; then
    rm -f "$COOKIE_JAR"
  fi
}
trap cleanup EXIT

remote_ref_oid() {
  local ref="$1"
  compose_dc_timeout exec -T musehub-server git --git-dir="${BARE_REPO_PATH}" rev-parse "$ref"
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
[[ "$run_dir_mode" =~ ^(2750|0750|750)$ && "$run_dir_own" == root:git ]] || {
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

wait_gateway_ready

compose_git_ssh_login() {
  local tmp code body
  tmp=$(mktemp)
  for _ in $(seq 1 30); do
    code=$(curl -sS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -H 'Content-Type: application/json' \
      -d "$(jq -n --arg h admin --arg p "$BOOTSTRAP_PW" '{handle:$h,password:$p}')" \
      -o "$tmp" -w "%{http_code}" "$BASE/api/v1/auth/login" || echo "000")
    body=$(cat "$tmp")
    if [[ "$code" == "200" ]] && jq -e '.ok == true' <<<"$body" >/dev/null 2>&1; then
      rm -f "$tmp"
      return 0
    fi
    if [[ "$code" =~ ^[45] ]]; then
      echo "compose git-ssh: login retry (http=$code)" >&2
      sleep 2
      continue
    fi
    echo "compose git-ssh: login failed http=$code body=$(head -c 400 "$body")" >&2
    rm -f "$tmp"
    return 1
  done
  echo "compose git-ssh: login gave up after retries" >&2
  rm -f "$tmp"
  return 1
}

compose_api_json() {
  local method="$1" url="$2" data="${3:-}" tmp code body
  tmp=$(mktemp)
  if [[ -n "$data" ]]; then
    code=$(curl -sS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -X "$method" \
      -H 'Content-Type: application/json' -d "$data" -o "$tmp" -w "%{http_code}" "$url")
  else
    code=$(curl -sS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -X "$method" \
      -o "$tmp" -w "%{http_code}" "$url")
  fi
  body=$(cat "$tmp")
  rm -f "$tmp"
  code="${code//$'\r'/}"
  if [[ "$code" -lt 200 || "$code" -ge 300 ]]; then
    echo "compose git-ssh: ${method} ${url} failed http=${code} body=${body}" >&2
    dc logs musehub-server --tail=40 >&2 || true
    exit 1
  fi
  printf '%s' "$body"
}

if [[ -n "${MUSEHUB_STACK_DC:-}" && -n "${COOKIE_JAR:-}" ]]; then
  [[ -f "$COOKIE_JAR" ]] || {
    echo "compose git-ssh: missing session cookie jar (${COOKIE_JAR})" >&2
    exit 1
  }
  echo "=== compose git-ssh: reuse stack admin session (embedded) ==="
else
  COOKIE_JAR=$(mktemp)
  compose_git_ssh_login
fi

KEY_DIR=$(mktemp -d)
chmod 0700 "$KEY_DIR"
ssh-keygen -t ed25519 -f "$KEY_DIR/id_ed25519" -N "" -q
compose_ssh_cli_base
PUB=$(cat "$KEY_DIR/id_ed25519.pub")

echo "=== compose git-ssh: register ssh key ==="
SSH_KEY_JSON=$(compose_api_json POST "$BASE/api/v1/ssh-keys" \
  "$(jq -n --arg k "$PUB" '{name:"compose-ssh",public_key:$k}')")
echo "$SSH_KEY_JSON" | jq -e '.id' >/dev/null || {
  echo "compose git-ssh: unexpected ssh-keys response: ${SSH_KEY_JSON}" >&2
  exit 1
}

REPO_SLUG="compose-ssh-$(date +%s)"
REPO_JSON=$(compose_api_json POST "$BASE/api/v1/orgs/demo/repos" \
  "$(jq -n --arg s "$REPO_SLUG" '{slug:$s}')")
REPO_ID=$(echo "$REPO_JSON" | jq -r .id)
[[ "$(echo "$REPO_JSON" | jq -r .id)" != null && -n "$REPO_ID" ]] || {
  echo "compose git-ssh: repo create missing id: ${REPO_JSON}" >&2
  exit 1
}
STORAGE_KEY=$(dc exec -T -e PGPASSWORD="$POSTGRES_PASSWORD" postgres \
  psql -U musehub -d musehub -tAc \
  "SELECT storage_key::text FROM repositories WHERE id = '${REPO_ID}'")
STORAGE_KEY=$(echo "$STORAGE_KEY" | tr -d '[:space:]')
[[ ${#STORAGE_KEY} -ge 32 ]] || {
  echo "compose git-ssh: could not resolve storage_key for repo ${REPO_ID}" >&2
  exit 1
}
BARE_REPO_PATH="/var/lib/musehub/git/${STORAGE_KEY}.git"

SSH_URL="ssh://git@127.0.0.1:${MUSEHUB_GIT_SSH_PORT}/demo/${REPO_SLUG}.git"

echo "=== compose git-ssh: SSH clone (no HTTP PAT sandbox) ==="
GIT_DIR=$(mktemp -d)
git_ssh_git "" "ssh-clone" clone "$SSH_URL" "$GIT_DIR"
git -C "$GIT_DIR" config user.email ci@musehub.test
git -C "$GIT_DIR" config user.name ci

MAIN_BEFORE=$(remote_ref_oid refs/heads/main)

echo "=== compose git-ssh: protected main (direct / force / delete) ==="
git -C "$GIT_DIR" checkout -B "$FEATURE_BRANCH"
echo probe >"$GIT_DIR/probe.txt"
git -C "$GIT_DIR" add probe.txt
git -C "$GIT_DIR" commit -m probe-main-denial-test --no-gpg-sign

assert_push_rejected "main-direct" "protected branch" origin "HEAD:refs/heads/main"
[[ "$(remote_ref_oid refs/heads/main)" == "$MAIN_BEFORE" ]]

assert_push_rejected "main-force" "protected branch" --force origin "HEAD:refs/heads/main"
[[ "$(remote_ref_oid refs/heads/main)" == "$MAIN_BEFORE" ]]

assert_push_rejected "main-delete" "protected branch" origin ":refs/heads/main"
[[ "$(remote_ref_oid refs/heads/main)" == "$MAIN_BEFORE" ]]

PAT_JSON=$(compose_api_json POST "$BASE/api/v1/tokens" \
  "$(jq -n --arg id "$REPO_ID" '{name:"compose-git",org:"demo",scope_mode:"selected",scopes:["repo:read","repo:write"],repo_ids:[$id]}')")
PAT=$(echo "$PAT_JSON" | jq -r .token)
TOKEN_ID=$(echo "$PAT_JSON" | jq -r .id)
[[ ${#PAT} -gt 10 ]]
git_http_auth_sandbox_begin "$PAT"
HTTP_URL="$(git_http_credential_free_url "http://127.0.0.1:${MUSEHUB_GATEWAY_HTTP_PORT}" demo "$REPO_SLUG")"

echo "=== compose git-ssh: SSH / HTTP alternation on ${FEATURE_BRANCH} (same worktree, thin pack) ==="
git -C "$GIT_DIR" checkout -B "$FEATURE_BRANCH"
echo "ssh-1" >"$GIT_DIR/roundtrip.txt"
git -C "$GIT_DIR" add roundtrip.txt
git -C "$GIT_DIR" commit -m ssh-1 --no-gpg-sign
git_ssh_git "$GIT_DIR" "ssh-1-push" push -u origin "HEAD:refs/heads/${FEATURE_BRANCH}"
OID_SSH_1=$(git -C "$GIT_DIR" rev-parse HEAD)
[[ "$(remote_ref_oid "refs/heads/${FEATURE_BRANCH}")" == "$OID_SSH_1" ]]

git -C "$GIT_DIR" remote set-url origin "$HTTP_URL"
echo "http-2" >>"$GIT_DIR/roundtrip.txt"
git -C "$GIT_DIR" add roundtrip.txt
git -C "$GIT_DIR" commit -m http-2 --no-gpg-sign
git_http_run "$GIT_DIR" push origin "HEAD:refs/heads/${FEATURE_BRANCH}"
OID_HTTP_2=$(git -C "$GIT_DIR" rev-parse HEAD)
[[ "$(remote_ref_oid "refs/heads/${FEATURE_BRANCH}")" == "$OID_HTTP_2" ]]
[[ "$OID_HTTP_2" != "$OID_SSH_1" ]]

git -C "$GIT_DIR" remote set-url origin "$SSH_URL"
echo "ssh-3" >>"$GIT_DIR/roundtrip.txt"
git -C "$GIT_DIR" add roundtrip.txt
git -C "$GIT_DIR" commit -m ssh-3 --no-gpg-sign
git_ssh_git "$GIT_DIR" "ssh-3-push" push origin "HEAD:refs/heads/${FEATURE_BRANCH}"
OID_SSH_3=$(git -C "$GIT_DIR" rev-parse HEAD)
[[ "$(remote_ref_oid "refs/heads/${FEATURE_BRANCH}")" == "$OID_SSH_3" ]]
[[ "$OID_SSH_3" != "$OID_HTTP_2" ]]

dc exec -T musehub-git-ssh sh -c '
  ! grep -q "DATABASE_URL=" /usr/local/lib/musehub/musehub-git-login.sh
  ! grep -q "postgres://" /var/lib/musehub/git-ssh/akc-wrap.sh
'

echo "=== compose git-ssh: port forward denied ==="
assert_port_forward_denied

echo "=== compose git-ssh: unknown key rejected ==="
assert_unknown_ssh_key_rejected

echo "product_compose_git_ssh_e2e=ok"
