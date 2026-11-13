#!/usr/bin/env bash
# compose_dc_timeout: run docker compose (or MUSEHUB_STACK_DC prefix) under timeout.
# MUSEHUB_STACK_DC is built with `printf '%q ' "${COMPOSE_BASE[@]}"` — do NOT re-%q the whole string.
set -euo pipefail

: "${COMPOSE_GIT_TIMEOUT_SEC:=180}"

compose_git_timeout() {
  /usr/bin/timeout -k 15 "${COMPOSE_GIT_TIMEOUT_SEC}" "$@"
}

compose_dc() {
  if [[ -n "${MUSEHUB_STACK_DC:-}" ]]; then
    # shellcheck disable=SC2086
    eval "$MUSEHUB_STACK_DC $(printf '%q ' "$@")"
  else
    docker compose -p "${COMPOSE_PROJECT_NAME:?}" "$@"
  fi
}

compose_dc_timeout() {
  if [[ -n "${MUSEHUB_STACK_DC:-}" ]]; then
    compose_git_timeout bash -c "${MUSEHUB_STACK_DC}$(printf ' %q' "$@")"
  else
    compose_git_timeout docker compose -p "${COMPOSE_PROJECT_NAME:?}" "$@"
  fi
}
