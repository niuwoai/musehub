#!/usr/bin/env bash
# PID slot helpers: identity check without killing unrelated processes (Linux / macOS).

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

dev_local_marker_pid() {
  local marker=$1
  [[ -f "$marker" ]] || return 1
  grep -E '^pid=[0-9]+$' "$marker" 2>/dev/null | head -n1 | cut -d= -f2
}

dev_local_marker_pattern() {
  local marker=$1
  [[ -f "$marker" ]] || return 1
  grep -E '^pattern=' "$marker" 2>/dev/null | head -n1 | cut -d= -f2-
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
  local marker_pid marker_pattern
  marker_pid=$(dev_local_marker_pid "$marker") || true
  marker_pattern=$(dev_local_marker_pattern "$marker") || true
  if [[ -n "$marker_pid" && "$marker_pid" != "$pid" ]]; then
    return 1
  fi
  if [[ -n "$marker_pattern" && "$marker_pattern" != "$pattern" ]]; then
    return 1
  fi
  local cmd
  cmd=$(dev_local_process_cmdline "$pid") || return 1
  [[ "$cmd" == *"$pattern"* ]]
}

dev_local_slot_claim() {
  local pidfile=$1 marker=$2 pid=$3 pattern=$4
  printf '%s\n' "$pid" >"$pidfile"
  {
    printf 'pid=%s\n' "$pid"
    printf 'pattern=%s\n' "$pattern"
    printf 'started_utc=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  } >"$marker"
}

dev_local_slot_clear_if_dead() {
  local pidfile=$1 marker=$2 pattern=$3
  [[ -f "$pidfile" ]] || return 0
  local pid
  pid=$(tr -d '[:space:]' <"$pidfile")
  if kill -0 "$pid" 2>/dev/null; then
  # Foreign or owned slots stay until stop/identity handling; never drop a live pid.
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
  local cmd
  cmd=$(dev_local_process_cmdline "$pid") || true
  if [[ "$cmd" != *"$pattern"* ]]; then
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
