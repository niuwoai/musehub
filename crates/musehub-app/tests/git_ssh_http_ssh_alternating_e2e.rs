//! Alternating SSH and smart HTTP writes on a feature branch (not protected main).

mod common;

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use base64::Engine;
use musehub_app::bare_repo_path;
use tokio::net::TcpListener;
use tower::ServiceExt;

use common::git_http_pat::GitHttpPatSandbox;
use common::git_ssh_fixture::git_ssh_fixture;
use common::git_ssh_util::{git_ssh_cmd_in, require_git_ssh_transport_prereqs};

fn ref_oid(bare: &std::path::Path, ref_name: &str) -> String {
    let out = Command::new("git")
        .args(["--git-dir", bare.to_str().unwrap(), "rev-parse", ref_name])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn fail_git_push(work: &std::path::Path, branch: &str, priv_key: &std::path::Path, port: u16) {
    let push = git_ssh_cmd_in(
        priv_key,
        port,
        Some(work),
        &["push", "origin", &format!("HEAD:refs/heads/{branch}")],
    );
    assert!(!push.status.success());
}

#[tokio::test]
async fn git_ssh_http_transport_alternation_on_feature_branch() {
    require_git_ssh_transport_prereqs();
    let f = git_ssh_fixture("sshalt").await;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serve_app = f.app.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, serve_app).await;
    });

    let repo_id: (uuid::Uuid,) = sqlx::query_as("SELECT id FROM repositories WHERE slug = $1")
        .bind(&f.repo)
        .fetch_one(&f.pool)
        .await
        .expect("repo id");
    let pat = GitHttpPatSandbox::mint(
        &f.app,
        &f.cookie,
        "demo",
        repo_id.0,
        &["repo:read", "repo:write"],
    )
    .await;
    let http_base = format!("http://{addr}");
    let http_url = GitHttpPatSandbox::credential_free_url(&http_base, "demo", &f.repo);

    let storage_row: (uuid::Uuid,) =
        sqlx::query_as("SELECT storage_key FROM repositories WHERE slug = $1")
            .bind(&f.repo)
            .fetch_one(&f.pool)
            .await
            .expect("storage key");
    let bare = bare_repo_path(&f.git_root, &storage_row.0);
    let main_before = ref_oid(&bare, "refs/heads/main");
    let feature = "feature/transport-alt";

    let ssh_work = tempfile::tempdir().unwrap();
    let clone = common::git_ssh_util::git_ssh_cmd(
        &f.priv_key,
        f.sshd.port,
        &["clone", &f.clone_url, ssh_work.path().to_str().unwrap()],
    );
    assert!(
        clone.status.success(),
        "{}",
        String::from_utf8_lossy(&clone.stderr)
    );

    std::fs::write(ssh_work.path().join("main-deny.txt"), b"deny\n").unwrap();
    Command::new("git")
        .current_dir(ssh_work.path())
        .args(["add", "main-deny.txt"])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(ssh_work.path())
        .args([
            "-c",
            "user.email=e2e@musehub.local",
            "-c",
            "user.name=e2e",
            "commit",
            "-m",
            "deny-main",
            "--no-gpg-sign",
        ])
        .status()
        .unwrap();
    fail_git_push(ssh_work.path(), "main", &f.priv_key, f.sshd.port);
    assert_eq!(ref_oid(&bare, "refs/heads/main"), main_before);

    Command::new("git")
        .current_dir(ssh_work.path())
        .args(["checkout", "-B", feature])
        .status()
        .unwrap();
    std::fs::write(ssh_work.path().join("alt.txt"), "ssh-1\n").unwrap();
    Command::new("git")
        .current_dir(ssh_work.path())
        .args(["add", "alt.txt"])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(ssh_work.path())
        .args([
            "-c",
            "user.email=e2e@musehub.local",
            "-c",
            "user.name=e2e",
            "commit",
            "-m",
            "ssh-1",
            "--no-gpg-sign",
        ])
        .status()
        .unwrap();
    let push1 = git_ssh_cmd_in(
        &f.priv_key,
        f.sshd.port,
        Some(ssh_work.path()),
        &[
            "push",
            "-u",
            "origin",
            &format!("HEAD:refs/heads/{feature}"),
        ],
    );
    assert!(
        push1.status.success(),
        "{}",
        String::from_utf8_lossy(&push1.stderr)
    );
    let oid_ssh_1 = ref_oid(&bare, &format!("refs/heads/{feature}"));

    musehub_app::git_ssh_runtime::ensure_shared_git_storage_root(&f.git_root)
        .expect("shared git storage root before HTTP");
    // SSH receive-pack writes refs as `git`; HTTP receive-pack runs via `sg musehub-git`.
    let bare_s = bare.to_str().unwrap();
    assert!(
        std::process::Command::new("sudo")
            .args(["-n", "chgrp", "-R", "musehub-git", bare_s])
            .status()
            .expect("sudo chgrp")
            .success(),
        "chgrp bare repo after SSH"
    );
    assert!(
        std::process::Command::new("sudo")
            .args(["-n", "chmod", "-R", "g+rwX", bare_s])
            .status()
            .expect("sudo chmod")
            .success(),
        "chmod bare repo after SSH"
    );
    let whoami = std::env::var("USER").unwrap_or_else(|_| "ubuntu".into());
    assert!(
        std::process::Command::new("sudo")
            .args([
                "-n",
                "chown",
                "-R",
                &format!("{whoami}:musehub-git"),
                &format!("{bare_s}/refs"),
            ])
            .status()
            .expect("sudo chown refs")
            .success(),
        "chown refs for shared HTTP receive-pack"
    );
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("admin:{}", pat.pat));
    let refs = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/git/demo/{}.git/info/refs?service=git-upload-pack",
                    f.repo
                ))
                .header(header::AUTHORIZATION, format!("Basic {basic}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot refs");
    assert_eq!(
        refs.status(),
        StatusCode::OK,
        "in-process git upload-pack advertisement failed"
    );
    let http_work = tempfile::tempdir().unwrap();
    pat.clone_repo_async(&http_url, http_work.path()).await;
    Command::new("git")
        .current_dir(http_work.path())
        .args(["checkout", feature])
        .status()
        .unwrap();
    std::fs::write(http_work.path().join("alt.txt"), "ssh-1\nhttp-2\n").unwrap();
    Command::new("git")
        .current_dir(http_work.path())
        .args(["add", "alt.txt"])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(http_work.path())
        .args([
            "-c",
            "user.email=e2e@musehub.local",
            "-c",
            "user.name=e2e",
            "commit",
            "-m",
            "http-2",
            "--no-gpg-sign",
        ])
        .status()
        .unwrap();
    let oid_http_2 = pat.push_branch_async(http_work.path(), feature).await;
    assert_ne!(oid_http_2, oid_ssh_1);
    assert_eq!(ref_oid(&bare, &format!("refs/heads/{feature}")), oid_http_2);

    let fetch = git_ssh_cmd_in(
        &f.priv_key,
        f.sshd.port,
        Some(ssh_work.path()),
        &["fetch", "origin", feature],
    );
    assert!(
        fetch.status.success(),
        "{}",
        String::from_utf8_lossy(&fetch.stderr)
    );
    let merge = git_ssh_cmd_in(
        &f.priv_key,
        f.sshd.port,
        Some(ssh_work.path()),
        &["merge", "--ff-only", &format!("origin/{feature}")],
    );
    assert!(
        merge.status.success(),
        "{}",
        String::from_utf8_lossy(&merge.stderr)
    );
    std::fs::write(ssh_work.path().join("alt.txt"), "ssh-1\nhttp-2\nssh-3\n").unwrap();
    Command::new("git")
        .current_dir(ssh_work.path())
        .args(["add", "alt.txt"])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(ssh_work.path())
        .args([
            "-c",
            "user.email=e2e@musehub.local",
            "-c",
            "user.name=e2e",
            "commit",
            "-m",
            "ssh-3",
            "--no-gpg-sign",
        ])
        .status()
        .unwrap();
    let push3 = git_ssh_cmd_in(
        &f.priv_key,
        f.sshd.port,
        Some(ssh_work.path()),
        &["push", "origin", &format!("HEAD:refs/heads/{feature}")],
    );
    assert!(
        push3.status.success(),
        "{}",
        String::from_utf8_lossy(&push3.stderr)
    );
    let oid_ssh_3 = ref_oid(&bare, &format!("refs/heads/{feature}"));
    assert_ne!(oid_ssh_3, oid_http_2);

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
            let mode = entry.metadata().unwrap().permissions().mode() & 0o077;
            assert_ne!(mode & 0o040, 0, "object shard dirs group-readable");
            any_dir = true;
        }
    }
    assert!(any_dir);

    pat.revoke().await;
}
