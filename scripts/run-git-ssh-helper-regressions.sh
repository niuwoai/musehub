#!/usr/bin/env bash
# Lightweight regression guards for SSH/git test helpers and compose dc timeout quoting.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build -p musehub-git-ssh -q 2>/dev/null || true
cargo test -p musehub-app --test git_ssh_util_regression -- --test-threads=1 --nocapture
chmod +x ./scripts/test/verify-compose-dc-timeout.regression.sh
./scripts/test/verify-compose-dc-timeout.regression.sh
echo "git_ssh_helper_regressions=ok"
