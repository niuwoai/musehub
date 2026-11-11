#!/usr/bin/env bash
# PID slot helpers: identity check without killing unrelated processes (Linux / macOS).

DEV_LOCAL_START_EPOCH_TOLERANCE_SEC="${DEV_LOCAL_START_EPOCH_TOLERANCE_SEC:-120}"

dev_local_process_cmdline() {
  local pid=$1
  if [[ ! "$pid" =~ ^[0-9]+$ ]]; then
    return 1
  fi
  if [[ -r "/proc/${pid}/cmdline" ]]; then
    tr '\0' ' ' <"/proc/${pid}/cmdline"
    return 0
  fi
  if command -v ps >/dev/null 2>&1; then
    ps -p "$pid" -o command= 2>/dev/null | sed 's/^[[:space:]]*//'
    return 0
  fi
  echo "dev_error=process_cmdline_unsupported need /proc or ps" >&2
  return 1
}

dev_local_process_start_epoch() {
  local pid=$1
  if [[ ! "$pid" =~ ^[0-9]+$ ]]; then
    return 1
  fi
  if [[ -r "/proc/${pid}/stat" ]] && [[ -r /proc/stat ]]; then
    local starttime btime clk epoch
    starttime=$(awk '{print $22}' "/proc/${pid}/stat")
    btime=$(awk '/^btime / {print $2; exit}' /proc/stat)
    clk=$(getconf CLK_TCK 2>/dev/null || echo 100)
    epoch=$((btime + starttime / clk))
    printf '%s\n' "$epoch"
    return 0
  fi
  if command -v ps >/dev/null 2>&1; then
    local lstart epoch
    lstart=$(ps -p "$pid" -o lstart= 2>/dev/null | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
    if [[ -n "$lstart" ]]; then
      if epoch=$(date -j -f "%a %b %d %T %Y" "$lstart" "+%s" 2>/dev/null); then
        printf '%s\n' "$epoch"
        return 0
      fi
      if epoch=$(date -d "$lstart" "+%s" 2>/dev/null); then
        printf '%s\n' "$epoch"
        return 0
      fi
    fi
  fi
  return 1
}

dev_local_marker_field() {
  local marker=$1 key=$2
  [[ -f "$marker" ]] || return 1
  grep -E "^${key}=" "$marker" 2>/dev/null | head -n1 | cut -d= -f2-
}

dev_local_epoch_near() {
  local a=$1 b=$2 tol=${3:-$DEV_LOCAL_START_EPOCH_TOLERANCE_SEC}
  [[ "$a" =~ ^[0-9]+$ && "$b" =~ ^[0-9]+$ ]] || return 1
  local d=$((a > b ? a - b : b - a))
  [[ "$d" -le "$tol" ]]
}

dev_local_slot_identity_ok() {
  local pid=$1 marker=$2 pattern=$3
  local cmd
  cmd=$(dev_local_process_cmdline "$pid") || return 1
  [[ "$cmd" == *"$pattern"* ]] || return 1

  local marker_pid marker_pattern exec_hint started_epoch proc_epoch
  marker_pid=$(dev_local_marker_field "$marker" pid) || true
  marker_pattern=$(dev_local_marker_field "$marker" pattern) || true
  exec_hint=$(dev_local_marker_field "$marker" exec_hint) || true
  started_epoch=$(dev_local_marker_field "$marker" started_epoch) || true

  if [[ -n "$marker_pid" && "$marker_pid" != "$pid" ]]; then
    return 1
  fi
  if [[ -n "$marker_pattern" && "$marker_pattern" != "$pattern" ]]; then
    return 1
  fi
  if [[ -n "$exec_hint" && "$cmd" != *"$(basename "$exec_hint")"* && "$cmd" != *"$exec_hint"* ]]; then
    return 1
  fi
  if [[ -n "$started_epoch" ]]; then
    proc_epoch=$(dev_local_process_start_epoch "$pid") || return 1
    dev_local_epoch_near "$started_epoch" "$proc_epoch" || return 1
  fi
  return 0
}

dev_local_slot_occupied() {
  local pidfile=$1 marker=$2 pattern=$3
  [[ -f "$pidfile" ]] || return 1
  local pid
  pid=$(tr -d '[:space:]' <"$pidfile")
  [[ "$pid" =~ ^[0-9]+$ ]] || return 1
  if ! kill -0 "$pid" 2>/dev/null; then
    return 1
  fi
  dev_local_slot_identity_ok "$pid" "$marker" "$pattern"
}

dev_local_slot_claim() {
  local pidfile=$1 marker=$2 pid=$3 pattern=$4 exec_hint=${5:-}
  local started_epoch
  started_epoch=$(dev_local_process_start_epoch "$pid") || started_epoch=$(date -u +%s)
  printf '%s\n' "$pid" >"$pidfile"
  {
    printf 'pid=%s\n' "$pid"
    printf 'pattern=%s\n' "$pattern"
    printf 'started_utc=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf 'started_epoch=%s\n' "$started_epoch"
    if [[ -n "$exec_hint" ]]; then
      printf 'exec_hint=%s\n' "$exec_hint"
    fi
  } >"$marker"
}

dev_local_slot_clear_if_dead() {
  local pidfile=$1 marker=$2 pattern=$3
  [[ -f "$pidfile" ]] || return 0
  local pid
  pid=$(tr -d '[:space:]' <"$pidfile")
  if kill -0 "$pid" 2>/dev/null; then
    return 0
  fi
  rm -f "$pidfile" "$marker"
}

dev_local_slot_stop() {
  local pidfile=$1 marker=$2 pattern=$3 label=$4
  if ! [[ -f "$pidfile" ]]; then
    echo "dev_stop=$label not_running"
    return 0
  fi
  local pid
  pid=$(tr -d '[:space:]' <"$pidfile")
  if ! kill -0 "$pid" 2>/dev/null; then
    rm -f "$pidfile" "$marker"
    echo "dev_stop=$label not_running stale_pidfile"
    return 0
  fi
  if ! dev_local_slot_identity_ok "$pid" "$marker" "$pattern"; then
    local cmd
    cmd=$(dev_local_process_cmdline "$pid") || true
    echo "dev_error=pidfile_identity_mismatch label=$label pid=$pid" >&2
    echo "dev_error=expected_pattern=$pattern" >&2
    echo "dev_hint=observed_cmd=${cmd:-unknown}" >&2
    echo "dev_hint=not_killing_foreign_process; fix or remove $pidfile manually" >&2
    return 1
  fi
  kill "$pid" 2>/dev/null || true
  local i
  for i in $(seq 1 40); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.25
  done
  if kill -0 "$pid" 2>/dev/null; then
    kill -TERM "$pid" 2>/dev/null || true
    sleep 1
  fi
  if kill -0 "$pid" 2>/dev/null; then
    echo "dev_error=stop_failed label=$label pid=$pid" >&2
    echo "dev_hint=pidfile retained at $pidfile" >&2
    return 1
  fi
  rm -f "$pidfile" "$marker"
  echo "dev_stop=$label pid=$pid"
  return 0
}
