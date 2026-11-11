use std::process::Command;

use musehub_app::bare_repo_path;
use uuid::Uuid;

pub fn push_branch(
    git_root: &std::path::Path,
    storage_key: Uuid,
    branch: &str,
    file: &str,
    content: &[u8],
) -> String {
    let bare = bare_repo_path(git_root, &storage_key);
    let work = tempfile::tempdir().unwrap();
    assert!(Command::new("git")
        .args([
            "clone",
            bare.to_str().unwrap(),
            work.path().to_str().unwrap(),
        ])
        .status()
        .unwrap()
        .success());
    let git = |args: &[&str]| {
        assert!(Command::new("git")
            .current_dir(work.path())
            .args(args)
            .status()
            .unwrap()
            .success());
    };
    if !Command::new("git")
        .current_dir(work.path())
        .args(["checkout", branch])
        .status()
        .unwrap()
        .success()
    {
        git(&["checkout", "-b", branch]);
    }
    if let Some(parent) = work.path().join(file).parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(work.path().join(file), content).unwrap();
    git(&["add", file]);
    git(&[
        "-c",
        "user.email=itest@musehub.local",
        "-c",
        "user.name=itest",
        "commit",
        "-m",
        "s1",
    ]);
    git(&["push", "origin", branch]);
    let out = Command::new("git")
        .current_dir(work.path())
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}
