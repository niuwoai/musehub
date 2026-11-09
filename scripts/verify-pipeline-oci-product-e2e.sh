#!/usr/bin/env bash
# Product path: local registry → pipeline OCI build → publish → compose-runner deploy (API).
set -euo pipefail
cd "$(dirname "$0")/.."

command -v docker >/dev/null || { echo "pipeline_oci_product=blocked no docker" >&2; exit 1; }
docker info >/dev/null 2>&1 || { echo "pipeline_oci_product=blocked docker info" >&2; exit 1; }

chmod +x ./scripts/start-ci-registry.sh
./scripts/start-ci-registry.sh

export MUSEHUB_OCI_REGISTRY="${MUSEHUB_OCI_REGISTRY:-127.0.0.1:5000}"
export MUSEHUB_REQUIRE_PIPELINE_OCI=1
export MUSEHUB_COMPOSE_ADAPTER_MODE=runner
export DATABASE_URL="${MUSEHUB_TEST_DATABASE_URL:-${DATABASE_URL:-postgres://musehub:musehub@localhost:5432/musehub}}"

cargo build -p musehub-agent-adapter --bin musehub-agent-reference -q
cargo build -p musehub-agent-adapter --bin musehub-pipeline-oci -q
cargo build -p musehub-agent-adapter --bin musehub-compose-runner -q
cargo build -p musehub-agent-adapter --bin musehub-compose-mock -q
BIN_OCI="$PWD/target/debug/musehub-pipeline-oci"

echo "=== two source oids → distinct image digests (build/push/pull) ==="
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
git init --bare "$TMP/bare.git" >/dev/null 2>&1
WORKDIR=$(mktemp -d)
git -C "$WORKDIR" init -q -b main
git -C "$WORKDIR" config user.email ci@musehub.test
git -C "$WORKDIR" config user.name ci
echo v1 >"$WORKDIR/README"
git -C "$WORKDIR" add README && git -C "$WORKDIR" commit -q -m v1
OID1=$(git -C "$WORKDIR" rev-parse HEAD)
git -C "$WORKDIR" push -q "$TMP/bare.git" HEAD:refs/heads/main
build_oid() {
  local oid=$1
  local out run_id repo_id
  out=$(mktemp -d)
  run_id="$(cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen)"
  repo_id="$(cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen)"
  if ! MUSEHUB_PIPELINE_SOURCE_OID="$oid" \
    MUSEHUB_PIPELINE_BARE_GIT="$TMP/bare.git" \
    MUSEHUB_PIPELINE_ARTIFACT_PATH="$out/bundle.tar" \
    MUSEHUB_PIPELINE_RUN_KIND=release_build \
    MUSEHUB_PIPELINE_RUN_ID="$run_id" \
    MUSEHUB_PIPELINE_REPO_ID="$repo_id" \
    MUSEHUB_OCI_REGISTRY="$MUSEHUB_OCI_REGISTRY" \
    "$BIN_OCI" >"$out/log" 2>&1; then
    cat "$out/log" >&2
    return 1
  fi
  grep -oE '[0-9./:a-zA-Z_-]+@sha256:[a-f0-9]{64}' "$out/log" | head -1
}
REF1=$(build_oid "$OID1")
echo v2 >"$WORKDIR/README"
git -C "$WORKDIR" add README && git -C "$WORKDIR" commit -q -m v2
OID2=$(git -C "$WORKDIR" rev-parse HEAD)
git -C "$WORKDIR" push -q "$TMP/bare.git" HEAD:refs/heads/main
REF2=$(build_oid "$OID2")
[[ -n "$REF1" && -n "$REF2" ]] || { echo "failed to resolve oci refs" >&2; exit 1; }
[[ "$REF1" != "$REF2" ]] || { echo "expected distinct digests got $REF1" >&2; exit 1; }
docker pull "$REF1" >/dev/null
docker pull "$REF2" >/dev/null
echo "digest1=$REF1"
echo "digest2=$REF2 oid1=$OID1 oid2=$OID2"

export MUSEHUB_PIPELINE_ADAPTER_BIN="$PWD/target/debug/musehub-pipeline-oci"
cargo run -p musehub-migrate -- up migrations "$DATABASE_URL"
env \
  CARGO_BIN_EXE_musehub_agent_reference="$PWD/target/debug/musehub-agent-reference" \
  CARGO_BIN_EXE_musehub_compose_runner="$PWD/target/debug/musehub-compose-runner" \
  CARGO_BIN_EXE_musehub_compose_mock="$PWD/target/debug/musehub-compose-mock" \
  cargo test -p musehub-app --test pipeline_oci_product_e2e -- --test-threads=1
echo "pipeline_oci_product=ok"
