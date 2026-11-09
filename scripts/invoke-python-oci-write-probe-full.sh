#!/usr/bin/env bash
# Run mcp_product_oci_write_probe.py in **full** mode (pipeline_run + deployment_start).
# Caller must export: MCP_URL API_BASE BEARER ORG_ID REPO_ID ENV_ID RELEASE_ID TARGET_OID ORG_SLUG REPO_SLUG
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"
PYDEPS="${MUSEHUB_MCP_PYDEPS:-$ROOT/target/mcp-matrix-pydeps}"
SCRIPT="$ROOT/crates/musehub-app/python/mcp_product_oci_write_probe.py"
: "${MCP_URL:?}" "${API_BASE:?}" "${BEARER:?}" "${ORG_ID:?}" "${REPO_ID:?}" "${ENV_ID:?}" "${RELEASE_ID:?}" "${TARGET_OID:?}"
ORG_SLUG="${ORG_SLUG:-demo}"
REPO_SLUG="${REPO_SLUG:?}"
chmod +x "$SCRIPT" 2>/dev/null || true
PYTHONPATH="$PYDEPS" python3 "$SCRIPT" \
  --url "$MCP_URL" \
  --api-base "$API_BASE" \
  --bearer "$BEARER" \
  --org-slug "$ORG_SLUG" \
  --repo-slug "$REPO_SLUG" \
  --org-id "$ORG_ID" \
  --repository-id "$REPO_ID" \
  --environment-id "$ENV_ID" \
  --release-id "$RELEASE_ID" \
  --target-oid "$TARGET_OID" \
  --mode full
