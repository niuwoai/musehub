#!/usr/bin/env bash
# Redacted diagnostics for browser E2E failures (no session cookies / passwords / PAT).
set -euo pipefail
cd "$(dirname "$0")/.."

DATABASE_URL="${DATABASE_URL:-${MUSEHUB_TEST_DATABASE_URL:-}}"
API="${MUSEHUB_E2E_API:-http://127.0.0.1:8080}"
LOG="${MUSEHUB_E2E_SERVER_LOG:-/tmp/musehub-e2e-server.log}"

redact() {
  sed -E \
    -e 's/(Set-Cookie:|Cookie:)[^[:space:]]+/REDACTED_COOKIE/gi' \
    -e 's/(password|mhp_[A-Za-z0-9]+|"password"[^,]*,[^,]*)[^[:print:]]*/\1":"REDACTED"/gi' \
    -e 's/postgres:\/\/[^@]+@/postgres:\/\/REDACTED@/g'
}

{
  echo "=== e2e_diagnostics ts=$(date -u +%Y-%m-%dT%H:%M:%SZ) ==="
  if [[ -f "$LOG" ]]; then
    echo "--- server log (redacted, last 80 lines) ---"
    tail -n 80 "$LOG" | redact
  else
    echo "server_log=missing path=$LOG"
  fi
  if [[ -n "$DATABASE_URL" ]]; then
    echo "--- recent agent_runs (error_code, state) ---"
    psql "$DATABASE_URL" -At -c \
      "SELECT id::text, state, COALESCE(error_code,'') FROM agent_runs ORDER BY updated_at DESC LIMIT 5" \
      2>/dev/null | redact || echo "agent_runs_query_failed"
  fi
  if curl -sf "$API/healthz" >/dev/null 2>&1; then
  echo "--- readyz ---"
  curl -sf "$API/readyz" | redact || true
  fi
} >&2
