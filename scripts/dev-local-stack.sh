#!/usr/bin/env bash
# Local dev: init / start / status / stop for musehub-server + Vite (loopback only).
# Does not reset DB, overwrite existing env, or kill processes outside our pid slots.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=lib/sed-portable.sh
source "$ROOT/scripts/lib/sed-portable.sh"
# shellcheck source=lib/dev-local-pid.sh
source "$ROOT/scripts/lib/dev-local-pid.sh"

STATE_DIR="${MUSEHUB_DEV_STATE_DIR:-$ROOT/.local/dev-stack}"
ENV_FILE="$STATE_DIR/env"
PID_DIR="$STATE_DIR/pids"
LOG_DIR="$STATE_DIR/logs"
SECRETS_DIR="$STATE_DIR/secrets"
GIT_DIR="$STATE_DIR/git"
ARTIFACT_DIR="$STATE_DIR/artifacts"

SERVER_PID="$PID_DIR/server.pid"
SERVER_MARKER="$PID_DIR/server.marker"
VITE_PID="$PID_DIR/vite.pid"
VITE_MARKER="$PID_DIR/vite.marker"
SERVER_LOG="$LOG_DIR/server.log"
VITE_LOG="$LOG_DIR/vite.log"

API_URL="${MUSEHUB_DEV_API_URL:-http://127.0.0.1:8080}"
VITE_URL="${MUSEHUB_DEV_WEB_URL:-http://127.0.0.1:5173}"

SERVER_CMD_PATTERN="musehub-server"
VITE_CMD_PATTERN="vite"

STARTED_SERVER=0
STARTED_VITE=0

usage() {
  cat <<'EOF'
Usage: ./scripts/dev-local-stack.sh <command>

  init    First-time: dirs + env (600) + migrate (idempotent). Never overwrites existing env.
  start   Migrate up, start API + Vite; exit non-zero unless /readyz and UI /login are reachable.
  status  Pids, /readyz summary, UI URL (no secrets).
  stop    Stop only processes we claimed in pid slots (identity-checked).

State: .local/dev-stack/ (override with MUSEHUB_DEV_STATE_DIR).

Requires: PostgreSQL, pg_isready, curl, jq, openssl, yarn, node.
Set MUSEHUB_DEV_WEB_URL host:port to match Vite listen (default 127.0.0.1:5173).
Not production / not remote runner — in-process executors + compose-mock profile.
EOF
}

