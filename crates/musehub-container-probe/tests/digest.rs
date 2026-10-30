//! Integration tests for the digest-pinned container probe.
//!
//! These tests talk to the local Docker Engine through the same absolute binary
//! paths as the crate. They never pull an image: `busybox:1.36.1` is already present
//! locally under the digest below and every probe disables pulling.
//!
//! If the Docker daemon is unavailable the tests fail with the `docker info` stderr;
//! they are not skipped.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use musehub_container_probe::{
    compose_container_id, compose_digest_probe, compose_down, compose_up, container_image_id,
    container_mount_sources, image_id_for_digest, run_digest, run_digest_with_docker,
    write_compose_file, DOCKER_BIN, PROBE_OUTPUT, SCOPE,
};
use musehub_core::ErrorCode;

/// `busybox:1.36.1` as already stored by the local Engine.
const BUSYBOX_DIGEST: &str =
    "busybox@sha256:73aaf090f3d85aa34ee199857f03fa3a95c8ede2ffd4cc2cdb5b94e566b11662";

/// Removes its directory when the test ends.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = unique_temp_path(tag);
        fs::create_dir_all(&path).expect("create temporary directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Temporary Compose project that is always torn down.
struct ComposeProject {
    dir: TempDir,
    name: String,
}

impl ComposeProject {
    fn new(tag: &str) -> Self {
        Self {
            dir: TempDir::new(tag),
            name: format!("musehub-probe-{}-{tag}", std::process::id()),
        }
    }

    fn dir(&self) -> &Path {
        self.dir.path()
    }

    fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for ComposeProject {
    fn drop(&mut self) {
        let _ = compose_down(self.dir(), self.name());
    }
}

fn unique_temp_path(tag: &str) -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "musehub-container-probe-{}-{tag}-{nanos}-{sequence}",
        std::process::id()
    ))
}

