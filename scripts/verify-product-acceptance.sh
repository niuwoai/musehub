#!/usr/bin/env bash
# Product acceptance: migrations up → schema check → product_slice → down → up (PostgreSQL).
set -euo pipefail
cd "$(dirname "$0")/.."

DATABASE_URL="${MUSEHUB_TEST_DATABASE_URL:-${DATABASE_URL:-}}"
if [[ -z "$DATABASE_URL" ]]; then
  echo "set MUSEHUB_TEST_DATABASE_URL or DATABASE_URL" >&2
  exit 1
fi

export MUSEHUB_GIT_ROOT="${MUSEHUB_GIT_ROOT:-/tmp/musehub-acceptance-git}"
export MUSEHUB_SESSION_SECRET="${MUSEHUB_SESSION_SECRET:-acceptance-session-secret-32chars}"
export MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD="${MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD:-Acceptance-Bootstrap-Strong-1!}"
export MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD
mkdir -p "$MUSEHUB_GIT_ROOT"
export MUSEHUB_TEST_DATABASE_URL="$DATABASE_URL"
# Test/acceptance only: release idle pool connections that block 0001 down teardown.
export MUSEHUB_MIGRATE_TERMINATE_IDLE_PEERS=1

cargo build -p musehub-agent-adapter --bin musehub-agent-reference -q
cargo build -p musehub-agent-adapter --bin musehub-agent-codex-sim -q
cargo build -p musehub-agent-adapter --bin musehub-pipeline-reference -q
cargo build -p musehub-agent-adapter --bin musehub-compose-mock -q
cargo build -p musehub-agent-adapter --bin musehub-compose-runner -q
cargo build -p musehub-git-ssh -q
if command -v sudo >/dev/null && [[ -x target/debug/musehub-git-ssh ]]; then
  sudo install -m 0755 target/debug/musehub-git-ssh /usr/local/bin/musehub-git-ssh
  if command -v setcap >/dev/null; then
    sudo setcap cap_setgid+ep /usr/local/bin/musehub-git-ssh
  fi
fi
export MUSEHUB_GIT_SSH_BIN="${MUSEHUB_GIT_SSH_BIN:-$PWD/target/debug/musehub-git-ssh}"
export MUSEHUB_REQUIRE_GIT_SSH="${MUSEHUB_REQUIRE_GIT_SSH:-1}"
if command -v sudo >/dev/null; then
  sudo install -d -m 755 /run/sshd 2>/dev/null || true
fi
if ! id git >/dev/null 2>&1; then
  if command -v useradd >/dev/null; then
  sudo useradd -r -M -s /bin/false git 2>/dev/null || true
  fi
fi
if id git >/dev/null 2>&1 && command -v sudo >/dev/null; then
  sudo passwd -d git 2>/dev/null || true
  sudo usermod -s /bin/false git 2>/dev/null || true
fi
getent group musehub-git >/dev/null 2>&1 || sudo groupadd -r musehub-git 2>/dev/null || true
if command -v sudo >/dev/null; then
  sudo usermod -aG musehub-git git 2>/dev/null || true
  sudo usermod -aG musehub-git "$(whoami)" 2>/dev/null || true
fi
export MUSEHUB_GIT_STORAGE_GROUP="${MUSEHUB_GIT_STORAGE_GROUP:-musehub-git}"
export MUSEHUB_GIT_STORAGE_SUDO_FALLBACK="${MUSEHUB_GIT_STORAGE_SUDO_FALLBACK:-1}"
export MUSEHUB_TEST_GIT_SSH_TIMEOUT_SEC="${MUSEHUB_TEST_GIT_SSH_TIMEOUT_SEC:-240}"
if ! test -x /usr/sbin/sshd && ! command -v sshd >/dev/null 2>&1; then
  echo "git_ssh=fail openssh-server (sshd) is required for product acceptance (install in CI setup)" >&2
  exit 1
fi
if [[ ! -x "$MUSEHUB_GIT_SSH_BIN" ]]; then
  echo "git_ssh=fail missing executable $MUSEHUB_GIT_SSH_BIN" >&2
  exit 1
fi

echo "=== migrate reset (down) ==="
cargo run -p musehub-migrate -- down migrations "$DATABASE_URL"

echo "=== migrate up ==="
cargo run -p musehub-migrate -- up migrations "$DATABASE_URL"

SUITE_TIMEOUT_SEC="${SUITE_TIMEOUT_SEC:-900}"
command -v /usr/bin/timeout >/dev/null || { echo "/usr/bin/timeout required" >&2; exit 1; }

echo "=== product integration tests (suite timeout=${SUITE_TIMEOUT_SEC}s) ==="
for suite in product_slice security_db agent_cancel_e2e protected_branch_git auth_matrix plan_immutable pr_gates_e2e pr_security_e2e agent_budget pat_scope pipeline_delivery_e2e pipeline_security_e2e pipeline_runner_pat_e2e pipeline_cancel_e2e deployment_idempotency_e2e deployment_conflict_e2e startup_reconcile_e2e lease_reconcile_e2e worker_claim_e2e multi_instance_reconcile_e2e stale_worker_fencing_e2e s1_collaboration_http_e2e s1_pr_comment_sync_race_e2e ssh_keys_injection_e2e git_ssh_transport_e2e git_ssh_security_matrix_e2e git_ssh_packaging_secrets_e2e git_ssh_run_dir_permissions_e2e git_ssh_http_ssh_alternating_e2e git_ssh_legacy_storage_migration_e2e git_ssh_storage_symlink_e2e git_ssh_home_hardening_e2e git_browse_limits_e2e; do
  echo "--- musehub-app $suite (start $(date -u +%H:%M:%S)) ---"
  /usr/bin/timeout -k 30 "${SUITE_TIMEOUT_SEC}" \
    cargo test -p musehub-app --test "$suite" -- --test-threads=1
  echo "--- musehub-app $suite ok (end $(date -u +%H:%M:%S)) ---"
done

echo "=== migrate down ==="
cargo run -p musehub-migrate -- down migrations "$DATABASE_URL"

echo "=== migrate up (again) ==="
cargo run -p musehub-migrate -- up migrations "$DATABASE_URL"

echo "=== migrate integration (full up/down + idle peer) ==="
/usr/bin/timeout -k 30 600 cargo test -p musehub-migrate --test full_up_down -- --test-threads=1

echo "=== caddy gateway layout ==="
bash scripts/verify-caddyfile-layout.sh

echo "acceptance=ok"
