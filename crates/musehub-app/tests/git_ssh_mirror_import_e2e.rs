//! Real Git-over-SSH validation for initial mirror import and protected refs.

mod common;

use std::process::Command;

use common::git_ssh_fixture::git_ssh_fixture;
use common::git_ssh_util::git_ssh_cmd_in;
use musehub_app::bare_repo_path;

fn git_ok(cwd: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .status()
        .expect("run git command");
    assert!(status.success(), "git {} failed", args.join(" "));
}

fn server_ref(bare: &std::path::Path, ref_name: &str) -> String {
    let output = Command::new("git")
        .args([
            "--git-dir",
            bare.to_str().expect("bare path"),
            "rev-parse",
            ref_name,
        ])
        .output()
        .expect("read server ref");
    assert!(output.status.success(), "server ref {ref_name} is missing");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[tokio::test]
async fn ssh_mirror_import_replaces_seed_once_and_protects_internal_refs() {
    let fixture = git_ssh_fixture("mirror").await;
    let repo_id: (uuid::Uuid,) =
        sqlx::query_as("SELECT storage_key FROM repositories WHERE slug = $1")
            .bind(&fixture.repo)
            .fetch_one(&fixture.pool)
            .await
            .expect("repository storage key");
    let bare = bare_repo_path(&fixture.git_root, &repo_id.0);
    let seed_oid = server_ref(&bare, "refs/heads/main");

    let source = tempfile::tempdir().expect("source repository");
    git_ok(source.path(), &["init", "-b", "main"]);
    std::fs::write(
        source.path().join("imported.txt"),
        "initial mirror import\n",
    )
    .expect("write source file");
    git_ok(source.path(), &["add", "imported.txt"]);
    git_ok(
        source.path(),
        &[
            "-c",
            "user.email=e2e@musehub.local",
            "-c",
            "user.name=e2e",
            "commit",
            "-m",
            "initial import",
            "--no-gpg-sign",
        ],
    );
    let imported_oid = String::from_utf8_lossy(
        &Command::new("git")
            .current_dir(source.path())
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("read source commit")
            .stdout,
    )
    .trim()
    .to_string();
    assert_ne!(imported_oid, seed_oid);

    git_ok(
        source.path(),
        &["remote", "add", "origin", &fixture.clone_url],
    );
    let import = git_ssh_cmd_in(
        &fixture.priv_key,
        fixture.sshd.port,
        Some(source.path()),
        &["push", "--mirror", "origin"],
    );
    assert!(import.status.success(), "initial SSH mirror import failed");
    assert_eq!(server_ref(&bare, "refs/heads/main"), imported_oid);

    let internal_ref = "refs/musehub/merge-staging/attacker";
    let update_internal = Command::new("git")
        .args([
            "--git-dir",
            bare.to_str().expect("bare path"),
            "update-ref",
            internal_ref,
            &seed_oid,
        ])
        .status()
        .expect("create internal server ref");
    assert!(update_internal.success());

    let advertised = git_ssh_cmd_in(
        &fixture.priv_key,
        fixture.sshd.port,
        Some(source.path()),
        &["ls-remote", "origin"],
    );
    assert!(advertised.status.success(), "SSH ls-remote failed");
    assert!(!String::from_utf8_lossy(&advertised.stdout).contains("refs/musehub/"));

    git_ok(source.path(), &["update-ref", internal_ref, &imported_oid]);
    let write_internal = git_ssh_cmd_in(
        &fixture.priv_key,
        fixture.sshd.port,
        Some(source.path()),
        &["push", "origin", &format!("{internal_ref}:{internal_ref}")],
    );
    assert!(!write_internal.status.success());
    assert_eq!(server_ref(&bare, internal_ref), seed_oid);
    assert_eq!(server_ref(&bare, "refs/heads/main"), imported_oid);
    git_ok(source.path(), &["update-ref", "-d", internal_ref]);

    std::fs::write(source.path().join("later.txt"), "subsequent change\n")
        .expect("write later file");
    git_ok(source.path(), &["add", "later.txt"]);
    git_ok(
        source.path(),
        &[
            "-c",
            "user.email=e2e@musehub.local",
            "-c",
            "user.name=e2e",
            "commit",
            "-m",
            "later change",
            "--no-gpg-sign",
        ],
    );
    let later_oid = String::from_utf8_lossy(
        &Command::new("git")
            .current_dir(source.path())
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("read later commit")
            .stdout,
    )
    .trim()
    .to_string();
    let second_mirror = git_ssh_cmd_in(
        &fixture.priv_key,
        fixture.sshd.port,
        Some(source.path()),
        &["push", "--mirror", "origin"],
    );
    assert!(!second_mirror.status.success());
    assert_eq!(server_ref(&bare, "refs/heads/main"), imported_oid);
    assert_ne!(later_oid, imported_oid);
}
