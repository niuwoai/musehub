#!/usr/bin/env bash
# Mirrors rust-portable job “product security integration tests” (local VM, no GitHub).
set -euo pipefail
cd "$(dirname "$0")/.."
export DATABASE_URL="${MUSEHUB_TEST_DATABASE_URL:-${DATABASE_URL:?}}"
export MUSEHUB_TEST_DATABASE_URL="$DATABASE_URL"
export MUSEHUB_GIT_ROOT="${MUSEHUB_GIT_ROOT:-/tmp/musehub-ci-git}"
export MUSEHUB_ARTIFACT_ROOT="${MUSEHUB_ARTIFACT_ROOT:-/tmp/musehub-ci-artifacts}"
export MUSEHUB_SESSION_SECRET="${MUSEHUB_SESSION_SECRET:-ci-session-secret-not-for-production}"
export MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD="${MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD:-Ci-Bootstrap-Strong!2026}"
mkdir -p "$MUSEHUB_GIT_ROOT" "$MUSEHUB_ARTIFACT_ROOT"
_codex_sim="$PWD/target/debug/musehub-agent-codex-sim"
_codex_fix="$PWD/target/debug/musehub-agent-codex-fixture"

_one() { cargo test -p musehub-app --test "$1" -- --test-threads=1; }

_one security_db
_one agent_cancel_e2e
_one protected_branch_git
_one auth_matrix
_one plan_immutable
_one pr_gates_e2e
_one pr_security_e2e
_one agent_budget
_one pat_scope
_one pipeline_delivery_e2e
_one pipeline_security_e2e
_one pipeline_runner_pat_e2e
_one pipeline_cancel_e2e
_one pipeline_oci_negative_e2e
MUSEHUB_MCP_PRODUCT=1 _one production_mcp_e2e
MUSEHUB_MCP_PRODUCT=1 _one production_mcp_security_e2e
MUSEHUB_MCP_PRODUCT=1 _one production_mcp_matrix
MUSEHUB_MCP_PRODUCT=1 _one production_mcp_auth_http_e2e
MUSEHUB_MCP_PRODUCT=1 _one production_mcp_validation_e2e
MUSEHUB_MCP_PRODUCT=1 _one production_mcp_write_matrix_e2e
MUSEHUB_MCP_PRODUCT=1 _one production_mcp_resources_e2e
MUSEHUB_MCP_PRODUCT=1 _one production_mcp_guards_e2e
MUSEHUB_MCP_PRODUCT=1 _one production_mcp_cross_tenant_resources_e2e
MUSEHUB_MCP_PRODUCT=1 _one production_mcp_selected_repo_e2e
MUSEHUB_MCP_PRODUCT=1 _one grants_security_e2e
MUSEHUB_MCP_PRODUCT=1 _one repository_grants_e2e
_one runner_daemon_acceptance_e2e
_one runner_recovery_e2e
_one runner_reclaim_build_e2e
_one remote_pipeline_runner_e2e
env "CARGO_BIN_EXE_musehub-agent-codex-sim=${_codex_sim}" \
  cargo test -p musehub-app --test agent_codex_mainline_select_e2e -- --test-threads=1
env "CARGO_BIN_EXE_musehub-agent-codex-fixture=${_codex_fix}" \
  cargo test -p musehub-app --test agent_codex_fixture_e2e -- --test-threads=1
_one codex_adapter_e2e
cargo test -p musehub-mcp-probe --test matrix python_mcp_sdk -- --test-threads=1
_one deployment_rollback_runner_e2e
_one deployment_rollback_negative_e2e

echo "ci_security_integration_local=ok"
