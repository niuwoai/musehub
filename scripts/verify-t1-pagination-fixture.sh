#!/usr/bin/env bash
# Build and verify the deterministic T1 pagination fixture under /tmp.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FIXTURE_DIR="$ROOT/scripts/fixtures/t1-pagination"
WORK_DIR="$(mktemp -d /tmp/musehub-t1-pagination.XXXXXX)"

for command in git node npm; do
  command -v "$command" >/dev/null 2>&1 || {
    printf 'required command is missing: %s\n' "$command" >&2
    exit 2
  }
done

cp "$FIXTURE_DIR/package.json" "$FIXTURE_DIR/paginate.js" \
  "$FIXTURE_DIR/paginate.test.js" "$WORK_DIR/"
git -C "$WORK_DIR" -c init.defaultBranch=main init -q
git -C "$WORK_DIR" config user.name 'MuseHub T1 Fixture'
git -C "$WORK_DIR" config user.email 't1-fixture@musehub.test'
git -C "$WORK_DIR" add package.json paginate.js paginate.test.js
GIT_AUTHOR_DATE='2026-01-01T00:00:00Z' GIT_COMMITTER_DATE='2026-01-01T00:00:00Z' \
  git -C "$WORK_DIR" commit -q -m 'T1 shared pagination baseline' --no-gpg-sign
BASE_OID="$(git -C "$WORK_DIR" rev-parse HEAD)"

if npm --prefix "$WORK_DIR" test >"$WORK_DIR/baseline-test.log" 2>&1; then
  echo 'T1 baseline unexpectedly passed; expected all three regressions to fail' >&2
  exit 1
fi
FAILED_TESTS="$(awk '$2 == "fail" { print $3 }' "$WORK_DIR/baseline-test.log")"
[[ "$FAILED_TESTS" == 3 ]] || {
  printf 'T1 baseline expected 3 failing tests, observed %s\n' "${FAILED_TESTS:-0}" >&2
  rg '^(ℹ fail |✖ )' "$WORK_DIR/baseline-test.log" >&2 || true
  exit 1
}

git -C "$WORK_DIR" switch -q -c t1/pagination-fix
git -C "$WORK_DIR" apply "$FIXTURE_DIR/fix.patch"
git -C "$WORK_DIR" add paginate.js
GIT_AUTHOR_DATE='2026-01-01T00:01:00Z' GIT_COMMITTER_DATE='2026-01-01T00:01:00Z' \
  git -C "$WORK_DIR" commit -q -m 'Fix 1-based pagination boundary' --no-gpg-sign
FIX_OID="$(git -C "$WORK_DIR" rev-parse HEAD)"
npm --prefix "$WORK_DIR" test

printf 't1_fixture=ok\n'
printf 't1_fixture_path=%s\n' "$WORK_DIR"
printf 't1_base_commit=%s\n' "$BASE_OID"
printf 't1_fix_commit=%s\n' "$FIX_OID"
printf 't1_baseline_failing_tests=%s\n' "$FAILED_TESTS"
