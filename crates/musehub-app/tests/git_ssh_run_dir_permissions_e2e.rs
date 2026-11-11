//! Production run-dir ACLs: git can traverse/read runtime.env, not write secrets.

#![cfg(unix)]

mod common;

use std::os::unix::fs::MetadataExt;
use std::process::Command;

use common::git_ssh_fixture::git_ssh_fixture;
use common::git_ssh_util::require_git_ssh_transport_prereqs;

#[tokio::test]
async fn git_ssh_run_dir_default_permissions_for_git_user() {
    require_git_ssh_transport_prereqs();
    let f = git_ssh_fixture("sshdirperm").await;
    let run_dir = &f.sshd.run_dir;
    let runtime = run_dir.join("runtime.env");
    assert!(
        Command::new("sudo")
            .args(["test", "-f", runtime.to_str().unwrap()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
        "runtime.env must exist after serve"
    );

    let mode_line = String::from_utf8(
        Command::new("stat")
            .args(["-c", "%a", run_dir.to_str().unwrap()])
            .output()
            .expect("stat run_dir")
            .stdout,
    )
    .unwrap();
    let mode = u32::from_str_radix(mode_line.trim(), 8).expect("octal mode");
    let meta = std::fs::metadata(run_dir).expect("run_dir");
    assert_eq!(meta.uid(), 0, "run dir root-owned");
    assert_eq!(
        mode & 0o007,
        0,
        "run dir not world-accessible (mode={mode:o})"
    );
    assert_eq!(mode & 0o002, 0, "run dir not world-writable");
    assert_eq!(mode & 0o020, 0, "run dir not group-writable");
    assert!(mode & 0o050 != 0, "run dir grants group traverse");

    let rmode_line = String::from_utf8(
        Command::new("sudo")
            .args(["stat", "-c", "%a", runtime.to_str().unwrap()])
            .output()
            .expect("stat runtime.env")
            .stdout,
    )
    .unwrap();
    let rmode = u32::from_str_radix(rmode_line.trim(), 8).expect("runtime octal mode");
    assert_eq!(rmode & 0o007, 0);
    assert_eq!(rmode, 0o640, "runtime.env mode 0640");

    assert!(
        Command::new("sudo")
            .args(["-u", "git", "test", "-x", run_dir.to_str().unwrap()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
        "git must traverse run dir"
    );
    assert!(
        Command::new("sudo")
            .args(["-u", "git", "test", "-r", runtime.to_str().unwrap()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
        "git must read runtime.env"
    );
    assert!(
        !Command::new("sudo")
            .args(["-u", "git", "test", "-w", runtime.to_str().unwrap()])
            .status()
            .map(|s| s.success())
            .unwrap_or(true),
        "git must not write runtime.env"
    );
    assert!(
        !Command::new("sudo")
            .args(["-u", "git", "test", "-w", run_dir.to_str().unwrap()])
            .status()
            .map(|s| s.success())
            .unwrap_or(true),
        "git must not write run dir"
    );

    let nobody = Command::new("sudo")
        .args(["-u", "nobody", "test", "-r", runtime.to_str().unwrap()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(!nobody, "unrelated uid must not read runtime.env");
}
