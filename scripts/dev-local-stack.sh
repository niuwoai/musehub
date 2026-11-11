#!/usr/bin/env bash
# Local dev: init / start / status / stop for musehub-server + Vite (loopback only).
# Does not reset DB, overwrite existing env, or kill processes outside our pidfiles.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=lib/sed-portable.sh
source "$ROOT/scripts/lib/sed-portable.sh"

STATE_DIR="${MUSEHUB_DEV_STATE_DIR:-$ROOT/.local/dev-stack}"
ENV_FILE="$STATE_DIR/env"
PID_DIR="$STATE_DIR/pids"
LOG_DIR="$STATE_DIR/logs"
SECRETS_DIR="$STATE_DIR/secrets"
GIT_DIR="$STATE_DIR/git"
ARTIFACT_DIR="$STATE_DIR/artifacts"

SERVER_PID="$PID_DIR/server.pid"
VITE_PID="$PID_DIR/vite.pid"
SERVER_LOG="$LOG_DIR/server.log"
VITE_LOG="$LOG_DIR/vite.log"
API_URL="${MUSEHUB_DEV_API_URL:-http://127.0.0.1:8080}"
VITE_URL="${MUSEHUB_DEV_WEB_URL:-http://127.0.0.1:5173}"

usage() {
  cat <<'EOF'
Usage: ./scripts/dev-local-stack.sh <command>

  init    First-time: dirs + env (600) + migrate (idempotent). Never overwrites existing env.
  start   Migrate up, start API + Vite; wait until /readyz.ready==true.
  status  Pids, /readyz summary, UI URL (no secrets).
  stop    Stop only processes recorded in .local/dev-stack/pids/.

State: .local/dev-stack/ (override with MUSEHUB_DEV_STATE_DIR).

Requires: PostgreSQL reachable via DATABASE_URL in env file; Rust + yarn.
Not production / not remote runner — API defaults to in-process executors + compose-mock profile.
EOF
}

ensure_layout() {
  mkdir -p "$PID_DIR" "$LOG_DIR" "$SECRETS_DIR" "$GIT_DIR" "$ARTIFACT_DIR"
  chmod 700 "$STATE_DIR" "$SECRETS_DIR" 2>/dev/null || true
}

redact_url() {
  local url=$1
  printf '%s' "$url" | sed -E 's#(postgres://[^:]+:)[^@]+@#\1***@#'
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
  return 1
}

