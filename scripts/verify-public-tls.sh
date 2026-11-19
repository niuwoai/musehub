#!/usr/bin/env bash
# Verify a publicly reachable MuseHub HTTPS gateway and its TLS/DNS behavior.
set -euo pipefail

DOMAIN="${MUSEHUB_PUBLIC_TLS_DOMAIN:-}"
HTTP_PORT="${MUSEHUB_PUBLIC_TLS_HTTP_PORT:-80}"
HTTPS_PORT="${MUSEHUB_PUBLIC_TLS_HTTPS_PORT:-443}"
EXPECTED_VERSION="${MUSEHUB_PUBLIC_TLS_EXPECTED_VERSION:-}"

if [[ -z "$DOMAIN" ]]; then
  echo "set MUSEHUB_PUBLIC_TLS_DOMAIN to the public gateway hostname" >&2
  exit 2
fi
if [[ ! "$DOMAIN" =~ ^[A-Za-z0-9.-]+$ || "$DOMAIN" == .* || "$DOMAIN" == *..* ]]; then
  echo "MUSEHUB_PUBLIC_TLS_DOMAIN must be a hostname without scheme, path, or port" >&2
  exit 2
fi
for port in "$HTTP_PORT" "$HTTPS_PORT"; do
  if [[ ! "$port" =~ ^[0-9]+$ ]]; then
    echo "HTTP and HTTPS ports must be integers from 1 through 65535" >&2
    exit 2
  fi
  port_number=$((10#$port))
  if (( port_number < 1 || port_number > 65535 )); then
    echo "HTTP and HTTPS ports must be integers from 1 through 65535" >&2
    exit 2
  fi
done
for tool in curl getent jq; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "required command is missing: $tool" >&2
    exit 2
  }
done

if ! DNS_RESULTS="$(getent ahosts "$DOMAIN" 2>/dev/null)" || [[ -z "$DNS_RESULTS" ]]; then
  echo "public_tls_dns=fail domain=$DOMAIN" >&2
  exit 1
fi
DNS_ADDRESS_COUNT="$(printf '%s\n' "$DNS_RESULTS" | awk '{print $1}' | sort -u | wc -l | tr -d '[:space:]')"
echo "public_tls_dns=ok domain=$DOMAIN resolved_addresses=$DNS_ADDRESS_COUNT"

HTTP_ORIGIN="http://${DOMAIN}:${HTTP_PORT}"
HTTPS_ORIGIN="https://${DOMAIN}:${HTTPS_PORT}"
if ! HTTP_RESULT="$(curl --silent --show-error --max-time 20 --output /dev/null --write-out '%{http_code}\n%{redirect_url}' "${HTTP_ORIGIN}/readyz")"; then
  echo "public_tls_http=fail domain=$DOMAIN" >&2
  exit 1
fi
HTTP_STATUS="${HTTP_RESULT%%$'\n'*}"
REDIRECT_URL="${HTTP_RESULT#*$'\n'}"
if [[ ! "$HTTP_STATUS" =~ ^3[0-9][0-9]$ ]]; then
  echo "public_tls_http_redirect=fail status=$HTTP_STATUS" >&2
  exit 1
fi
case "$REDIRECT_URL" in
  "https://${DOMAIN}/"*|"https://${DOMAIN}:${HTTPS_PORT}/"*) ;;
  *)
    echo "public_tls_http_redirect=fail location=$REDIRECT_URL" >&2
    exit 1
    ;;
esac
echo "public_tls_http_redirect=ok status=$HTTP_STATUS"

READY_BODY="$(curl --fail --silent --show-error --max-time 30 "${HTTPS_ORIGIN}/readyz")" || {
  echo "public_tls_https_readyz=fail domain=$DOMAIN" >&2
  exit 1
}
if ! jq -e '.ready == true' >/dev/null <<<"$READY_BODY"; then
  echo "public_tls_https_readyz=fail body_not_ready" >&2
  exit 1
fi
BODY_VERSION="$(jq -er '.version | strings' <<<"$READY_BODY")" || {
  echo "public_tls_version=fail missing_version" >&2
  exit 1
}

VERSION_HEADERS="$(curl --fail --silent --show-error --max-time 20 --dump-header - --output /dev/null "${HTTPS_ORIGIN}/version")" || {
  echo "public_tls_https_version=fail domain=$DOMAIN" >&2
  exit 1
}
HEADER_VERSION="$(printf '%s\n' "$VERSION_HEADERS" | awk 'tolower($1) == "x-app-version:" {gsub("\\r", "", $2); version=$2} END {print version}')"
VERSION_BODY="$(curl --fail --silent --show-error --max-time 20 "${HTTPS_ORIGIN}/version" | jq -er '.version | strings')" || {
  echo "public_tls_https_version=fail invalid_version_body" >&2
  exit 1
}
if [[ -z "$HEADER_VERSION" || "$HEADER_VERSION" != "$BODY_VERSION" || "$VERSION_BODY" != "$BODY_VERSION" ]]; then
  echo "public_tls_version=fail ready=$BODY_VERSION header=${HEADER_VERSION:-missing} endpoint=$VERSION_BODY" >&2
  exit 1
fi
if [[ -n "$EXPECTED_VERSION" && "$BODY_VERSION" != "$EXPECTED_VERSION" ]]; then
  echo "public_tls_version=fail expected=$EXPECTED_VERSION actual=$BODY_VERSION" >&2
  exit 1
fi

echo "public_tls_certificate_chain_and_hostname=ok"
echo "public_tls_https_readyz=ok"
echo "public_tls_version=ok version=$BODY_VERSION"
echo "public_tls_acceptance=ok domain=$DOMAIN"
