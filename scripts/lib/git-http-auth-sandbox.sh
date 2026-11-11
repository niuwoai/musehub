#!/usr/bin/env bash
# Ephemeral GIT_CONFIG_GLOBAL: PAT only in mode-0600 file (never git remote URL / argv).
set -euo pipefail

GIT_HTTP_AUTH_DIR=""
GIT_HTTP_AUTH_CFG=""

redact_git_output() {
  local text="$1"
  local pat="${GIT_HTTP_SANDBOX_PAT:-}"
  if [[ -n "$pat" ]]; then
    text="${text//"$pat"/[REDACTED_PAT]}"
    local basic
    basic=$(printf 'admin:%s' "$pat" | base64 -w0 2>/dev/null || printf 'admin:%s' "$pat" | base64)
    text="${text//"$basic"/[REDACTED_BASIC]}"
  fi
  text="$(echo "$text" | sed -E 's/Authorization: Basic [A-Za-z0-9+/=]+/Authorization: Basic [REDACTED]/g')"
  text="$(echo "$text" | sed -E 's/mhp_[A-Za-z0-9_-]+/[REDACTED_PAT]/g')"
  printf '%s' "$text"
}

git_http_auth_sandbox_begin() {
  local pat="$1"
  GIT_HTTP_SANDBOX_PAT="$pat"
  GIT_HTTP_AUTH_DIR=$(mktemp -d)
  chmod 0700 "$GIT_HTTP_AUTH_DIR"
  GIT_HTTP_AUTH_CFG="$GIT_HTTP_AUTH_DIR/config"
  local basic
  basic=$(printf 'admin:%s' "$pat" | base64 -w0 2>/dev/null || printf 'admin:%s' "$pat" | base64)
  printf '[credential]\n\thelper =\n[http]\n\textraHeader = Authorization: Basic %s\n' "$basic" >"$GIT_HTTP_AUTH_CFG"
  chmod 0600 "$GIT_HTTP_AUTH_CFG"
  export GIT_CONFIG_GLOBAL="$GIT_HTTP_AUTH_CFG"
  export GIT_CONFIG_SYSTEM=/dev/null
  export GIT_CONFIG_NOSYSTEM=1
  export GIT_TERMINAL_PROMPT=0
  export GIT_ASKPASS=
  export GIT_CONFIG_COUNT=0
}

git_http_auth_sandbox_end() {
  unset GIT_CONFIG_GLOBAL GIT_CONFIG_SYSTEM GIT_CONFIG_NOSYSTEM GIT_HTTP_SANDBOX_PAT
  if [[ -n "${GIT_HTTP_AUTH_DIR:-}" ]]; then
    rm -rf "$GIT_HTTP_AUTH_DIR"
    GIT_HTTP_AUTH_DIR=""
    GIT_HTTP_AUTH_CFG=""
  fi
}

git_http_credential_free_url() {
  local base="$1" org="$2" repo="$3"
  local host_port="${base#http://}"
  host_port="${host_port%%/*}"
  printf 'http://%s/git/%s/%s.git' "$host_port" "$org" "$repo"
}

git_http_run() {
  local dir="${1:-}"; shift
  local err_file
  err_file=$(mktemp)
  local status=0
  if [[ -n "$dir" ]]; then
    git -C "$dir" "$@" 2>"$err_file" || status=$?
  else
    git "$@" 2>"$err_file" || status=$?
  fi
  if [[ "$status" -ne 0 ]]; then
    local err
    err="$(redact_git_output "$(cat "$err_file")")"
    rm -f "$err_file"
    echo "git failed (exit $status): $err" >&2
    return "$status"
  fi
  rm -f "$err_file"
  return 0
}