require_tools() {
  local missing=()
  for t in curl jq openssl pg_isready cargo yarn node; do
    command -v "$t" >/dev/null 2>&1 || missing+=("$t")
  done
  if ((${#missing[@]} > 0)); then
    echo "dev_error=missing_tools ${missing[*]}" >&2
    exit 1
  fi
}

ensure_layout() {
  mkdir -p "$PID_DIR" "$LOG_DIR" "$SECRETS_DIR" "$GIT_DIR" "$ARTIFACT_DIR"
  chmod 700 "$STATE_DIR" "$SECRETS_DIR" 2>/dev/null || true
}

redact_url() {
  local url=$1
  printf '%s' "$url" | sed -E 's#(postgres://[^:]+:)[^@]+@#\1***@#'
}

vite_host_port() {
  local url=$1
  local host port
  host=$(printf '%s' "$url" | sed -E 's#^https?://([^:/]+).*#\1#')
  port=$(printf '%s' "$url" | sed -E 's#^https?://[^:/]+:([0-9]+).*#\1#')
  if [[ "$port" == "$url" || -z "$port" ]]; then
    port=5173
  fi
  host="${host:-127.0.0.1}"
  printf '%s %s\n' "$host" "$port"
}

api_host_port() {
  local bind=${1:-127.0.0.1:8080}
  local host=${bind%%:*}
  local port=${bind##*:}
  port="${port:-8080}"
  host="${host:-127.0.0.1}"
  printf '%s %s\n' "$host" "$port"
}

port_busy() {
  local port=$1
  if command -v ss >/dev/null 2>&1; then
    ss -ltn 2>/dev/null | grep -q ":${port} "
    return $?
  fi
  if command -v lsof >/dev/null 2>&1; then
    lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1
    return $?
  fi
  echo "dev_error=port_check_unavailable need ss or lsof" >&2
  return 1
}

load_env() {
  if [[ ! -f "$ENV_FILE" ]]; then
    echo "dev_error=missing_env run: ./scripts/dev-local-stack.sh init" >&2
    echo "dev_hint=env_path=$ENV_FILE" >&2
    exit 1
  fi
  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
}

write_env_if_missing() {
  ensure_layout
  if [[ -f "$ENV_FILE" ]]; then
    echo "dev_init=skip env_exists path=$ENV_FILE"
    return 0
  fi
  local db_url session bootstrap_pw
  db_url="${DATABASE_URL:-postgres://musehub:musehub@127.0.0.1:5432/musehub}"
  session="$(openssl rand -hex 32)"
  bootstrap_pw="$(openssl rand -base64 21 | tr -d '\n')"

  {
    echo '# MuseHub local dev stack (generated). chmod 600 — do not commit.'
    printf '%s=%q\n' DATABASE_URL "$db_url"
    printf '%s=%q\n' MUSEHUB_BIND "127.0.0.1:8080"
    printf '%s=%q\n' MUSEHUB_GIT_ROOT "$GIT_DIR"
    printf '%s=%q\n' MUSEHUB_ARTIFACT_ROOT "$ARTIFACT_DIR"
    printf '%s=%q\n' MUSEHUB_SESSION_SECRET "$session"
    printf '%s=%q\n' MUSEHUB_DEV_BOOTSTRAP_PASSWORD_FILE "$SECRETS_DIR/bootstrap-admin.password"
    printf '%s=%q\n' MUSEHUB_DEV_WEB_URL "$VITE_URL"
  } >"$ENV_FILE"
  chmod 600 "$ENV_FILE"

  printf '%s\n' "$bootstrap_pw" >"$SECRETS_DIR/bootstrap-admin.password"
  chmod 600 "$SECRETS_DIR/bootstrap-admin.password"

  echo "dev_init=created env=$ENV_FILE"
  echo "dev_init=git_root=$GIT_DIR"
  echo "dev_init=database_url=$(redact_url "$db_url")"
  echo "dev_init=bootstrap_admin_password_file=$SECRETS_DIR/bootstrap-admin.password"
  echo "dev_init=login_handle=admin org=demo"
}

wait_database() {
  local url=$1
  local host port
  host=$(printf '%s' "$url" | sed -E 's#^postgres://[^@]+@([^:/]+).*#\1#')
  port=$(printf '%s' "$url" | sed -E 's#^postgres://[^@]+@[^:/]+:([0-9]+)/.*#\1#')
  port="${port:-5432}"
  for _ in $(seq 1 30); do
    if pg_isready -h "$host" -p "$port" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "dev_error=postgres_not_ready host=$host port=$port" >&2
  echo "dev_hint=install postgresql client tools (pg_isready)" >&2
  return 1
}

run_migrate_up() {
  cargo run -p musehub-migrate -- up migrations "$DATABASE_URL"
}

wait_readyz() {
  local i body
  for i in $(seq 1 90); do
    if body=$(curl -sf "$API_URL/readyz" 2>/dev/null); then
      if printf '%s' "$body" | jq -e '.ready == true' >/dev/null 2>&1; then
        echo "dev_readyz=ok url=$API_URL/readyz version=$(printf '%s' "$body" | jq -r .version)"
        return 0
      fi
      echo "dev_readyz=waiting attempt=$i checks=$(printf '%s' "$body" | jq -c .checks 2>/dev/null || echo unknown)"
    else
      echo "dev_readyz=waiting attempt=$i curl_failed"
    fi
    sleep 1
  done
  echo "dev_error=readyz_not_ready url=$API_URL/readyz" >&2
  echo "dev_hint=tail $SERVER_LOG" >&2
  curl -sf "$API_URL/readyz" 2>/dev/null | jq . >&2 || true
  return 1
}

wait_vite_login() {
  local url=$1
  local i
  for i in $(seq 1 60); do
    if curl -sf "${url%/}/login" 2>/dev/null | grep -q 'id="root"'; then
      echo "dev_vite=ok url=${url%/}/login"
      return 0
    fi
    sleep 1
  done
  echo "dev_error=vite_login_unreachable url=${url%/}/login" >&2
  echo "dev_hint=tail $VITE_LOG" >&2
  return 1
}

cleanup_start_partial() {
  if [[ "$STARTED_VITE" -eq 1 ]]; then
    dev_local_slot_stop "$VITE_PID" "$VITE_MARKER" "$VITE_CMD_PATTERN" vite || true
    STARTED_VITE=0
  fi
  if [[ "$STARTED_SERVER" -eq 1 ]]; then
    dev_local_slot_stop "$SERVER_PID" "$SERVER_MARKER" "$SERVER_CMD_PATTERN" server || true
    STARTED_SERVER=0
  fi
}

assert_slot_free_or_fail() {
  local pidfile=$1 marker=$2 pattern=$3 label=$4
  dev_local_slot_clear_if_dead "$pidfile" "$marker" "$pattern"
  if dev_local_slot_occupied "$pidfile" "$marker" "$pattern"; then
    echo "dev_error=${label}_already_running pid=$(tr -d '[:space:]' <"$pidfile")" >&2
    echo "dev_hint=./scripts/dev-local-stack.sh stop" >&2
    exit 1
  fi
  if [[ -f "$pidfile" ]]; then
    local pid cmd
    pid=$(tr -d '[:space:]' <"$pidfile")
    cmd=$(dev_local_process_cmdline "$pid" 2>/dev/null) || true
    if kill -0 "$pid" 2>/dev/null && [[ "$cmd" != *"$pattern"* ]]; then
      echo "dev_error=pidfile_foreign_process label=$label pid=$pid" >&2
      echo "dev_hint=not_killing; remove $pidfile after verifying process" >&2
      exit 1
    fi
  fi
}

cmd_init() {
  require_tools
  write_env_if_missing
  load_env
  wait_database "$DATABASE_URL"
  run_migrate_up
  echo "dev_init=migrate=ok"
  echo "dev_init=next ./scripts/dev-local-stack.sh start"
}

cmd_start() {
  require_tools
  load_env
  ensure_layout
  STARTED_SERVER=0
  STARTED_VITE=0
  local start_ok=0
  trap 'if [[ $start_ok -eq 0 ]]; then cleanup_start_partial; fi' EXIT

  local vite_host vite_port api_port _api_host
  read -r vite_host vite_port < <(vite_host_port "${MUSEHUB_DEV_WEB_URL:-$VITE_URL}")
  read -r _api_host api_port < <(api_host_port "${MUSEHUB_BIND:-127.0.0.1:8080}")

  assert_slot_free_or_fail "$SERVER_PID" "$SERVER_MARKER" "$SERVER_CMD_PATTERN" server
  assert_slot_free_or_fail "$VITE_PID" "$VITE_MARKER" "$VITE_CMD_PATTERN" vite

  if port_busy "$api_port"; then
    echo "dev_error=port_in_use port=$api_port (MUSEHUB_BIND)" >&2
    exit 1
  fi
  if port_busy "$vite_port"; then
    echo "dev_error=port_in_use port=$vite_port (MUSEHUB_DEV_WEB_URL)" >&2
    exit 1
  fi

  wait_database "$DATABASE_URL"
  run_migrate_up

  echo "dev_start=server bind=${MUSEHUB_BIND:-127.0.0.1:8080} log=$SERVER_LOG"
  cargo build -q -p musehub-server

  local server_pid
  (
    export MUSEHUB_BIND MUSEHUB_GIT_ROOT MUSEHUB_ARTIFACT_ROOT MUSEHUB_SESSION_SECRET DATABASE_URL
    if [[ -n "${MUSEHUB_DEV_BOOTSTRAP_PASSWORD_FILE:-}" && -f "${MUSEHUB_DEV_BOOTSTRAP_PASSWORD_FILE}" ]]; then
      read -r MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD <"${MUSEHUB_DEV_BOOTSTRAP_PASSWORD_FILE}"
      export MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
    fi
    cd "$ROOT" && exec "$ROOT/target/debug/musehub-server" >>"$SERVER_LOG" 2>&1
  ) &
  server_pid=$!
  dev_local_slot_claim "$SERVER_PID" "$SERVER_MARKER" "$server_pid" "$SERVER_CMD_PATTERN"
  STARTED_SERVER=1

  if ! wait_readyz; then
    cleanup_start_partial
    exit 1
  fi

  if ! dev_local_slot_occupied "$VITE_PID" "$VITE_MARKER" "$VITE_CMD_PATTERN"; then
    if [[ ! -d "$ROOT/web/node_modules" ]]; then
      yarn --cwd "$ROOT/web" install
    fi
    echo "dev_start=vite host=$vite_host port=$vite_port log=$VITE_LOG"
    local vite_pid
    (
      cd "$ROOT/web" && exec ./node_modules/.bin/vite --host "$vite_host" --port "$vite_port" >>"$VITE_LOG" 2>&1
    ) &
    vite_pid=$!
    dev_local_slot_claim "$VITE_PID" "$VITE_MARKER" "$vite_pid" "$VITE_CMD_PATTERN"
    STARTED_VITE=1

    if ! wait_vite_login "${MUSEHUB_DEV_WEB_URL:-$VITE_URL}"; then
      cleanup_start_partial
      exit 1
    fi
  fi

  echo "dev_start=ui ${MUSEHUB_DEV_WEB_URL:-$VITE_URL}/login"
  echo "dev_start=api $API_URL/readyz"
  echo "dev_profile=demo mock_executors in_process (not production remote worker)"
  start_ok=1
  trap - EXIT
}

cmd_status() {
  ensure_layout
  if [[ -f "$ENV_FILE" ]]; then
    load_env
    echo "dev_status=env $ENV_FILE"
    echo "dev_status=database $(redact_url "${DATABASE_URL:-unset}")"
    echo "dev_status=git_root ${MUSEHUB_GIT_ROOT:-unset}"
  else
    echo "dev_status=env missing path=$ENV_FILE"
  fi

  if dev_local_slot_occupied "$SERVER_PID" "$SERVER_MARKER" "$SERVER_CMD_PATTERN"; then
    echo "dev_status=server pid=$(tr -d '[:space:]' <"$SERVER_PID")"
  else
    echo "dev_status=server stopped"
  fi
  if dev_local_slot_occupied "$VITE_PID" "$VITE_MARKER" "$VITE_CMD_PATTERN"; then
    echo "dev_status=vite pid=$(tr -d '[:space:]' <"$VITE_PID") url=${MUSEHUB_DEV_WEB_URL:-$VITE_URL}"
  else
    echo "dev_status=vite stopped"
  fi

  if curl -sf "$API_URL/readyz" >/dev/null 2>&1; then
    curl -sf "$API_URL/readyz" | jq '{ready,version,checks}'
  else
    echo "dev_status=readyz unreachable url=$API_URL/readyz"
  fi
}

cmd_stop() {
  ensure_layout
  if [[ -f "$ENV_FILE" ]]; then
    set -a
    # shellcheck disable=SC1090
    source "$ENV_FILE"
    set +a
  fi
  local api_port vite_port _h
  read -r _h vite_port < <(vite_host_port "${MUSEHUB_DEV_WEB_URL:-$VITE_URL}")
  read -r _h api_port < <(api_host_port "${MUSEHUB_BIND:-127.0.0.1:8080}")

  local stop_ok=0
  dev_local_slot_stop "$VITE_PID" "$VITE_MARKER" "$VITE_CMD_PATTERN" vite || stop_ok=1
  dev_local_slot_stop "$SERVER_PID" "$SERVER_MARKER" "$SERVER_CMD_PATTERN" server || stop_ok=1

  local i
  for i in $(seq 1 40); do
    if ! port_busy "$api_port" && ! port_busy "$vite_port"; then
      break
    fi
    sleep 0.25
  done

  if port_busy "$api_port" || port_busy "$vite_port"; then
    echo "dev_warn=ports_still_busy api=$api_port vite=$vite_port" >&2
    stop_ok=1
  fi

  echo "dev_stop=data_preserved state_dir=$STATE_DIR"
  return "$stop_ok"
}

main() {
  local cmd=${1:-}
  case "$cmd" in
    init) cmd_init ;;
    start) cmd_start ;;
    status) cmd_status ;;
    stop) cmd_stop ;;
    -h | --help | help | '') usage ;;
    *)
      echo "dev_error=unknown_command $cmd" >&2
      usage >&2
      exit 1
      ;;
  esac
}

main "$@"
