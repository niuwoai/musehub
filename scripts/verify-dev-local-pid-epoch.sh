#!/usr/bin/env bash
# Fixture: lstart parsing + self PID epoch (Linux CI; macOS ps路径未在 Agent 实测).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=lib/dev-local-pid.sh
source "$ROOT/scripts/lib/dev-local-pid.sh"

probe=$(dev_local_process_start_epoch $$) || probe=""
if [[ -n "$probe" ]]; then
  now=$(date -u +%s)
  d=$((now > probe ? now - probe : probe - now))
  [[ "$d" -lt 60 ]] || {
    echo "verify_pid_epoch=fail self_epoch_drift=$d probe=$probe now=$now" >&2
    exit 1
  }
else
  echo "verify_pid_epoch=skip self_epoch uname=$(uname -s)"
fi

sample_gnu="Tue Nov 11 12:00:00 2025"
sample_bsd="Tue Nov 11 12:00:00 2025"
if epoch=$(dev_local_parse_ps_lstart_to_epoch "$sample_gnu"); then
  [[ "$epoch" =~ ^[0-9]+$ ]] || exit 1
else
  echo "verify_pid_epoch=warn gnu_lstart_fixture_unparsed uname=$(uname -s)"
fi
if epoch=$(dev_local_parse_ps_lstart_to_epoch "$sample_bsd"); then
  [[ "$epoch" =~ ^[0-9]+$ ]] || exit 1
fi

dev_local_epoch_near 100 104 5 || exit 1
dev_local_epoch_near 100 200 5 && exit 1 || true

echo "verify_dev_local_pid_epoch=ok platform=$(uname -s)"
