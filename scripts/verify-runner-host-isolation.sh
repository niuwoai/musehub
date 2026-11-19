#!/usr/bin/env bash
# Standalone Runner host template: render it and assert the CI Docker boundary.
#
# The job adapter runs `docker run` for untrusted workflow code. This acceptance fails when the
# rendered configuration hands the Runner container the host Docker socket, drops TLS
# verification, loses the read-only client certificate mount, or loses a shared store mount.
# The checker is also run against a deliberately weakened render so a passing run cannot be
# explained by an assertion that never fires.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

COMPOSE_FILE="$ROOT/deploy/docker-compose.runner-host.yml"
PROFILE=experimental-shared-storage
SERVICE=musehub-runner
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

cat >"$WORK/assert_render.py" <<'PY'
import json
import sys

render_path, service, expect = sys.argv[1], sys.argv[2], sys.argv[3]
with open(render_path, encoding="utf-8") as handle:
    config = json.load(handle)

problems = []
services = config.get("services") or {}
if service not in services:
    problems.append("service_missing")
else:
    rendered = services[service]
    volumes = rendered.get("volumes") or []
    for volume in volumes:
        source = str(volume.get("source") or "")
        target = str(volume.get("target") or "")
        if "docker.sock" in source or "docker.sock" in target:
            problems.append("docker_socket_mounted")
    certs = [item for item in volumes if item.get("target") == "/var/lib/musehub/docker-certs"]
    if len(certs) != 1:
        problems.append("ci_docker_cert_mount_count")
    elif not certs[0].get("read_only"):
        problems.append("ci_docker_cert_mount_not_read_only")
    for target in (
        "/var/lib/musehub/git",
        "/var/lib/musehub/artifacts",
        "/var/lib/musehub/pipeline-workspaces",
    ):
        mounted = [item for item in volumes if item.get("target") == target]
        if len(mounted) != 1:
            problems.append(f"shared_store_missing:{target}")
        elif mounted[0].get("read_only"):
            problems.append(f"shared_store_read_only:{target}")
    environment = rendered.get("environment") or {}
    if not str(environment.get("DOCKER_HOST") or "").startswith("tcp://"):
        problems.append("docker_host_not_tcp")
    if environment.get("DOCKER_TLS_VERIFY") != "1":
        problems.append("docker_tls_verify_not_1")
    if environment.get("DOCKER_CERT_PATH") != "/var/lib/musehub/docker-certs":
        problems.append("docker_cert_path_mismatch")

if expect == "clean" and problems:
    print("runner_host_isolation_violation=" + ",".join(problems), file=sys.stderr)
    sys.exit(1)
if expect == "violation" and not problems:
    print("runner_host_isolation_checker_not_effective", file=sys.stderr)
    sys.exit(1)
PY

write_env() {
  local target="$1"
  local instance="$2"
  local docker_host="$3"
  local cert_dir="$4"
  {
    echo "MUSEHUB_RUNNER_INSTANCE_ID=$instance"
    echo "MUSEHUB_OCI_REGISTRY=registry.internal:5000"
    echo "MUSEHUB_JOB_ALLOWED_IMAGES=registry.internal:5000/ci@sha256:$(printf 'a%.0s' $(seq 1 64))"
    echo "MUSEHUB_GIT_ROOT_HOST=/srv/musehub/git"
    echo "MUSEHUB_ARTIFACT_ROOT_HOST=/srv/musehub/artifacts"
    echo "MUSEHUB_PIPELINE_WORKSPACE_ROOT_HOST=/srv/musehub/pipeline-workspaces"
    echo "MUSEHUB_RUNNER_DOCKER_HOST=$docker_host"
    echo "MUSEHUB_RUNNER_DOCKER_CERT_DIR=$cert_dir"
  } >"$target"
  : >"$WORK/runner-host.env"
}

render() {
  local env_file="$1"
  local output="$2"
  docker compose --project-directory "$WORK" -f "$COMPOSE_FILE" \
    --profile "$PROFILE" --env-file "$env_file" config --format json >"$output"
}

if ! docker compose version >/dev/null 2>&1; then
  echo "docker compose is required for the runner host isolation check" >&2
  exit 1
fi

instance=00000000-0000-4000-8000-000000000000
write_env "$WORK/happy.env" "$instance" tcp://ci-docker.internal:2376 /etc/musehub/docker-certs
if ! render "$WORK/happy.env" "$WORK/happy.json" 2>"$WORK/happy.err"; then
  echo "runner_host_isolation_render_failed" >&2
  cat "$WORK/happy.err" >&2
  exit 1
fi
python3 "$WORK/assert_render.py" "$WORK/happy.json" "$SERVICE" clean
echo "runner_host_tls_endpoint_and_shared_stores=ok"

python3 - "$WORK/happy.json" "$WORK/weakened.json" <<'PY'
import json
import sys

source, destination = sys.argv[1], sys.argv[2]
with open(source, encoding="utf-8") as handle:
    config = json.load(handle)
config["services"]["musehub-runner"]["volumes"].append(
    {"type": "bind", "source": "/var/run/docker.sock", "target": "/var/run/docker.sock"}
)
with open(destination, "w", encoding="utf-8") as handle:
    json.dump(config, handle)
PY
if python3 "$WORK/assert_render.py" "$WORK/weakened.json" "$SERVICE" violation 2>/dev/null; then
  echo "runner_host_isolation_checker_not_effective" >&2
  exit 1
fi
echo "runner_host_docker_socket_rejected=ok"

check_required() {
  local label="$1"
  local env_file="$2"
  set +e
  out=$(docker compose --project-directory "$WORK" -f "$COMPOSE_FILE" \
    --profile "$PROFILE" --env-file "$env_file" config 2>&1)
  ec=$?
  set -e
  if [[ $ec -eq 0 ]]; then
    echo "${label}_rendered_without_required_variable" >&2
    exit 1
  fi
  echo "${label}_required=ok"
}

write_env "$WORK/no_docker_host.env" "$instance" "" /etc/musehub/docker-certs
check_required runner_host_docker_host "$WORK/no_docker_host.env"

write_env "$WORK/no_cert_dir.env" "$instance" tcp://ci-docker.internal:2376 ""
check_required runner_host_docker_cert_dir "$WORK/no_cert_dir.env"

echo "runner_host_isolation=ok"
