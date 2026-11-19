#!/usr/bin/env bash
# Isolated, read-only API performance verification of a throwaway product stack.
#
# Creates a uniquely named Compose project, builds it with BuildKit, seeds the dataset the
# documented target needs (>= 50 active members, >= 100 repositories) through the API, and runs
# the documented 100 RPS / 60 s read-only benchmark from scripts/benchmark-api.py.
#
# Every resource this script touches carries the generated project name:
#   project   musehub-perf-<run id>
#   work dir  ${TMPDIR:-/tmp}/musehub-perf.<run id>.XXXX (unique env/config paths, mode 700)
#   ports     dynamically allocated free loopback ports
#   images    musehub-perf-<service>:<run id> (built with a run-specific build label)
#   reports   .local/perf/runs/isolated-<run id>/
#
# Nothing is built, pulled or started before the preflight passes, and the preflight itself only
# runs after the report path was claimed atomically with mkdir(2): a second invocation that uses the
# same MUSEHUB_PERF_RUN_ID is refused instead of racing into the pull/build steps, and a failed
# preflight releases only the empty claim this invocation created. The preflight then requires an
# unused target project, absent run-specific image tags, a provable local Docker root and at least
# 20 GiB free on it; a Docker root that cannot be established is refused rather than replaced by a
# default path. The preflight runs before the cleanup trap is installed.
#
# Cleanup is label-scoped and refuses to run unless the project identifier, its marker file and the
# validated state all agree. Only the run-specific image tags are removed, and only after the
# project containers are gone. Nothing outside that project is removed. Scheduler job execution is
# never used.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SELF="$ROOT/scripts/$(basename "${BASH_SOURCE[0]}")"
COMPOSE_FILE="$ROOT/deploy/docker-compose.yml"
PROJECT_PREFIX="musehub-perf"
PROJECT_REGEX='^musehub-perf-[a-z0-9][a-z0-9-]{5,48}$'
RUN_ID_REGEX='^[a-z0-9][a-z0-9-]{5,48}$'
CONTROL_PLANE_MEMORY_CAP_MIB=8192
BENCHMARK_RPS=100
BENCHMARK_DURATION_SECONDS=60
BENCHMARK_MIN_MEMBERS=50
BENCHMARK_MIN_REPOSITORIES=100
SEED_MEMBERS=49
SEED_REPOSITORIES=100
CONCURRENT_JOBS_DECLARED=10
ORG="${MUSEHUB_PERF_ORG:-demo}"
ADMIN_HANDLE="${MUSEHUB_PERF_HANDLE:-admin}"
READY_TIMEOUT_SECONDS="${MUSEHUB_PERF_READY_TIMEOUT_SECONDS:-900}"

SERVICE_NAMES=(postgres musehub-server musehub-git-ssh gateway)
SERVICE_MEMORY_MIB=(2560 4096 768 512)

PROJECT_NAME=""
PROJECT_VALIDATED=0
WORK_DIR=""
ENV_FILE=""
OVERRIDE_FILE=""
PROJECT_MARKER=""
RUN_ID=""
REPORT_DIR=""
RESERVATION_DIR=""
RESERVATION_OWNED=0
HTTP_PORT=""
GIT_SSH_PORT=""
ADMIN_PASSWORD=""
SSD_DECLARED="unknown"
VCPU=""
CPU_MODEL="unknown"
HOST_MEMORY_MIB="unknown"
DOCKER_ROOT_DIR="unknown"
BACKING_DEVICE="unknown"
BACKING_FS="unknown"
DISK_ROTATIONAL="unknown"
DISK_TYPE="unknown"
ERRORS=0

log() { printf '  %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'USAGE'
Usage: ./scripts/verify-api-performance-isolated.sh [--keep] [--self-check]

  --keep         leave the isolated Compose project running (prints the cleanup command)
  --self-check   run identifier, memory-cap, cleanup-guard, Docker-root refusal and run-path
                 reservation checks only; no Docker resources are created and Docker is not
                 invoked

Requirements before a real run:
  - the report path .local/perf/runs/isolated-<run id> must be unused: it is claimed atomically
    before the Docker preflight, so a concurrent run with the same MUSEHUB_PERF_RUN_ID is refused,
    and a failed preflight releases only the empty claim this run created
  - the local Docker root must be provable: docker info must succeed, DOCKER_HOST must be unset or
    a local unix socket, the docker context must be the default local context, and the reported
    DockerRootDir must exist on an inspectable filesystem; otherwise the run is refused instead of
    falling back to the default Docker directory or the filesystem root
  - the Compose project musehub-perf-<run id> must not already own containers, volumes or
    networks (the preflight refuses instead of adopting or deleting them)
  - no image musehub-perf-<service>:<run id> may already exist
  - that Docker root needs at least 20 GiB free, because the stack is built and pulled first; the
    preflight refuses before any build or pull
Environment:
  MUSEHUB_PERF_RUN_ID   optional run identifier suffix; must match [a-z0-9][a-z0-9-]{5,48}
  MUSEHUB_PERF_ORG      organization slug (default: demo)
  MUSEHUB_PERF_KEEP     1 = same as --keep
USAGE
}

lower() { printf '%s' "$1" | tr '[:upper:]' '[:lower:]'; }

new_run_id() {
  local stamp random_hex
  stamp="$(lower "$(date -u +%Y%m%dT%H%M%SZ)")"
  random_hex="$(od -An -N3 -tx1 /dev/urandom | tr -d ' \n')"
  [[ -n "$random_hex" ]] || die "could not read randomness for the run identifier"
  printf '%s-%s\n' "$stamp" "$random_hex"
}

validate_run_id() {
  local candidate="${1:-}"
  [[ -n "$candidate" ]] || return 1
  [[ "$candidate" =~ $RUN_ID_REGEX ]] || return 1
  [[ "$candidate" != *--* ]] || return 1
  [[ "$candidate" != *- ]] || return 1
  return 0
}

validate_project_name() {
  local candidate="${1:-}"
  [[ -n "$candidate" ]] || return 1
  [[ "$candidate" =~ $PROJECT_REGEX ]] || return 1
  [[ "$candidate" != *--* ]] || return 1
  [[ "$candidate" != *- ]] || return 1
  [[ "$candidate" != *..* ]] || return 1
  return 0
}

require_validated_project() {
  local name="${1:-}" marker="${2:-}"
  if [[ "${PROJECT_VALIDATED:-0}" != "1" ]]; then
    printf 'refusing cleanup: project %q was never validated by this run\n' "$name" >&2
    return 1
  fi
  if ! validate_project_name "$name"; then
    printf 'refusing cleanup: unsafe project identifier %q\n' "$name" >&2
    return 1
  fi
  if [[ -z "$marker" || ! -f "$marker" ]]; then
    printf 'refusing cleanup: project marker %q is missing\n' "$marker" >&2
    return 1
  fi
  if [[ "$(head -n 1 "$marker" 2>/dev/null || true)" != "$name" ]]; then
    printf 'refusing cleanup: marker %q does not record project %q\n' "$marker" "$name" >&2
    return 1
  fi
  return 0
}

memory_cap_total_mib() {
  local total=0 value
  for value in "${SERVICE_MEMORY_MIB[@]}"; do
    total=$((total + value))
  done
  printf '%s\n' "$total"
}

free_ports() {
  python3 - "$@" <<'PY'
import socket
import sys

def free_port():
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]

