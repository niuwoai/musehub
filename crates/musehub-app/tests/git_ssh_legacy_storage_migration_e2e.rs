//! Legacy git volume: fail-closed migration to group-shared bare repos.

mod common;

use std::process::Command;

use musehub_app::bare_repo_path;
use musehub_app::git_ssh_runtime::ensure_shared_git_storage_root;
use uuid::Uuid;

#[tokio::test]
async fn legacy_bare_repo_migrated_to_group_shared_layout() {
    common::git_ssh_sshd::ensure_git_os_user();
    let git_root = tempfile::tempdir().expect("git root");
    let storage_key = Uuid::new_v4();
    let bare = bare_repo_path(git_root.path(), &storage_key);
    assert!(Command::new("git")
        .args(["init", "--bare", bare.to_str().unwrap()])
        .status()
        .unwrap()
        .success());

    let root_owned = Command::new("sudo")
        .args(["chown", "-R", "root:root", bare.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(root_owned.success());

    std::env::remove_var("MUSEHUB_GIT_STORAGE_SUDO_FALLBACK");
    std::env::set_var("MUSEHUB_GIT_STORAGE_GROUP", "musehub-git");
    let err = ensure_shared_git_storage_root(git_root.path()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("chgrp") || msg.contains("git storage root") || msg.contains("group"),
        "expected actionable migration error, got: {msg}"
    );

    std::env::set_var("MUSEHUB_GIT_STORAGE_SUDO_FALLBACK", "1");
    ensure_shared_git_storage_root(git_root.path()).expect("migration with sudo fallback");

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(bare.join("objects")).expect("objects dir");
        assert_ne!(meta.gid(), 0, "objects must be group-owned after migration");
        assert_ne!(meta.mode() & 0o020, 0, "objects must be group-writable");
    }
    let group = std::env::var("MUSEHUB_GIT_STORAGE_GROUP").unwrap_or_else(|_| "musehub-git".into());
    let objects = bare.join("objects").to_str().unwrap().to_string();
    let can_write = Command::new("sudo")
        .args(["-n", "-u", "git", "-g", &group, "test", "-w", &objects])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(can_write, "git user must write objects after migration");

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
    assert_eq!(String::from_utf8_lossy(&shared.stdout).trim(), "group");
}