pid_alive() {
  local f=$1
  [[ -f "$f" ]] || return 1
  local pid
  pid=$(cat "$f")
  kill -0 "$pid" 2>/dev/null
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
  db_url="${DATABASE_URL:-postgres://musehub:musehub@127.0.0.1:5432/musehub_dev}"
  session="$(openssl rand -hex 32)"
  bootstrap_pw="$(openssl rand -base64 21 | tr -d '\n')"

  cat >"$ENV_FILE" <<EOF
# MuseHub local dev stack (generated). chmod 600 — do not commit.
DATABASE_URL=$db_url
MUSEHUB_BIND=127.0.0.1:8080
MUSEHUB_GIT_ROOT=$GIT_DIR
MUSEHUB_ARTIFACT_ROOT=$ARTIFACT_DIR
MUSEHUB_SESSION_SECRET=$session
MUSEHUB_DEV_BOOTSTRAP_PASSWORD_FILE=$SECRETS_DIR/bootstrap-admin.password
EOF
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
  if command -v pg_isready >/dev/null 2>&1; then
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
    echo "dev_hint=start_postgres e.g. docker run -d --name musehub-pg -e POSTGRES_USER=musehub -e POSTGRES_PASSWORD=musehub -e POSTGRES_DB=musehub_dev -p 5432:5432 postgres:17" >&2
    return 1
  fi
  return 0
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

cmd_init() {
  write_env_if_missing
  load_env
  wait_database "$DATABASE_URL"
  run_migrate_up
  echo "dev_init=migrate=ok"
  echo "dev_init=next ./scripts/dev-local-stack.sh start"
}

cmd_start() {
  load_env
  ensure_layout

  if pid_alive "$SERVER_PID"; then
    echo "dev_error=server_already_running pid=$(cat "$SERVER_PID")" >&2
    echo "dev_hint=./scripts/dev-local-stack.sh status" >&2
    exit 1
  fi

  local api_port vite_port
  api_port="${MUSEHUB_BIND##*:}"
  vite_port="5173"
  if port_busy "$api_port"; then
    echo "dev_error=port_in_use port=$api_port (MUSEHUB_BIND)" >&2
    echo "dev_hint=stop other listener or change MUSEHUB_BIND in $ENV_FILE" >&2
    exit 1
  fi
  if port_busy "$vite_port"; then
    echo "dev_error=port_in_use port=$vite_port (vite)" >&2
    echo "dev_hint=stop other vite or set VITE port in web dev command manually" >&2
    exit 1
  fi

  wait_database "$DATABASE_URL"
  run_migrate_up

  export MUSEHUB_BIND MUSEHUB_GIT_ROOT MUSEHUB_ARTIFACT_ROOT MUSEHUB_SESSION_SECRET
  export DATABASE_URL
  if [[ -n "${MUSEHUB_DEV_BOOTSTRAP_PASSWORD_FILE:-}" && -f "${MUSEHUB_DEV_BOOTSTRAP_PASSWORD_FILE}" ]]; then
    MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD="$(cat "${MUSEHUB_DEV_BOOTSTRAP_PASSWORD_FILE}")"
    export MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
  fi

  echo "dev_start=server bind=$MUSEHUB_BIND log=$SERVER_LOG"
  cargo build -q -p musehub-server
  (cd "$ROOT" && exec "$ROOT/target/debug/musehub-server" >>"$SERVER_LOG" 2>&1) &
  echo $! >"$SERVER_PID"

  wait_readyz

  if ! pid_alive "$VITE_PID"; then
    if [[ ! -d "$ROOT/web/node_modules" ]]; then
      yarn --cwd "$ROOT/web" install
    fi
    echo "dev_start=vite url=$VITE_URL log=$VITE_LOG"
    (cd "$ROOT/web" && exec ./node_modules/.bin/vite --host 127.0.0.1 --port 5173 >>"$VITE_LOG" 2>&1) &
    echo $! >"$VITE_PID"
    for _ in $(seq 1 60); do
      if curl -sf "$VITE_URL/" >/dev/null 2>&1; then
        break
      fi
      sleep 1
    done
  fi

  echo "dev_start=ui $VITE_URL/login"
  echo "dev_start=api $API_URL/readyz"
  echo "dev_profile=demo mock_executors in_process (not production remote worker)"
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

  if pid_alive "$SERVER_PID"; then
    echo "dev_status=server pid=$(cat "$SERVER_PID")"
  else
    echo "dev_status=server stopped"
  fi
  if pid_alive "$VITE_PID"; then
    echo "dev_status=vite pid=$(cat "$VITE_PID") url=$VITE_URL"
  else
    echo "dev_status=vite stopped"
  fi

  if curl -sf "$API_URL/readyz" >/dev/null 2>&1; then
    curl -sf "$API_URL/readyz" | jq '{ready,version,checks}'
  else
    echo "dev_status=readyz unreachable url=$API_URL/readyz"
  fi
}

stop_pidfile() {
  local f=$1 label=$2
  if ! pid_alive "$f"; then
    rm -f "$f"
    echo "dev_stop=$label not_running"
    return 0
  fi
  local pid
  pid=$(cat "$f")
  kill "$pid" 2>/dev/null || true
  for _ in $(seq 1 40); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.25
  done
  if kill -0 "$pid" 2>/dev/null; then
    kill -TERM "$pid" 2>/dev/null || true
    sleep 1
  fi
  if kill -0 "$pid" 2>/dev/null; then
    echo "dev_warn=$label pid=$pid still_running" >&2
  fi
  rm -f "$f"
  echo "dev_stop=$label pid=$pid"
}

cmd_stop() {
  ensure_layout
  if [[ -f "$ENV_FILE" ]]; then
    set -a
    # shellcheck disable=SC1090
    source "$ENV_FILE"
    set +a
  fi
  stop_pidfile "$VITE_PID" vite
  stop_pidfile "$SERVER_PID" server
  local api_port=${MUSEHUB_BIND:-127.0.0.1:8080}
  api_port=${api_port##*:}
  api_port="${api_port:-8080}"
  local vite_port=5173
  for _ in $(seq 1 40); do
    if ! port_busy "$api_port" && ! port_busy "$vite_port"; then
      break
    fi
    sleep 0.25
  done
  echo "dev_stop=data_preserved state_dir=$STATE_DIR"
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
