//! Product Git-over-SSH smoke (clone + feature push + protected main).

mod common;

use std::process::Command;

use common::git_ssh_fixture::git_ssh_fixture;
use common::git_ssh_util::{git_ssh_cmd_in, require_git_ssh_transport_prereqs};

#[tokio::test]
async fn git_ssh_transport_auth_and_protection() {
    require_git_ssh_transport_prereqs();
    let f = git_ssh_fixture("sshsmoke").await;
    let work_dir = tempfile::tempdir().expect("work");
    let work = work_dir.path();
    let clone = common::git_ssh_util::git_ssh_cmd(
        &f.priv_key,
        f.sshd.port,
        &["clone", &f.clone_url, work.to_str().unwrap()],
    );
    assert!(
        clone.status.success(),
        "clone failed: {}",
        String::from_utf8_lossy(&clone.stderr)
    );
    std::fs::write(work.join("ssh.txt"), b"via-ssh\n").unwrap();
    Command::new("git")
        .current_dir(work)
        .args(["checkout", "-b", "feature/ssh"])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(work)
        .args(["add", "ssh.txt"])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(work)
        .args([
            "-c",
            "user.email=e2e@musehub.local",
            "-c",
            "user.name=e2e",
            "commit",
            "-m",
            "ssh",
            "--no-gpg-sign",
        ])
        .status()
        .unwrap();
    let push = git_ssh_cmd_in(
        &f.priv_key,
        f.sshd.port,
        Some(work),
        &["push", "origin", "HEAD:refs/heads/feature/ssh"],
    );
    assert!(
        push.status.success(),
        "push: {}",
        String::from_utf8_lossy(&push.stderr)
    );
    let push_main = git_ssh_cmd_in(
        &f.priv_key,
        f.sshd.port,
        Some(work),
        &["push", "origin", "HEAD:main"],
    );
    assert!(
        !push_main.status.success(),
        "protected main push should fail"
    );
}
