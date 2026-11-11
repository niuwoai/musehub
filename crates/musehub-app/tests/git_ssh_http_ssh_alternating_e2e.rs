//! SSH receive-pack after repeated pushes; bare repo keeps group-shared layout.

mod common;

use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use common::git_ssh_fixture::git_ssh_fixture;
use common::git_ssh_util::{git_ssh_cmd_in, require_git_ssh_transport_prereqs};

#[tokio::test]
async fn git_ssh_repeated_push_retains_group_shared_objects() {
    require_git_ssh_transport_prereqs();
    let f = git_ssh_fixture("sshalt").await;

    let storage_row: (uuid::Uuid,) =
        sqlx::query_as("SELECT storage_key FROM repositories WHERE slug = $1")
            .bind(&f.repo)
            .fetch_one(&f.pool)
            .await
            .expect("storage key");
    let bare = musehub_app::bare_repo_path(&f.git_root, &storage_row.0);

    let work = tempfile::tempdir().unwrap();
    git_ssh_cmd_in(
        &f.priv_key,
        f.sshd.port,
        None,
        &["clone", &f.clone_url, work.path().to_str().unwrap()],
    );
    for (i, msg) in ["ssh-a", "ssh-b", "ssh-c"].iter().enumerate() {
        std::fs::write(work.path().join("alt.txt"), format!("line {i}")).unwrap();
        Command::new("git")
            .current_dir(work.path())
            .args(["add", "alt.txt"])
            .status()
            .unwrap();
        Command::new("git")
            .current_dir(work.path())
            .args(["commit", "-m", msg])
            .status()
            .unwrap();
        git_ssh_cmd_in(
            &f.priv_key,
            f.sshd.port,
            Some(work.path()),
            &["push", "origin", "main"],
        );
    }

    let shared = Command::new("git")
        .args([
            "--git-dir",
            bare.to_str().unwrap(),
            "config",
            "--get",
            "core.sharedRepository",
        ])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&shared.stdout).trim(),
        "group",
        "bare repo must use shared repository mode"
    );

    let mut any_dir = false;
    for entry in std::fs::read_dir(bare.join("objects")).expect("objects") {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            #[cfg(unix)]
            {
                let mode = entry.metadata().unwrap().permissions().mode() & 0o077;
                assert_ne!(mode & 0o040, 0, "object shard dirs group-readable");
            }
            any_dir = true;
        }
    }
    assert!(any_dir);
}