/// `docker inspect --format FORMAT TARGET`, run independently of the crate helpers.
fn docker_inspect(format: &str, target: &str) -> String {
    let output = docker(&["inspect", "--format", format, target]);
    assert_success(&output, "docker inspect");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// `docker image inspect --format FORMAT DIGEST`, run independently of the crate.
fn docker_image_inspect(format: &str, image_digest: &str) -> String {
    let output = docker(&["image", "inspect", "--format", format, image_digest]);
    assert_success(&output, "docker image inspect");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn docker(args: &[&str]) -> Output {
    Command::new(DOCKER_BIN)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("run {DOCKER_BIN} {args:?}: {error}"))
}

fn assert_success(output: &Output, step: &str) {
    assert!(
        output.status.success(),
        "{step} failed with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn socket_arguments_are_rejected_before_docker_runs() {
    let temp = TempDir::new("reject");
    let marker = temp.path().join("docker-ran");
    let sentinel = temp.path().join("sentinel-docker.sh");
    fs::write(
        &sentinel,
        format!("#!/bin/sh\n: > '{}'\nexit 0\n", marker.display()),
    )
    .expect("write sentinel docker");
    let mut permissions = fs::metadata(&sentinel)
        .expect("sentinel metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&sentinel, permissions).expect("mark sentinel executable");

    let error = run_digest_with_docker(
        &sentinel,
        BUSYBOX_DIGEST,
        &["-v", "/var/run/docker.sock:/var/run/docker.sock"],
    )
    .expect_err("a socket mount argument must be rejected");
    assert_eq!(error.code(), "container_probe_failed");
    assert!(error.to_string().contains("docker.sock"), "{error}");
    assert!(
        !marker.exists(),
        "docker must not run when the arguments mount the Docker socket"
    );

    let flag_error = run_digest_with_docker(&sentinel, BUSYBOX_DIGEST, &["--privileged"])
        .expect_err("a privileged flag must be rejected");
    assert_eq!(flag_error.code(), "container_probe_failed");
    assert!(
        flag_error.to_string().contains("--privileged"),
        "{flag_error}"
    );
    assert!(
        !marker.exists(),
        "docker must not run when a privileged flag is forwarded"
    );
}

#[test]
fn run_digest_executes_the_pinned_image_without_a_socket() {
    let report = run_digest(BUSYBOX_DIGEST, &["printf", PROBE_OUTPUT])
        .unwrap_or_else(|error| panic!("docker run probe failed: {error}"));

    assert_eq!(report.scope, SCOPE);
    assert_eq!(report.image_digest, BUSYBOX_DIGEST);
    assert_eq!(report.exit_code, Some(0), "stderr: {}", report.stderr);
    assert_eq!(report.stdout, PROBE_OUTPUT);
    assert!(!report.socket_mounted);
    assert_eq!(
        report.running_image_id, None,
        "--rm deletes the container before it can be inspected"
    );
    assert_eq!(
        image_id_for_digest(BUSYBOX_DIGEST).expect("image id for digest"),
        docker_image_inspect("{{.Id}}", BUSYBOX_DIGEST)
    );
}

#[test]
fn compose_probe_reports_the_image_id_that_actually_ran() {
    let project = ComposeProject::new("digest");
    let report = compose_digest_probe(project.dir(), project.name(), BUSYBOX_DIGEST)
        .unwrap_or_else(|error| panic!("compose digest probe failed: {error}"));

    assert_eq!(report.scope, SCOPE);
    assert_eq!(report.scope, "colima_docker_vm");
    assert_eq!(report.project_name, project.name());
    assert_eq!(report.image_digest, BUSYBOX_DIGEST);
    assert_eq!(report.exit_code, Some(0), "stderr: {}", report.stderr);
    assert!(!report.socket_mounted);

    let independent_image_id = docker_image_inspect("{{.Id}}", BUSYBOX_DIGEST);
    assert_eq!(report.expected_image_id, independent_image_id);
    assert_eq!(
        report.running_image_id, independent_image_id,
        "running image ID must equal the digest image ID"
    );
    assert!(
        report.combined_output().contains(PROBE_OUTPUT),
        "probe output missing: {}",
        report.combined_output()
    );
}

#[test]
fn running_container_mounts_no_docker_socket_before_cleanup() {
    let project = ComposeProject::new("mounts");
    write_compose_file(project.dir(), BUSYBOX_DIGEST).expect("write compose file");
    let manifest =
        fs::read_to_string(project.dir().join("compose.yaml")).expect("read compose file");
    assert!(
        manifest.contains(&format!("image: {BUSYBOX_DIGEST}")),
        "{manifest}"
    );
    assert!(!manifest.contains("volumes"), "{manifest}");
    assert!(!manifest.contains("privileged"), "{manifest}");
    assert!(!manifest.contains("docker.sock"), "{manifest}");

    let up = compose_up(project.dir(), project.name()).expect("docker-compose up");
    assert_eq!(up.exit_code, Some(0), "stderr: {}", up.stderr);

    let container_id =
        compose_container_id(project.dir(), project.name()).expect("compose container id");

    // The container has exited but still exists, so inspect before `down` removes it.
    let mounts = container_mount_sources(&container_id).expect("container mount sources");
    assert!(
        mounts
            .iter()
            .all(|source| !source.to_lowercase().contains("docker.sock")),
        "mounts must not include the Docker socket: {mounts:?}"
    );
    let raw_mounts = docker_inspect("{{json .Mounts}}", &container_id);
    assert!(
        !raw_mounts.contains("docker.sock"),
        "docker inspect reports a socket mount: {raw_mounts}"
    );
    assert_eq!(
        docker_inspect("{{.HostConfig.Privileged}}", &container_id),
        "false"
    );
    assert_eq!(
        docker_inspect("{{.HostConfig.NetworkMode}}", &container_id),
        "none"
    );
    assert_eq!(
        docker_inspect("{{.Config.Image}}", &container_id),
        BUSYBOX_DIGEST
    );
    assert_eq!(
        container_image_id(&container_id).expect("container image id"),
        docker_image_inspect("{{.Id}}", BUSYBOX_DIGEST)
    );

    compose_down(project.dir(), project.name()).expect("docker-compose down");
}
