use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use musehub_core::ErrorCode;
use musehub_runner_probe::{run_confined, spawn_confined, RunnerProbeError, SCOPE};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("musehub-runner-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).expect("create workspace");
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn sh() -> &'static std::path::Path {
    std::path::Path::new("/bin/sh")
}

#[test]
fn workspace_write_succeeds_but_parent_path_is_rejected() {
    let workspace = TempDir::new();
    let parent = workspace
        .path()
        .parent()
        .expect("temp parent")
        .to_path_buf();
    let outside = parent.join(format!("outside-{}.txt", std::process::id()));
    let _ = fs::remove_file(&outside);

    let error = run_confined(
        workspace.path(),
        sh(),
        &["-c", "printf marker > ../outside.txt"],
        &HashMap::new(),
        Duration::from_millis(500),
    )
    .expect_err("parent traversal must be rejected");

    assert_eq!(error.code(), "runner_probe_failed");
    assert!(
        !outside.exists(),
        "rejected command must not create {outside:?}"
    );
    assert!(
        matches!(error, RunnerProbeError::Failed { step, .. } if step == "reject workspace escape")
    );

    let report = run_confined(
        workspace.path(),
        sh(),
        &["-c", "printf marker > inside.txt"],
        &HashMap::new(),
        Duration::from_millis(500),
    )
    .expect("workspace write");

    assert_eq!(report.scope, SCOPE);
    assert_eq!(report.exit_code, Some(0));
    assert_eq!(
        fs::read_to_string(workspace.path().join("inside.txt")).expect("inside file"),
        "marker"
    );
}

#[test]
fn timeout_terminates_the_process() {
    let workspace = TempDir::new();
    let started = std::time::Instant::now();
    let error = run_confined(
        workspace.path(),
        sh(),
        &["-c", "while true; do sleep 1; done"],
        &HashMap::new(),
        Duration::from_millis(300),
    )
    .expect_err("loop must time out");

    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(error.code(), "runner_probe_failed");
    let message = error.to_string();
    assert!(message.contains("process timeout"), "{message}");
    assert!(message.contains("terminated"), "{message}");
}

#[test]
fn cancel_stops_the_process_before_it_writes_a_marker() {
    let workspace = TempDir::new();
    let running = spawn_confined(
        workspace.path(),
        sh(),
        &["-c", "sleep 30; printf late > marker.txt"],
        &HashMap::new(),
    )
    .expect("spawn");

    std::thread::sleep(Duration::from_millis(50));
    let report = running.cancel().expect("cancel");
    std::thread::sleep(Duration::from_millis(100));

    assert!(report.canceled);
    assert_eq!(report.scope, SCOPE);
    assert_ne!(report.exit_code, Some(0));
    assert!(!workspace.path().join("marker.txt").exists());
}
