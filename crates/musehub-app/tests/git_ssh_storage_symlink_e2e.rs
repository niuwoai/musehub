//! Storage migration must not follow *.git symlinks outside the volume.

mod common;

use std::fs;
use std::process::Command;

use musehub_app::bare_repo_path;
use musehub_app::git_ssh_runtime::ensure_shared_git_storage_root;
use uuid::Uuid;

#[tokio::test]
async fn migrate_rejects_external_bare_symlink_without_touching_sentinel() {
    common::git_ssh_sshd::ensure_git_os_user();
    let git_root = tempfile::tempdir().expect("git root");
    let sentinel = tempfile::tempdir().expect("sentinel");
    let marker = sentinel.path().join("marker.txt");
    fs::write(&marker, b"untouched\n").expect("marker");
    let evil = git_root.path().join("evil.git");
    std::os::unix::fs::symlink(sentinel.path(), &evil).expect("symlink");

    std::env::set_var("MUSEHUB_GIT_STORAGE_GROUP", "musehub-git");
    std::env::set_var("MUSEHUB_GIT_STORAGE_SUDO_FALLBACK", "1");
    let err = ensure_shared_git_storage_root(git_root.path()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("symlink"),
        "expected symlink rejection, got: {msg}"
    );

    let body = fs::read_to_string(&marker).expect("sentinel still readable");
    assert_eq!(body, "untouched\n");
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(&marker).expect("stat").permissions().mode() & 0o777
    };
    assert_eq!(mode, 0o644, "sentinel mode unchanged");
}

#[tokio::test]
async fn migrate_rejects_nested_symlink_inside_bare_repo() {
    common::git_ssh_sshd::ensure_git_os_user();
    let git_root = tempfile::tempdir().expect("git root");
    let storage_key = Uuid::new_v4();
    let bare = bare_repo_path(git_root.path(), &storage_key);
    assert!(Command::new("git")
        .args(["init", "--bare", bare.to_str().unwrap()])
        .status()
        .unwrap()
        .success());

    let outside = tempfile::tempdir().expect("outside");
    let nested = bare.join("objects").join("escape");
    fs::create_dir_all(nested.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(outside.path(), &nested).expect("nested symlink");

    std::env::set_var("MUSEHUB_GIT_STORAGE_GROUP", "musehub-git");
    std::env::set_var("MUSEHUB_GIT_STORAGE_SUDO_FALLBACK", "1");
    let err = ensure_shared_git_storage_root(git_root.path()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("symlink") || msg.contains("nested"),
        "expected nested symlink rejection, got: {msg}"
    );
}