count = int(sys.argv[1]) if len(sys.argv) > 1 else 2
ports = []
while len(ports) < count:
    port = free_port()
    if port not in ports:
        ports.append(port)
print(" ".join(str(port) for port in ports))
PY
}

probe_cpu() {
  VCPU="$( (nproc 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null || printf 'unknown') | head -n 1)"
  CPU_MODEL="$(awk -F':' '/^model name|^Model|^cpu model/ {gsub(/^ +| +$/, "", $2); print $2; exit}' /proc/cpuinfo 2>/dev/null || true)"
  if [[ -z "$CPU_MODEL" ]]; then
    CPU_MODEL="$(sysctl -n machdep.cpu.brand_string 2>/dev/null || printf 'unknown')"
  fi
}

probe_host_memory() {
  HOST_MEMORY_MIB="$(awk '/^MemTotal:/ {print int($2 / 1024); exit}' /proc/meminfo 2>/dev/null || true)"
  [[ -n "$HOST_MEMORY_MIB" ]] || HOST_MEMORY_MIB="unknown"
}

# Prints the free KiB of the filesystem holding <dir>, or fails. An empty, relative, missing or
# uninspectable path is refused; no substitute path is ever used in its place.
docker_root_free_kib() {
  local dir="${1:-}" available_kib
  [[ -n "$dir" ]] || return 1
  [[ "$dir" == /* ]] || return 1
  [[ -d "$dir" ]] || return 1
  available_kib="$(df -Pk "$dir" 2>/dev/null | awk 'NR == 2 {print $4}')"
  [[ "${available_kib:-}" =~ ^[0-9]+$ ]] || return 1
  printf '%s\n' "$available_kib"
}

# Establishes the local Docker root or fails. A non-local DOCKER_HOST, a non-default docker context,
# a failed `docker info`, or a reported root that does not exist or cannot be inspected refuses the
# run: this script measures the daemon's own storage, so it never silently substitutes another path.
probe_docker_root() {
  local host="${DOCKER_HOST:-}" context="${DOCKER_CONTEXT:-}" root
  case "$host" in
    "" | unix://*) ;;
    *)
      printf 'refusing to run: DOCKER_HOST=%s is not a local unix socket, so the Docker root cannot be proven local\n' "$host" >&2
      return 1
      ;;
  esac
  if [[ -n "$context" && "$context" != "default" ]]; then
    printf 'refusing to run: DOCKER_CONTEXT=%s is not the local default context\n' "$context" >&2
    return 1
  fi
  context="$(docker context show 2>/dev/null || true)"
  if [[ -n "$context" && "$context" != "default" ]]; then
    printf 'refusing to run: docker context %s is not the local default context\n' "$context" >&2
    return 1
  fi
  if ! root="$(docker info --format '{{.DockerRootDir}}' 2>/dev/null)"; then
    printf 'refusing to run: docker info failed, so the local Docker daemon and its root directory could not be established\n' >&2
    DOCKER_ROOT_DIR=""
    return 1
  fi
  root="$(printf '%s' "$root" | tr -d '\r\n')"
  if ! docker_root_free_kib "$root" >/dev/null; then
    printf 'refusing to run: reported Docker root %s is not an existing local directory with an inspectable filesystem\n' "${root:-<empty>}" >&2
    DOCKER_ROOT_DIR=""
    return 1
  fi
  DOCKER_ROOT_DIR="$root"
  return 0
}

disk_rotational_flag() {
  local block="${1#/dev/}"
  [[ -n "$block" ]] || return 1
  [[ -r "/sys/class/block/$block/queue/rotational" ]] || return 1
  head -n 1 "/sys/class/block/$block/queue/rotational" 2>/dev/null || return 1
}

probe_disk_type() {
  local target root_source resolved name path kind rota
  local chain="" layers=0 unreadable=0 saw_rotational=0 saw_disk=0
  target="$DOCKER_ROOT_DIR"
  BACKING_FS="$(df -PT "$target" 2>/dev/null | awk 'NR == 2 {print $2}')"
  [[ -n "$BACKING_FS" ]] || BACKING_FS="unknown"
  root_source="$(df -P "$target" 2>/dev/null | awk 'NR == 2 {print $1}')"
  resolved=""
  # lsblk -s walks the reverse dependency chain from the filesystem source upwards: dm/crypt/LVM or
  # md layers, their partitions, and finally every whole disk backing them. A dm device reports its
  # own queue/rotational flag, so only the underlying disks prove anything about the medium: every
  # resolved layer must report rotational=0 and at least one whole disk must be reached before this
  # script declares the Docker-root backing storage SSD.
  if [[ -n "$root_source" && "$root_source" == /dev/* ]] && command -v lsblk >/dev/null 2>&1; then
    resolved="$(readlink -f "$root_source" 2>/dev/null || true)"
    [[ -n "$resolved" && -b "$resolved" ]] || resolved="$root_source"
    while read -r name path kind rota; do
      [[ -n "$name" && -n "$kind" ]] || continue
      if [[ -z "$rota" ]]; then
        rota="$(disk_rotational_flag "$path" || true)"
      fi
      layers=$((layers + 1))
      case "$rota" in
        0) ;;
        1) saw_rotational=1 ;;
        *)
          rota="unknown"
          unreadable=$((unreadable + 1))
          ;;
      esac
      if [[ "$kind" == "disk" ]]; then
        saw_disk=1
      fi
      chain="${chain:+$chain<-}${name}(${kind}):${rota}"
    done < <(lsblk -s -n -o NAME,PATH,TYPE,ROTA "$resolved" 2>/dev/null || true)
  fi
  # A Docker root that is not on a local block device, an unresolved chain, an unreadable rotational
  # value, or a chain that never reaches a whole disk stays unknown; a backing store we cannot prove
  # is never declared SSD.
  if ((layers == 0)) || ((unreadable > 0)) || ((saw_disk == 0)); then
    DISK_ROTATIONAL="unknown"
    DISK_TYPE="unknown"
    SSD_DECLARED="unknown"
  elif ((saw_rotational > 0)); then
    DISK_ROTATIONAL="1"
    DISK_TYPE="hdd"
    SSD_DECLARED="no"
  else
    DISK_ROTATIONAL="0"
    DISK_TYPE="ssd"
    SSD_DECLARED="yes"
  fi
  BACKING_DEVICE="${root_source:-unknown} (resolved=${resolved:-unresolved}, chain=${chain:-unresolved})"
  log "docker root: $DOCKER_ROOT_DIR ($BACKING_DEVICE), fs=$BACKING_FS, rotational=$DISK_ROTATIONAL, disk_type=$DISK_TYPE, ssd_declared=$SSD_DECLARED"
}

compose() {
  DOCKER_BUILDKIT=1 COMPOSE_DOCKER_CLI_BUILD=1 docker compose \
    --project-name "$PROJECT_NAME" \
    --project-directory "$ROOT/deploy" \
    -f "$COMPOSE_FILE" \
    -f "$OVERRIDE_FILE" \
    --env-file "$ENV_FILE" \
    "$@"
}

PROJECT_LABEL_KEY="com.docker.compose.project"
RUN_IMAGE_LABEL_KEY="com.musehub.perf.run"
BUILD_LABEL_KEY="com.musehub.perf.project"
DOCKER_ROOT_MIN_FREE_GIB=20
BUILD_SERVICE_NAMES=(musehub-server musehub-git-ssh gateway)

is_build_service() {
  local service="${1:-}" candidate
  for candidate in "${BUILD_SERVICE_NAMES[@]}"; do
    [[ "$service" == "$candidate" ]] && return 0
  done
  return 1
}

image_tag_for() {
  printf '%s-%s:%s\n' "$PROJECT_PREFIX" "${1:-}" "$RUN_ID"
}

project_containers() {
  docker ps -aq --filter "label=$PROJECT_LABEL_KEY=$PROJECT_NAME" 2>/dev/null || true
}

project_volumes() {
  docker volume ls -q --filter "label=$PROJECT_LABEL_KEY=$PROJECT_NAME" 2>/dev/null || true
}

project_networks() {
  docker network ls -q --filter "label=$PROJECT_LABEL_KEY=$PROJECT_NAME" 2>/dev/null || true
}

# Runs before the cleanup trap is installed and before any compose build/up or image pull. A
# project name that is already in use is refused instead of adopted or deleted.
preflight_project_absence() {
  local found
  found="$(project_containers)"
  if [[ -n "$found" ]]; then
    printf 'refusing to run: compose project %s already owns containers: %s\n' \
      "$PROJECT_NAME" "$(printf '%s' "$found" | tr '\n' ' ')" >&2
    return 1
  fi
  found="$(project_volumes)"
  if [[ -n "$found" ]]; then
    printf 'refusing to run: compose project %s already owns volumes: %s\n' \
      "$PROJECT_NAME" "$(printf '%s' "$found" | tr '\n' ' ')" >&2
    return 1
  fi
  found="$(project_networks)"
  if [[ -n "$found" ]]; then
    printf 'refusing to run: compose project %s already owns networks: %s\n' \
      "$PROJECT_NAME" "$(printf '%s' "$found" | tr '\n' ' ')" >&2
    return 1
  fi
  printf 'preflight_project_absent project=%s\n' "$PROJECT_NAME"
  return 0
}

preflight_image_collisions() {
  local service tag labelled
  for service in "${BUILD_SERVICE_NAMES[@]}"; do
    tag="$(image_tag_for "$service")"
    if docker image inspect -f '{{.Id}}' "$tag" >/dev/null 2>&1; then
      printf 'refusing to run: run-specific image tag %s already exists\n' "$tag" >&2
      return 1
    fi
    labelled="$(docker image ls -q --filter "label=$RUN_IMAGE_LABEL_KEY=$RUN_ID" 2>/dev/null || true)"
    if [[ -n "$labelled" ]]; then
      printf 'refusing to run: images already carry %s=%s: %s\n' \
        "$RUN_IMAGE_LABEL_KEY" "$RUN_ID" "$(printf '%s' "$labelled" | tr '\n' ' ')" >&2
      return 1
    fi
  done
  printf 'preflight_image_tags_absent count=%s\n' "${#BUILD_SERVICE_NAMES[@]}"
  return 0
}

preflight_docker_root_space() {
  local target available_kib available_gib
  target="$DOCKER_ROOT_DIR"
  if ! available_kib="$(docker_root_free_kib "$target")"; then
    printf 'refusing to run: docker root %s is missing or its filesystem cannot be inspected\n' "${target:-<unset>}" >&2
    return 1
  fi
  available_gib=$((available_kib / 1024 / 1024))
  if ((available_gib < DOCKER_ROOT_MIN_FREE_GIB)); then
    printf 'refusing to run: docker root %s has %s GiB free, at least %s GiB is required before pulling or building images\n' \
      "$target" "$available_gib" "$DOCKER_ROOT_MIN_FREE_GIB" >&2
    return 1
  fi
  printf 'preflight_docker_root_free_gib=%s required_gib=%s\n' "$available_gib" "$DOCKER_ROOT_MIN_FREE_GIB"
  return 0
}

preflight_run() {
  probe_docker_root || return 1
  preflight_project_absence || return 1
  preflight_image_collisions || return 1
  preflight_docker_root_space || return 1
  return 0
}

# Claims <dir> as this run's report path with mkdir(2). That atomic create is the mutual exclusion:
# a second invocation using the same MUSEHUB_PERF_RUN_ID fails here instead of racing into the
# pull/build steps. Nothing is written inside the claim, so a failed preflight can remove it.
reserve_run_path() {
  local dir="${1:-}" parent
  [[ -n "$dir" ]] || { printf 'refusing to run: empty run path\n' >&2; return 1; }
  parent="$(dirname "$dir")"
  if ! mkdir -p "$parent" 2>/dev/null; then
    printf 'refusing to run: cannot create run path parent %s\n' "$parent" >&2
    return 1
  fi
  if ! mkdir "$dir" 2>/dev/null; then
    if [[ -e "$dir" ]]; then
      printf 'refusing to run: run path %s is already reserved by another run\n' "$dir" >&2
    else
      printf 'refusing to run: cannot create run path %s\n' "$dir" >&2
    fi
    return 1
  fi
  RESERVATION_DIR="$dir"
  RESERVATION_OWNED=1
  printf 'run_path_reserved=%s\n' "$dir"
  return 0
}

# Releases only the empty claim created by this invocation. A path that already holds reports is
# evidence and is never removed.
release_run_reservation() {
  local dir="${RESERVATION_DIR:-}"
  [[ "${RESERVATION_OWNED:-0}" == "1" ]] || return 0
  RESERVATION_OWNED=0
  RESERVATION_DIR=""
  [[ -n "$dir" && -d "$dir" ]] || return 0
  if [[ -n "$(ls -A "$dir" 2>/dev/null)" ]]; then
    printf 'refusing to release non-empty run path %s\n' "$dir" >&2
    return 1
  fi
  if ! rmdir "$dir" 2>/dev/null; then
    printf 'could not release run path reservation %s\n' "$dir" >&2
    return 1
  fi
  printf 'run_path_released=%s\n' "$dir"
  return 0
}

release_run_reservation_and_die() {
  release_run_reservation || true
  die "$@"
}

# Removes only the run-specific image tags built for this run, and only after the project
# containers are gone. An image whose run label does not match is left alone.
remove_run_images() {
  local service tag label removed=0 failed=0
  for service in "${BUILD_SERVICE_NAMES[@]}"; do
    tag="$(image_tag_for "$service")"
    if ! docker image inspect -f '{{.Id}}' "$tag" >/dev/null 2>&1; then
      continue
    fi
    label="$(docker image inspect -f '{{ index .Config.Labels "'"$RUN_IMAGE_LABEL_KEY"'" }}' "$tag" 2>/dev/null || true)"
    if [[ "$label" != "$RUN_ID" ]]; then
      printf 'cleanup_skipped_foreign_resource image=%s label=%s\n' "$tag" "${label:-none}" >&2
      continue
    fi
    if docker image rm "$tag" >/dev/null 2>&1; then
      removed=$((removed + 1))
    else
      printf 'cleanup_image_removal_failed image=%s\n' "$tag" >&2
      failed=$((failed + 1))
    fi
  done
  printf 'cleanup_images_run_scoped removed=%s failed=%s\n' "$removed" "$failed"
  ((failed == 0))
}

cleanup_project() {
  local name="${1:-}" marker="${2:-}" container_id label remaining volume_name network_name
  local service tag image_status=0
  if ! require_validated_project "$name" "$marker"; then
    printf 'cleanup_skipped_unvalidated_project\n' >&2
    return 1
  fi
  if [[ "${KEEP_PROJECT:-0}" == "1" ]]; then
    printf 'cleanup_skipped_keep_requested project=%s\n' "$name"
    printf 'cleanup command: docker compose --project-name %s -f %s -f %s --env-file %s down -v --remove-orphans\n' \
      "$name" "$COMPOSE_FILE" "$OVERRIDE_FILE" "$ENV_FILE"
    printf 'cleanup image command: docker image rm'
    for service in "${BUILD_SERVICE_NAMES[@]}"; do
      printf ' %s' "$(image_tag_for "$service")"
    done
    printf '\n'
    return 0
  fi
  if [[ -n "$ENV_FILE" && -f "$ENV_FILE" && -n "$OVERRIDE_FILE" && -f "$OVERRIDE_FILE" ]]; then
    compose down -v --remove-orphans --timeout 30 >/dev/null 2>&1 || true
  fi
  while IFS= read -r container_id; do
    [[ -n "$container_id" ]] || continue
    label="$(docker inspect -f '{{ index .Config.Labels "com.docker.compose.project" }}' "$container_id" 2>/dev/null || true)"
    if [[ "$label" == "$name" ]]; then
      docker rm -f "$container_id" >/dev/null 2>&1 || true
    else
      printf 'cleanup_skipped_foreign_resource container=%s label=%s\n' "$container_id" "${label:-none}" >&2
    fi
  done < <(project_containers)
  while IFS= read -r volume_name; do
    [[ -n "$volume_name" ]] || continue
    label="$(docker volume inspect -f '{{ index .Labels "com.docker.compose.project" }}' "$volume_name" 2>/dev/null || true)"
    if [[ "$label" == "$name" ]]; then
      docker volume rm -f "$volume_name" >/dev/null 2>&1 || true
    else
      printf 'cleanup_skipped_foreign_resource volume=%s label=%s\n' "$volume_name" "${label:-none}" >&2
    fi
  done < <(project_volumes)
  while IFS= read -r network_name; do
    [[ -n "$network_name" ]] || continue
    label="$(docker network inspect -f '{{ index .Labels "com.docker.compose.project" }}' "$network_name" 2>/dev/null || true)"
    if [[ "$label" == "$name" ]]; then
      docker network rm "$network_name" >/dev/null 2>&1 || true
    else
      printf 'cleanup_skipped_foreign_resource network=%s label=%s\n' "$network_name" "${label:-none}" >&2
    fi
  done < <(project_networks)
  remaining="$(project_containers)"
  if [[ -n "$remaining" ]]; then
    printf 'cleanup_incomplete project=%s remaining=%s\n' "$name" "$(printf '%s' "$remaining" | tr '\n' ' ')" >&2
    return 1
  fi
  remove_run_images || image_status=1
  if ((image_status != 0)); then
    printf 'cleanup_incomplete project=%s run_image_tags_remaining=1\n' "$name" >&2
    return 1
  fi
  printf 'cleanup_complete project=%s\n' "$name"
  return 0
}

on_exit() {
  local status=$?
  set +e
  if [[ "$PROJECT_VALIDATED" == "1" ]]; then
    cleanup_project "$PROJECT_NAME" "$PROJECT_MARKER" || true
  else
    printf 'cleanup_skipped_unvalidated_project (no project was created)\n' >&2
  fi
  if [[ -n "$WORK_DIR" && -d "$WORK_DIR" && "${KEEP_PROJECT:-0}" != "1" ]]; then
    rm -rf "$WORK_DIR"
  fi
  exit "$status"
}

write_env_and_override() {
  local index name memory
  umask 077
  ADMIN_PASSWORD="$(od -An -N24 -tx1 /dev/urandom | tr -d ' \n')"
  local session_secret webhook_key postgres_password
  session_secret="$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
  postgres_password="$(od -An -N18 -tx1 /dev/urandom | tr -d ' \n')"
  webhook_key="$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
  {
    printf 'POSTGRES_PASSWORD=%s\n' "$postgres_password"
    printf 'MUSEHUB_SESSION_SECRET=%s\n' "$session_secret"
    printf 'MUSEHUB_WEBHOOK_ENCRYPTION_KEY=%s\n' "$webhook_key"
    printf 'MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD=%s\n' "$ADMIN_PASSWORD"
    printf 'MUSEHUB_GATEWAY_HTTP_PORT=%s\n' "$HTTP_PORT"
    printf 'MUSEHUB_GIT_SSH_PORT=%s\n' "$GIT_SSH_PORT"
    printf 'MUSEHUB_PUBLIC_HTTP_ORIGIN=http://127.0.0.1:%s\n' "$HTTP_PORT"
    printf 'MUSEHUB_PUBLIC_SSH_HOST=127.0.0.1\n'
  } >"$ENV_FILE"
  chmod 600 "$ENV_FILE"
    {
      printf 'services:\n'
      for index in "${!SERVICE_NAMES[@]}"; do
        name="${SERVICE_NAMES[$index]}"
        memory="${SERVICE_MEMORY_MIB[$index]}"
        printf '  %s:\n' "$name"
        printf '    mem_limit: %sm\n' "$memory"
        printf '    memswap_limit: %sm\n' "$memory"
        if is_build_service "$name"; then
          printf '    image: %s\n' "$(image_tag_for "$name")"
          printf '    build:\n'
          printf '      labels:\n'
          printf '        %s: %s\n' "$RUN_IMAGE_LABEL_KEY" "$RUN_ID"
          printf '        %s: %s\n' "$BUILD_LABEL_KEY" "$PROJECT_NAME"
        fi
      done
    } >"$OVERRIDE_FILE"
  chmod 600 "$OVERRIDE_FILE"
}

wait_for_ready() {
  local origin="$1" timeout_seconds="$2" service="$3"
  python3 - "$origin" "$timeout_seconds" "$service" <<'PY'
import json
import sys
import time
import urllib.request

origin, timeout_seconds, service = sys.argv[1], float(sys.argv[2]), sys.argv[3]
url = origin.rstrip("/") + "/readyz"
deadline = time.monotonic() + timeout_seconds
started = time.monotonic()
last = "no response"
while time.monotonic() < deadline:
    try:
        with urllib.request.urlopen(url, timeout=5) as response:
            body = response.read().decode("utf-8", "replace")
        checks = json.loads(body).get("checks") or {}
    except Exception as error:  # noqa: BLE001 - readiness polling reports the last failure
        last = type(error).__name__ + ": " + str(error)[:160]
        time.sleep(2)
        continue
    if str(checks.get("product_api")) == "ok" and str(checks.get("database")) == "ok":
        waited = round(time.monotonic() - started, 1)
        print(f"readyz_ok service={service} waited_seconds={waited}")
        sys.exit(0)
    last = "product_api=" + str(checks.get("product_api")) + " database=" + str(checks.get("database"))
    time.sleep(2)
print(f"readyz_timeout service={service} last={last}", file=sys.stderr)
sys.exit(1)
PY
}

seed_dataset() {
  local origin="$1" org="$2" slug="$3"
  MUSEHUB_PERF_PASSWORD="$ADMIN_PASSWORD" python3 - "$origin" "$org" "$ADMIN_HANDLE" "$slug" "$SEED_MEMBERS" "$SEED_REPOSITORIES" <<'PY'
import json
import os
import secrets
import sys
import urllib.error
import urllib.request

origin, org, handle, slug, members_needed, repositories_needed = sys.argv[1:7]
members_needed, repositories_needed = int(members_needed), int(repositories_needed)
password = os.environ["MUSEHUB_PERF_PASSWORD"]
api = origin.rstrip("/") + "/api/v1"


def call(method, path, body=None, cookie=None):
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(api + path, data=data, method=method)
    request.add_header("Content-Type", "application/json")
    if cookie:
        request.add_header("Cookie", cookie)
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return response.status, response.read().decode("utf-8", "replace"), response.headers.get("Set-Cookie")
    except urllib.error.HTTPError as error:
        return error.code, error.read().decode("utf-8", "replace"), error.headers.get("Set-Cookie")


def existing(path, cookie):
    status, body, _ = call("GET", path, cookie=cookie)
    if status != 200:
        raise SystemExit(f"dataset preflight failed (HTTP {status}) for {path}")
    payload = json.loads(body)
    if not isinstance(payload, list):
        raise SystemExit(f"dataset preflight returned a non-list payload for {path}")
    return payload


status, _body, set_cookie = call("POST", "/auth/login", {"handle": handle, "password": password})
if status != 200 or not set_cookie:
    raise SystemExit(f"admin login failed (HTTP {status})")
cookie = set_cookie.split(";", 1)[0]

active = sum(
    1
    for item in existing(f"/orgs/{org}/members", cookie)
    if isinstance(item, dict)
    and item.get("membership_status") == "active"
    and item.get("principal_status") == "active"
)
created_members = 0
index = 0
while active < 1 + members_needed:
    index += 1
    if index > members_needed + 10:
        raise SystemExit(f"could not reach {1 + members_needed} active members (have {active})")
    member = f"perf{slug}m{index:03d}"
    status, body, _ = call(
        "POST",
        f"/orgs/{org}/members",
        {"handle": member, "password": secrets.token_hex(20), "role": "member"},
        cookie=cookie,
    )
    if status in (200, 201):
        created_members += 1
        active += 1
    elif status == 409:
        continue
    else:
        raise SystemExit(f"member creation failed for {member} (HTTP {status}): {body[:200]}")

repositories = existing(f"/orgs/{org}/repos", cookie)
created_repositories = 0
index = len(repositories)
while len(repositories) < repositories_needed:
    index += 1
    if index > repositories_needed + 20:
        raise SystemExit(f"could not reach {repositories_needed} repositories (have {len(repositories)})")
    repo_slug = f"perf-r{index:04d}-{slug}"
    status, body, _ = call(
        "POST",
        f"/orgs/{org}/repos",
        {"slug": repo_slug, "description": "isolated API performance dataset"},
        cookie=cookie,
    )
    if status in (200, 201):
        created_repositories += 1
        repositories.append({"slug": repo_slug})
    elif status == 409:
        repositories.append({"slug": repo_slug})
    else:
        raise SystemExit(f"repository creation failed for {repo_slug} (HTTP {status}): {body[:200]}")

active_members = sum(
    1
    for item in existing(f"/orgs/{org}/members", cookie)
    if isinstance(item, dict)
    and item.get("membership_status") == "active"
    and item.get("principal_status") == "active"
)
observed_repositories = len(existing(f"/orgs/{org}/repos", cookie))
if active_members < 1 + members_needed or observed_repositories < repositories_needed:
    raise SystemExit(
        "dataset seeding did not reach the required scale "
        f"(active_members={active_members}, repositories={observed_repositories})"
    )
print(
    f"dataset_seeded created_members={created_members} created_repositories={created_repositories} "
    f"active_members={active_members} repositories={observed_repositories}"
)
PY
}

write_environment_report() {
  local report_dir="$1" origin="$2" benchmark_command="$3"
  MUSEHUB_PERF_SERVICE_NAMES="$(printf '%s\n' "${SERVICE_NAMES[@]}")" \
  MUSEHUB_PERF_SERVICE_MEMORY="${SERVICE_MEMORY_MIB[*]}" \
  python3 - "$report_dir" "$PROJECT_NAME" "$RUN_ID" "$origin" "$ORG" "$benchmark_command" <<'PY'
import json
import os
import sys

(report_dir, project, run_id, origin, org, benchmark_command) = sys.argv[1:7]
names = os.environ["MUSEHUB_PERF_SERVICE_NAMES"].split()
memory = [int(value) for value in os.environ["MUSEHUB_PERF_SERVICE_MEMORY"].split()]
limits = {name: {"mem_limit_mib": value} for name, value in zip(names, memory)}
cap_mib = int(os.environ["MUSEHUB_PERF_CAP_MIB"])
payload = {
    "run_id": run_id,
    "compose_project": project,
    "api_origin": origin,
    "organization": org,
    "measurement": {
        "kind": "read_only_api_benchmark",
        "rps": int(os.environ["MUSEHUB_PERF_RPS"]),
        "duration_seconds": int(os.environ["MUSEHUB_PERF_DURATION"]),
        "scheduler_job_execution": "disabled",
        "benchmark_command": benchmark_command,
    },
    "dataset": {
        "seed_members": int(os.environ["MUSEHUB_PERF_SEED_MEMBERS"]),
        "seed_repositories": int(os.environ["MUSEHUB_PERF_SEED_REPOSITORIES"]),
        "required_active_members": int(os.environ["MUSEHUB_PERF_MIN_MEMBERS"]),
        "required_repositories": int(os.environ["MUSEHUB_PERF_MIN_REPOSITORIES"]),
    },
    "host": {
        "cpu_vcpu": os.environ["MUSEHUB_PERF_VCPU"],
        "cpu_model": os.environ["MUSEHUB_PERF_CPU_MODEL"],
        "memory_total_mib": os.environ["MUSEHUB_PERF_HOST_MEMORY_MIB"],
    },
    "docker_root": {
        "path": os.environ["MUSEHUB_PERF_DOCKER_ROOT"],
        "backing_device": os.environ["MUSEHUB_PERF_BACKING_DEVICE"],
        "filesystem": os.environ["MUSEHUB_PERF_BACKING_FS"],
        "rotational": os.environ["MUSEHUB_PERF_ROTATIONAL"],
        "disk_type": os.environ["MUSEHUB_PERF_DISK_TYPE"],
        "ssd_declared": os.environ["MUSEHUB_PERF_SSD_DECLARED"],
          "ssd_declared_basis": (
              "every layer of the Docker-root backing chain (LVM/RAID/crypt/partition/disk) "
              "reports queue/rotational==0"
              if os.environ["MUSEHUB_PERF_SSD_DECLARED"] == "yes"
              else "not verified; the report does not claim SSD"
          ),
    },
    "control_plane_limits": {
        "services": limits,
        "total_mem_limit_mib": sum(memory),
        "cap_mib": cap_mib,
        "cap_enforced": sum(memory) <= cap_mib,
    },
    "notes": [
        "Repository names and container limits are reported; no credentials or session secrets are written here.",
        "Scheduler and job execution probes are intentionally not invoked by this script.",
    ],
}
with open(os.path.join(report_dir, "environment.json"), "w", encoding="utf-8") as handle:
    json.dump(payload, handle, indent=2, sort_keys=True)
    handle.write("\n")

rows = "\n".join(
    f"| `{name}` | {limits[name]['mem_limit_mib']} MiB |" for name in names
)
markdown = f"""# Isolated API performance environment

- Run id: `{run_id}`
- Compose project: `{project}`
- API origin: `{origin}`
- Organization: `{org}`
- Host CPU: {payload['host']['cpu_vcpu']} vCPU ({payload['host']['cpu_model']})
- Host memory: {payload['host']['memory_total_mib']} MiB
- Docker root: `{payload['docker_root']['path']}`
- Docker-root backing device: `{payload['docker_root']['backing_device']}`
- Docker-root filesystem: `{payload['docker_root']['filesystem']}`
- Backing rotational flag: `{payload['docker_root']['rotational']}`
- Backing disk type: `{payload['docker_root']['disk_type']}`
- SSD declared: `{payload['docker_root']['ssd_declared']}` ({payload['docker_root']['ssd_declared_basis']})

## Control-plane container limits

| Service | Memory limit |
| --- | --- |
{rows}
| **Total** | **{payload['control_plane_limits']['total_mem_limit_mib']} MiB** |

Cap: {cap_mib} MiB; enforced: `{payload['control_plane_limits']['cap_enforced']}`.

## Measurement

- Read-only benchmark: {payload['measurement']['rps']} RPS for {payload['measurement']['duration_seconds']} seconds.
- Scheduler job execution: `{payload['measurement']['scheduler_job_execution']}` (no workflow jobs are started).
- Benchmark command: `{benchmark_command}`
- Dataset scale: {payload['dataset']['seed_members']} additional members and {payload['dataset']['seed_repositories']} repositories created through the API.
"""
with open(os.path.join(report_dir, "environment.md"), "w", encoding="utf-8") as handle:
    handle.write(markdown)
print("environment_report_written environment.json environment.md")
PY
}

run_benchmark() {
  local origin="$1" report_dir="$2"
  env -u MUSEHUB_PERF_ALLOW_JOB_EXECUTION \
    MUSEHUB_PERF_PASSWORD="$ADMIN_PASSWORD" \
    python3 "$ROOT/scripts/benchmark-api.py" \
    --origin "$origin" \
    --org "$ORG" \
    --handle "$ADMIN_HANDLE" \
    --rps "$BENCHMARK_RPS" \
    --duration-seconds "$BENCHMARK_DURATION_SECONDS" \
    --min-members "$BENCHMARK_MIN_MEMBERS" \
    --min-repositories "$BENCHMARK_MIN_REPOSITORIES" \
    --control-plane-vcpu "$VCPU" \
    --control-plane-memory-gib "$((CONTROL_PLANE_MEMORY_CAP_MIB / 1024))" \
    --ssd-declared "$SSD_DECLARED" \
    --concurrent-jobs-declared "$CONCURRENT_JOBS_DECLARED" \
    --output-prefix "$report_dir/api"
}

function_body() {
  local name="$1"
  sed -n "/^$name() {\$/,/^}\$/p" "$SELF"
}

self_check() {
  local failures=0 total
  total="$(memory_cap_total_mib)"
  if ((total > CONTROL_PLANE_MEMORY_CAP_MIB)); then
    printf 'self_check_failed memory_cap_total=%s exceeds %s\n' "$total" "$CONTROL_PLANE_MEMORY_CAP_MIB" >&2
    failures=$((failures + 1))
  fi
  if ((${#SERVICE_NAMES[@]} != ${#SERVICE_MEMORY_MIB[@]})); then
    printf 'self_check_failed service_memory_limits_mismatch\n' >&2
    failures=$((failures + 1))
  fi
  local candidate
  for candidate in "musehub-perf-20260101t000000z-abcdef" "musehub-perf-abc123"; do
    if ! validate_project_name "$candidate"; then
      printf 'self_check_failed valid_project_rejected=%s\n' "$candidate" >&2
      failures=$((failures + 1))
    fi
  done
  for candidate in "" "musehub" "musehub-perf-" "musehub-perf-AB" "MuseHub-perf-abc123" "musehub-perf-abc--123" "musehub-product" "../musehub-perf-abc123"; do
    if validate_project_name "$candidate"; then
      printf 'self_check_failed unsafe_project_accepted=%q\n' "$candidate" >&2
      failures=$((failures + 1))
    fi
  done
  local guard_dir guard_marker guard_other
  guard_dir="$(mktemp -d)"
  guard_marker="$guard_dir/marker"
  guard_other="$guard_dir/other-marker"
  printf 'musehub-perf-guardtest\n' >"$guard_marker"
  printf 'musehub-perf-otherrun\n' >"$guard_other"
  # A matching marker alone must not authorise cleanup: the project also has to be validated by the
  # run that created it. The same real marker is accepted once that validation happened.
  PROJECT_VALIDATED=0
  if require_validated_project "musehub-perf-guardtest" "$guard_marker" 2>/dev/null; then
    printf 'self_check_failed marker_without_validation_accepted\n' >&2
    failures=$((failures + 1))
  fi
  PROJECT_VALIDATED=1
  if ! require_validated_project "musehub-perf-guardtest" "$guard_marker" 2>/dev/null; then
    printf 'self_check_failed valid_project_and_marker_rejected\n' >&2
    failures=$((failures + 1))
  fi
  if require_validated_project "musehub-product" "$guard_marker" 2>/dev/null; then
    printf 'self_check_failed foreign_project_accepted\n' >&2
    failures=$((failures + 1))
  fi
  if require_validated_project "musehub-perf-guardtest" "$guard_dir/missing-marker" 2>/dev/null; then
    printf 'self_check_failed missing_marker_accepted\n' >&2
    failures=$((failures + 1))
  fi
  if require_validated_project "musehub-perf-guardtest" "$guard_other" 2>/dev/null; then
    printf 'self_check_failed mismatched_marker_accepted\n' >&2
    failures=$((failures + 1))
  fi
  PROJECT_VALIDATED=0
  rm -rf "$guard_dir"
  local saved_run_id tag
  saved_run_id="${RUN_ID:-}"
  RUN_ID="20260101t000000z-abcdef"
  tag="$(image_tag_for musehub-server)"
  if [[ "$tag" != "musehub-perf-musehub-server:20260101t000000z-abcdef" ]]; then
    printf 'self_check_failed run_specific_image_tag_shape=%s\n' "$tag" >&2
    failures=$((failures + 1))
  fi
  RUN_ID="$saved_run_id"
  local body needle
  body="$(function_body write_env_and_override)"
  if [[ -z "$body" ]]; then
    printf 'self_check_failed write_env_and_override_not_found\n' >&2
    failures=$((failures + 1))
  else
    for needle in 'image: %s' 'RUN_IMAGE_LABEL_KEY' 'labels:'; do
      if [[ "$body" != *"$needle"* ]]; then
        printf 'self_check_failed override_missing=%s\n' "$needle" >&2
        failures=$((failures + 1))
      fi
    done
  fi
  local preflight_line trap_line build_line reserve_line
  reserve_line="$(grep -n '^reserve_run_path[[:space:]]' "$SELF" | head -n 1 | cut -d: -f1)"
  preflight_line="$(grep -n '^preflight_run[[:space:]]' "$SELF" | head -n 1 | cut -d: -f1)"
  trap_line="$(grep -n '^trap on_exit EXIT$' "$SELF" | head -n 1 | cut -d: -f1)"
  build_line="$(grep -n '^compose build$' "$SELF" | head -n 1 | cut -d: -f1)"
  if [[ -z "$reserve_line" || -z "$preflight_line" || -z "$trap_line" || -z "$build_line" ]]; then
    printf 'self_check_failed reserve_or_preflight_or_trap_or_build_call_missing\n' >&2
    failures=$((failures + 1))
  elif ((reserve_line >= preflight_line)) || ((preflight_line >= trap_line)) || ((preflight_line >= build_line)); then
    printf 'self_check_failed run_path_order reserve_line=%s preflight_line=%s trap_line=%s build_line=%s\n' \
      "$reserve_line" "$preflight_line" "$trap_line" "$build_line" >&2
    failures=$((failures + 1))
  fi
  local unsafe_pattern
  for unsafe_pattern in \
    'docker[[:space:]]+system[[:space:]]+prune' \
    'docker[[:space:]]+volume[[:space:]]+prune' \
    'docker[[:space:]]+network[[:space:]]+prune' \
    'docker[[:space:]]+container[[:space:]]+prune'; do
    if grep -Eq "$unsafe_pattern" "$SELF"; then
      printf 'self_check_failed unsafe_prune_present=%s\n' "$unsafe_pattern" >&2
      failures=$((failures + 1))
    fi
  done
  while IFS= read -r line; do
    [[ -n "$line" ]] || continue
    if [[ "$line" != *'--project-name "$PROJECT_NAME"'* ]]; then
      printf 'self_check_failed unscoped_compose_down=%s\n' "$line" >&2
      failures=$((failures + 1))
    fi
  done < <(grep -n 'docker compose .*down' "$SELF" | grep -v 'cleanup command' || true)
  # The Docker-root probes must never substitute a path when the daemon, the reported root or its
  # filesystem cannot be established. Only pure helpers are called here; Docker is not invoked.
  local probe_name probe_body probe_literal
  for probe_name in probe_docker_root preflight_docker_root_space probe_disk_type; do
    probe_body="$(function_body "$probe_name")"
    if [[ -z "$probe_body" ]]; then
      printf 'self_check_failed unknown_function=%s\n' "$probe_name" >&2
      failures=$((failures + 1))
      continue
    fi
    for probe_literal in '/var/lib/docker' 'target="/"'; do
      if [[ "$probe_body" == *"$probe_literal"* ]]; then
        printf 'self_check_failed docker_root_fallback_present function=%s literal=%s\n' \
          "$probe_name" "$probe_literal" >&2
        failures=$((failures + 1))
      fi
    done
  done
  local root_probe_dir
  root_probe_dir="$(mktemp -d)"
  if docker_root_free_kib "" >/dev/null 2>&1; then
    printf 'self_check_failed empty_docker_root_accepted\n' >&2
    failures=$((failures + 1))
  fi
  if docker_root_free_kib "relative/docker-root" >/dev/null 2>&1; then
    printf 'self_check_failed relative_docker_root_accepted\n' >&2
    failures=$((failures + 1))
  fi
  if docker_root_free_kib "$root_probe_dir/missing-root" >/dev/null 2>&1; then
    printf 'self_check_failed missing_docker_root_accepted\n' >&2
    failures=$((failures + 1))
  fi
  if ! docker_root_free_kib "$root_probe_dir" >/dev/null 2>&1; then
    printf 'self_check_failed existing_docker_root_rejected\n' >&2
    failures=$((failures + 1))
  fi
  rm -rf "$root_probe_dir"
  # The run-path claim must be atomic, and releasing it must never delete evidence.
  local reservation_parent reservation_dir saved_dir saved_owned
  reservation_parent="$(mktemp -d)"
  reservation_dir="$reservation_parent/isolated-reservation"
  RESERVATION_DIR=""
  RESERVATION_OWNED=0
  if ! reserve_run_path "$reservation_dir" >/dev/null; then
    printf 'self_check_failed run_path_reservation_refused\n' >&2
    failures=$((failures + 1))
  fi
  saved_dir="$RESERVATION_DIR"
  saved_owned="$RESERVATION_OWNED"
  if reserve_run_path "$reservation_dir" >/dev/null 2>&1; then
    printf 'self_check_failed duplicate_run_path_reservation_accepted\n' >&2
    failures=$((failures + 1))
  fi
  if [[ "$RESERVATION_DIR" != "$saved_dir" || "$RESERVATION_OWNED" != "$saved_owned" ]]; then
    printf 'self_check_failed refused_reservation_took_ownership\n' >&2
    failures=$((failures + 1))
  fi
  printf 'partial report\n' >"$reservation_dir/notes.txt"
  RESERVATION_DIR="$reservation_dir"
  RESERVATION_OWNED=1
  if release_run_reservation >/dev/null 2>&1; then
    printf 'self_check_failed non_empty_reservation_released\n' >&2
    failures=$((failures + 1))
  fi
  rm -f "$reservation_dir/notes.txt"
  RESERVATION_DIR="$reservation_dir"
  RESERVATION_OWNED=1
  if ! release_run_reservation >/dev/null; then
    printf 'self_check_failed empty_reservation_not_released\n' >&2
    failures=$((failures + 1))
  fi
  if [[ -e "$reservation_dir" ]]; then
    printf 'self_check_failed released_reservation_left_behind\n' >&2
    failures=$((failures + 1))
  fi
  RESERVATION_DIR=""
  RESERVATION_OWNED=0
  rm -rf "$reservation_parent"
  # Only run_benchmark decides whether scheduler job execution can happen; the self-check text
  # itself must not be grepped, or the check matches its own literal.
  body="$(function_body run_benchmark)"
  if [[ -z "$body" ]]; then
    printf 'self_check_failed run_benchmark_not_found\n' >&2
    failures=$((failures + 1))
  else
    if [[ "$body" == *"MUSEHUB_PERF_ALLOW_JOB_EXECUTION=1"* ]]; then
      printf 'self_check_failed scheduler_job_execution_enabled_in_run_benchmark\n' >&2
      failures=$((failures + 1))
    fi
    if [[ "$body" != *"-u MUSEHUB_PERF_ALLOW_JOB_EXECUTION"* ]]; then
      printf 'self_check_failed run_benchmark_does_not_clear_scheduler_optin\n' >&2
      failures=$((failures + 1))
    fi
  fi
  if ((failures > 0)); then
    printf 'self_check=FAIL count=%s\n' "$failures" >&2
    return 1
  fi
  printf 'self_check=ok memory_cap_total_mib=%s cap_mib=%s\n' "$total" "$CONTROL_PLANE_MEMORY_CAP_MIB"
  printf 'cleanup_guard=ok (invalid project, unvalidated project, foreign project, missing marker and mismatched marker are refused)\n'
  printf 'cleanup_scope=ok (label-scoped containers, volumes and networks only)\n'
  printf 'cleanup_images=ok (only %s-<service>:<run id> tags are removed, after containers are gone)\n' "$PROJECT_PREFIX"
  printf 'preflight=ok (project absence, run-specific image tags, a provable local Docker root and %s GiB free space are enforced before build)\n' "$DOCKER_ROOT_MIN_FREE_GIB"
  printf 'docker_root_guard=ok (a failed docker info, non-local DOCKER_HOST or context, a missing root or an uninspectable filesystem is refused without a fallback path; checked without invoking Docker)\n'
  printf 'run_path_reservation=ok (the report path is claimed with mkdir before the preflight, a duplicate claim is refused, and a failed preflight releases only the empty claim it created)\n'
  printf 'scheduler_job_execution=disabled\n'
  return 0
}

KEEP_PROJECT="${MUSEHUB_PERF_KEEP:-0}"
SELF_CHECK=0
while (($# > 0)); do
  case "$1" in
    --keep)
      KEEP_PROJECT=1
      ;;
    --self-check)
      SELF_CHECK=1
      ;;
    --help | -h)
      usage
      exit 0
      ;;
    *)
      die "unknown argument: $1 (use --help)"
      ;;
  esac
  shift
done

if [[ "$SELF_CHECK" == "1" ]]; then
  self_check
  exit 0
fi

[[ "$(memory_cap_total_mib)" -le "$CONTROL_PLANE_MEMORY_CAP_MIB" ]] ||
  die "control-plane memory limits $(memory_cap_total_mib) MiB exceed the ${CONTROL_PLANE_MEMORY_CAP_MIB} MiB cap"

if [[ -n "${MUSEHUB_PERF_RUN_ID:-}" ]]; then
  RUN_ID="$(lower "$MUSEHUB_PERF_RUN_ID")"
  validate_run_id "$RUN_ID" ||
    die "MUSEHUB_PERF_RUN_ID must match $RUN_ID_REGEX (no trailing or doubled dashes)"
else
  RUN_ID="$(new_run_id)"
  validate_run_id "$RUN_ID" || die "generated run identifier is invalid: $RUN_ID"
fi
PROJECT_NAME="$PROJECT_PREFIX-$RUN_ID"
validate_project_name "$PROJECT_NAME" || die "generated project identifier is invalid: $PROJECT_NAME"

command -v docker >/dev/null 2>&1 || die "docker is required"
command -v python3 >/dev/null 2>&1 || die "python3 is required"
docker compose version >/dev/null 2>&1 || die "docker compose v2 is required"

# Claim the report path before the Docker preflight: mkdir(2) is the reservation, so two invocations
# that share a MUSEHUB_PERF_RUN_ID cannot both reach the pull/build steps.
REPORT_DIR="$ROOT/.local/perf/runs/isolated-$RUN_ID"
reserve_run_path "$REPORT_DIR" ||
  die "refusing to run: run path $REPORT_DIR is already reserved (choose a new MUSEHUB_PERF_RUN_ID)"

# Refuse before the cleanup trap exists and before anything is pulled or built: the Docker root must
# be provable, the project name must be unused, the run-specific image tags must not exist, and the
# root needs headroom. A failed preflight releases only the empty claim created just above.
preflight_run || release_run_reservation_and_die "preflight failed; no image was pulled or built"

WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/musehub-perf.$RUN_ID.XXXXXXXX")"
chmod 700 "$WORK_DIR"
ENV_FILE="$WORK_DIR/perf.env"
OVERRIDE_FILE="$WORK_DIR/docker-compose.limits.yml"
PROJECT_MARKER="$WORK_DIR/project-name"
printf '%s\n' "$PROJECT_NAME" >"$PROJECT_MARKER"
chmod 600 "$PROJECT_MARKER"

printf 'isolated_api_performance project=%s run_id=%s\n' "$PROJECT_NAME" "$RUN_ID"
log "reports: $REPORT_DIR"
log "work dir: $WORK_DIR"

read -r HTTP_PORT GIT_SSH_PORT < <(free_ports 2) || die "could not allocate free ports"
[[ -n "$HTTP_PORT" && -n "$GIT_SSH_PORT" ]] || die "could not allocate free ports"
log "dynamic ports: http=$HTTP_PORT git_ssh=$GIT_SSH_PORT"

probe_cpu
probe_host_memory
probe_docker_root || die "docker root became unavailable after the preflight"
probe_disk_type
log "host: $VCPU vCPU ($CPU_MODEL), ${HOST_MEMORY_MIB} MiB memory"

write_env_and_override
PROJECT_VALIDATED=1
trap on_exit EXIT

compose config --quiet >/dev/null || die "compose configuration is invalid for project $PROJECT_NAME"
log "compose config validated (memory cap $(memory_cap_total_mib) MiB of ${CONTROL_PLANE_MEMORY_CAP_MIB} MiB)"

log "building with BuildKit"
compose build

log "starting stack"
compose up -d

ORIGIN="http://127.0.0.1:$HTTP_PORT"
wait_for_ready "$ORIGIN" "$READY_TIMEOUT_SECONDS" "$PROJECT_NAME" || die "stack did not become ready"
log "stack ready at $ORIGIN"

RUN_SLUG="$(printf '%s' "$RUN_ID" | tr -d '-' | cut -c1-8)"
[[ -n "$RUN_SLUG" ]] || die "could not derive a dataset slug from the run identifier"
seed_dataset "$ORIGIN" "$ORG" "$RUN_SLUG"
log "dataset prepared: $SEED_MEMBERS additional members, $SEED_REPOSITORIES repositories"

BENCHMARK_COMMAND="python3 scripts/benchmark-api.py --origin $ORIGIN --org $ORG --rps $BENCHMARK_RPS --duration-seconds $BENCHMARK_DURATION_SECONDS --min-members $BENCHMARK_MIN_MEMBERS --min-repositories $BENCHMARK_MIN_REPOSITORIES --control-plane-vcpu $VCPU --control-plane-memory-gib $((CONTROL_PLANE_MEMORY_CAP_MIB / 1024)) --ssd-declared $SSD_DECLARED --concurrent-jobs-declared $CONCURRENT_JOBS_DECLARED --output-prefix .local/perf/runs/isolated-$RUN_ID/api"
MUSEHUB_PERF_CAP_MIB="$CONTROL_PLANE_MEMORY_CAP_MIB" \
  MUSEHUB_PERF_RPS="$BENCHMARK_RPS" \
  MUSEHUB_PERF_DURATION="$BENCHMARK_DURATION_SECONDS" \
  MUSEHUB_PERF_SEED_MEMBERS="$SEED_MEMBERS" \
  MUSEHUB_PERF_SEED_REPOSITORIES="$SEED_REPOSITORIES" \
  MUSEHUB_PERF_MIN_MEMBERS="$BENCHMARK_MIN_MEMBERS" \
  MUSEHUB_PERF_MIN_REPOSITORIES="$BENCHMARK_MIN_REPOSITORIES" \
  MUSEHUB_PERF_VCPU="$VCPU" \
  MUSEHUB_PERF_CPU_MODEL="$CPU_MODEL" \
  MUSEHUB_PERF_HOST_MEMORY_MIB="$HOST_MEMORY_MIB" \
  MUSEHUB_PERF_DOCKER_ROOT="$DOCKER_ROOT_DIR" \
  MUSEHUB_PERF_BACKING_DEVICE="$BACKING_DEVICE" \
  MUSEHUB_PERF_BACKING_FS="$BACKING_FS" \
  MUSEHUB_PERF_ROTATIONAL="$DISK_ROTATIONAL" \
  MUSEHUB_PERF_DISK_TYPE="$DISK_TYPE" \
  MUSEHUB_PERF_SSD_DECLARED="$SSD_DECLARED" \
  write_environment_report "$REPORT_DIR" "$ORIGIN" "$BENCHMARK_COMMAND"

set +e
run_benchmark "$ORIGIN" "$REPORT_DIR"
BENCHMARK_STATUS=$?
set -e
if ((BENCHMARK_STATUS != 0)); then
  ERRORS=$((ERRORS + 1))
  warn "benchmark exited with status $BENCHMARK_STATUS; reports in $REPORT_DIR"
fi

if ((ERRORS > 0)); then
  printf 'isolated_api_performance=FAILED reports=%s\n' "$REPORT_DIR"
  exit 1
fi
printf 'isolated_api_performance=REPORTED reports=%s\n' "$REPORT_DIR"
printf 'Read the generated api.md for the pass/fail verdict; absence of a real report means the target is not met.\n'
