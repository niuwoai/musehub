//! Integration tests for the rootless Docker probe.
//!
//! These tests run the real probe: they reach the Linux VM with
//! `/opt/homebrew/bin/colima ssh`, talk to the rootless Engine at
//! `unix:///run/user/502/docker.sock`, build `musehub-rootless-probe:local` from
//! stdin, and start `busybox:1.36.1`, which is already present locally.
//!
//! Nothing is skipped. If Colima, the VM, or the rootless daemon is unavailable
//! the test fails with the captured stderr of the failing command.

use std::process::{Command, Output};

use musehub_core::ErrorCode;
use musehub_rootless_probe::{
    probe, ProbeConfig, PROBE_OUTPUT, PROBE_TAG, ROOTLESS_DOCKER_HOST, SCOPE, SOCKET_CHECK_SCRIPT,
};

/// Runs `colima ssh -- env DOCKER_HOST=... docker ARGS` with an argument array.
fn colima_docker(args: &[&str]) -> Output {
    Command::new(musehub_rootless_probe::COLIMA_BIN)
        .arg("ssh")
        .arg("--")
        .arg("env")
        .arg(format!("DOCKER_HOST={ROOTLESS_DOCKER_HOST}"))
        .arg("docker")
        .args(args)
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "run {} ssh -- env DOCKER_HOST={ROOTLESS_DOCKER_HOST} docker {args:?}: {error}",
                musehub_rootless_probe::COLIMA_BIN
            )
        })
}

fn assert_success(output: &Output, step: &str) {
    assert!(
        output.status.success(),
        "{step} failed with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn probe_reports_rootless_engine_and_socket_absence() {
    let config = ProbeConfig::default();
    let report = probe(&config).unwrap_or_else(|error| {
        panic!(
            "rootless probe failed ({}): {error}\n\
             hint: start the Colima VM with the rootless Engine and run `colima ssh -- env DOCKER_HOST={ROOTLESS_DOCKER_HOST} docker info`",
            error.code()
        )
    });

    assert_eq!(report.scope, SCOPE);
    assert_eq!(report.scope, "linux_rootless_colima");
    assert!(report.rootless, "the Engine must report name=rootless");
    assert!(
        report.docker_root_dir.starts_with("/home/"),
        "DockerRootDir must be a user data directory, got {:?}",
        report.docker_root_dir
    );
    assert_ne!(report.docker_root_dir, "/var/lib/docker");
    assert!(
        !report.socket_visible_in_container,
        "a container must not see /var/run/docker.sock"
    );
    assert_eq!(report.build_exit_code, Some(0));

    // Independent verification: the same Engine and the same images, queried by
    // this test instead of by the crate.
    let info = colima_docker(&["info", "--format", "{{json .}}"]);
    assert_success(&info, "docker info");
    let info_json = stdout_of(&info);
    assert!(
        info_json.contains("\"name=rootless\""),
        "SecurityOptions do not contain name=rootless: {info_json}"
    );
    assert!(
        !info_json.contains("\"DockerRootDir\":\"/var/lib/docker\""),
        "the default rootful directory must not be used: {info_json}"
    );
    assert!(
        info_json.contains("\"DockerRootDir\":\"/home/"),
        "DockerRootDir is not a user home directory: {info_json}"
    );

    let built = colima_docker(&[
        "run",
        "--rm",
        "--network",
        "none",
        PROBE_TAG,
        "cat",
        "/probe.txt",
    ]);
    assert_success(&built, "docker run built image");
    assert_eq!(stdout_of(&built), PROBE_OUTPUT);

    let socket = colima_docker(&[
        "run",
        "--rm",
        "--network",
        "none",
        "--entrypoint",
        "/bin/sh",
        "busybox:1.36.1",
        "-c",
        SOCKET_CHECK_SCRIPT,
    ]);
    assert_success(&socket, "docker run socket check");
    assert_eq!(stdout_of(&socket), "no-socket");
}

#[test]
fn unreachable_rootless_daemon_fails_with_stderr_instead_of_skipping() {
    let config = ProbeConfig {
        docker_host: "unix:///run/user/502/absent-docker.sock".to_string(),
        ..ProbeConfig::default()
    };
    let error = probe(&config).expect_err("an unreachable daemon must fail the probe");

    assert_eq!(error.code(), "rootless_probe_failed");
    let message = error.to_string();
    assert!(message.contains("docker info"), "{message}");
    assert!(
        message.contains("absent-docker.sock") || message.contains("Cannot connect"),
        "the failure must keep the command or the daemon stderr: {message}"
    );
}

#[test]
fn missing_colima_binary_fails_inside_the_probe() {
    let config = ProbeConfig {
        colima_bin: "/nonexistent/colima".into(),
        ..ProbeConfig::default()
    };
    let error = probe(&config).expect_err("a missing Colima binary must fail the probe");

    assert_eq!(error.code(), "rootless_probe_failed");
    let message = error.to_string();
    assert!(message.contains("/nonexistent/colima"), "{message}");
}
