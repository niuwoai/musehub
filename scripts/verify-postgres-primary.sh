#!/usr/bin/env bash
# Post-failover check for an external PostgreSQL HA endpoint and API instance.
set -euo pipefail

DATABASE_URL="${MUSEHUB_DATABASE_URL:?set MUSEHUB_DATABASE_URL to the HA writer endpoint}"
OLD_PRIMARY_URL="${MUSEHUB_HA_OLD_PRIMARY_DATABASE_URL:?set a direct connection to the former primary}"
API_BASES_RAW="${MUSEHUB_HA_API_BASES:-${MUSEHUB_HA_API_BASE:?set MUSEHUB_HA_API_BASES or MUSEHUB_HA_API_BASE}}"
IFS=',' read -r -a API_BASES <<< "$API_BASES_RAW"

command -v psql >/dev/null 2>&1 || { echo "psql is required" >&2; exit 1; }
command -v curl >/dev/null 2>&1 || { echo "curl is required" >&2; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "jq is required" >&2; exit 1; }

not_writable="$(psql "$DATABASE_URL" -X -A -t -v ON_ERROR_STOP=1 \
  -c "SELECT pg_is_in_recovery() OR current_setting('transaction_read_only')::boolean" \
  | tr -d '[:space:]')"
[[ "$not_writable" == "f" ]] || {
  echo "database writer endpoint is not a writable primary (read_only=$not_writable)" >&2
  exit 1
}

old_primary_writable="$(psql "$OLD_PRIMARY_URL" -X -A -t -v ON_ERROR_STOP=1 \
  -c "SELECT NOT pg_is_in_recovery() AND NOT current_setting('transaction_read_only')::boolean" \
  | tr -d '[:space:]')"
[[ "$old_primary_writable" == "f" ]] || {
  echo "former primary is still writable or could not be verified (writable=$old_primary_writable)" >&2
  exit 1
}

expected_version=""
for api_base in "${API_BASES[@]}"; do
  api_base="${api_base%/}"
  [[ -n "$api_base" ]] || { echo "API base list contains an empty entry" >&2; exit 1; }
  ready_response="$(curl -sS -w '\n%{http_code}' "$api_base/readyz")"
  http_status="${ready_response##*$'\n'}"
  ready_body="${ready_response%$'\n'*}"
  [[ "$http_status" == "200" ]] || {
    echo "API readiness returned HTTP $http_status for $api_base" >&2
    exit 1
  }
  printf '%s' "$ready_body" | jq -e '.ready == true and .checks.database_role == "primary"' >/dev/null || {
    echo "API is not ready against a PostgreSQL primary: $api_base" >&2
    exit 1
  }

  version_body="$(curl -fsS "$api_base/version" | jq -er '.version')"
  version_header="$(curl -fsS -D - -o /dev/null "$api_base/version" \
    | tr -d '\r' \
    | awk 'tolower($1) == "x-app-version:" { print $2; exit }')"
  [[ -n "$version_header" && "$version_body" == "$version_header" ]] || {
    echo "API version endpoint and X-App-Version header disagree for $api_base" >&2
    exit 1
  }
  if [[ -z "$expected_version" ]]; then
    expected_version="$version_body"
  elif [[ "$version_body" != "$expected_version" ]]; then
    echo "API instances report different versions" >&2
    exit 1
  fi
done
printf 'postgres_primary_failover_check=ok version=%s api_instances=%s former_primary_writable=false\n' \
  "$expected_version" "${#API_BASES[@]}"
