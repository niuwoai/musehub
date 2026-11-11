//! Product Git-over-SSH: real sshd + git clone/push with stored keys.

mod common;

use std::path::PathBuf;
use std::process::Command;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

use common::git_ssh_sshd::TestGitSshd;

fn git_ssh_bin() -> PathBuf {
    if let Ok(p) = std::env::var("MUSEHUB_GIT_SSH_BIN") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/musehub-git-ssh")
}

fn ed25519_keypair() -> (PathBuf, String) {
    let dir = tempfile::tempdir().expect("keydir").into_path();
    let priv_path = dir.join("id_ed25519");
    let status = Command::new("ssh-keygen")
        .args([
            "-t",
            "ed25519",
            "-f",
            priv_path.to_str().expect("path"),
            "-N",
            "",
            "-q",
        ])
        .status()
        .expect("ssh-keygen");
    assert!(status.success());
    let pub_body = std::fs::read_to_string(priv_path.with_extension("pub")).expect("pub");
    (priv_path, pub_body.trim().to_string())
}

fn ssh_git(
    priv_key: &PathBuf,
    port: u16,
    args: &[&str],
    env_extra: &[(&str, &str)],
) -> std::process::Output {
    let mut cmd = Command::new("git");
    cmd.args(["-c", "core.sshCommand=ssh -i"]);
    // build ssh command as single -c value
    let ssh_cmd = format!(
        "ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -i {} -p {}",
        priv_key.display(),
        port
    );
    cmd.args(["-c", &format!("core.sshCommand={ssh_cmd}")]);
    cmd.args(args);
    for (k, v) in env_extra {
        cmd.env(k, v);
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.output().expect("git")
}

#[tokio::test]
async fn git_ssh_transport_auth_and_protection() {
    let sshd = ["/usr/sbin/sshd", "/usr/local/sbin/sshd"]
        .iter()
        .map(std::path::Path::new)
        .find(|p| p.exists())
        .map(std::path::Path::to_path_buf)
        .or_else(|| {
            Command::new("which")
                .arg("sshd")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| PathBuf::from(s.trim()))
        });
    if sshd.is_none() {
        eprintln!("skip: sshd not available");
        return;
    }
    let bin = git_ssh_bin();
    if !bin.exists() {
        eprintln!("skip: build musehub-git-ssh first");
        return;
    }
    let pool = common::migrated_pool().await;
    let database_url = std::env::var("MUSEHUB_TEST_DATABASE_URL").expect("db url");
    let git_root = tempfile::tempdir().expect("git root").path().to_path_buf();
    let boot = common::bootstrap_password();
    let app = common::product_app(pool.clone(), &git_root, &boot).await;
    let cookie = common::login_cookie(&app, "admin", &boot).await;
    let repo = format!("ssh-{}", Uuid::new_v4().simple());
    assert_eq!(
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos")
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(json!({ "slug": repo }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status(),
        StatusCode::CREATED
    );
    let (priv_key, pub_key) = ed25519_keypair();
    let key_create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/ssh-keys")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "name": "e2e", "public_key": pub_key }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(key_create.status(), StatusCode::CREATED);
    let sshd = TestGitSshd::start(&git_root, &database_url, &bin);
    let url = format!("ssh://git@127.0.0.1:{}/{}/{}.git", sshd.port, "demo", repo);
    let work = tempfile::tempdir().expect("work").into_path();
    let clone = ssh_git(
        &priv_key,
        sshd.port,
        &["clone", &url, work.to_str().unwrap()],
        &[],
    );
    assert!(
        clone.status.success(),
        "clone failed: {}",
        String::from_utf8_lossy(&clone.stderr)
    );
    std::fs::write(work.join("ssh.txt"), b"via-ssh\n").unwrap();
    Command::new("git")
        .current_dir(&work)
        .args(["checkout", "-b", "feature/ssh"])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(&work)
        .args(["add", "ssh.txt"])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(&work)
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
    let push = Command::new("git")
        .current_dir(&work)
        .args([
            "-c",
            &format!(
                "core.sshCommand=ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -i {} -p {}",
                priv_key.display(),
                sshd.port
            ),
        ])
        .args(["push", "origin", "HEAD:refs/heads/feature/ssh"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        push.status.success(),
        "push: {}",
        String::from_utf8_lossy(&push.stderr)
    );
    let push_main = Command::new("git")
        .current_dir(&work)
        .args([
            "-c",
            &format!(
                "core.sshCommand=ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -i {} -p {}",
                priv_key.display(),
                sshd.port
            ),
        ])
        .args(["push", "origin", "HEAD:main"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        !push_main.status.success(),
        "protected main push should fail"
    );
}
