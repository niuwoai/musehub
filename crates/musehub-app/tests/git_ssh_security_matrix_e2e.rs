//! Git-over-SSH security and protection matrix (requires loopback sshd).

mod common;

use std::process::Command;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

use common::git_ssh_fixture::git_ssh_fixture;
use common::git_ssh_util::{git_ssh_cmd, git_ssh_cmd_in, raw_ssh, raw_ssh_as};

fn commit_file(work: &std::path::Path, branch: &str, file: &str, contents: &[u8]) {
    Command::new("git")
        .current_dir(work)
        .args(["checkout", "-B", branch])
        .status()
        .unwrap();
    std::fs::write(work.join(file), contents).unwrap();
    Command::new("git")
        .current_dir(work)
        .args(["add", file])
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
            "e2e",
            "--no-gpg-sign",
        ])
        .status()
        .unwrap();
}

#[tokio::test]
async fn git_ssh_clone_fetch_push_feature_branch() {
    let f = git_ssh_fixture("sshflow").await;
    let work_dir = tempfile::tempdir().expect("work");
    let work = work_dir.path();
    let clone = git_ssh_cmd(
        &f.priv_key,
        f.sshd.port,
        &["clone", &f.clone_url, work.to_str().unwrap()],
    );
    assert!(
        clone.status.success(),
        "{}",
        String::from_utf8_lossy(&clone.stderr)
    );
    commit_file(work, "feature/ssh", "f.txt", b"data\n");
    let push = git_ssh_cmd_in(
        &f.priv_key,
        f.sshd.port,
        Some(work),
        &["push", "origin", "HEAD:refs/heads/feature/ssh"],
    );
    assert!(
        push.status.success(),
        "{}",
        String::from_utf8_lossy(&push.stderr)
    );
    let fetch = git_ssh_cmd_in(
        &f.priv_key,
        f.sshd.port,
        Some(work),
        &["fetch", "origin", "feature/ssh"],
    );
    assert!(fetch.status.success());
}

#[tokio::test]
async fn git_ssh_protected_main_push_force_delete_denied() {
    let f = git_ssh_fixture("sshprot").await;
    let work_dir = tempfile::tempdir().expect("work");
    let work = work_dir.path();
    let clone = git_ssh_cmd(
        &f.priv_key,
        f.sshd.port,
        &["clone", &f.clone_url, work.to_str().unwrap()],
    );
    assert!(
        clone.status.success(),
        "{}",
        String::from_utf8_lossy(&clone.stderr)
    );
    commit_file(work, "feature/x", "x.txt", b"x\n");
    let push_main = git_ssh_cmd_in(
        &f.priv_key,
        f.sshd.port,
        Some(work),
        &["push", "origin", "HEAD:main"],
    );
    assert!(!push_main.status.success());
    let push_force = git_ssh_cmd_in(
        &f.priv_key,
        f.sshd.port,
        Some(work),
        &["push", "--force", "origin", "HEAD:main"],
    );
    assert!(!push_force.status.success());
    let del_main = git_ssh_cmd_in(
        &f.priv_key,
        f.sshd.port,
        Some(work),
        &["push", "origin", ":refs/heads/main"],
    );
    assert!(!del_main.status.success());
}

#[tokio::test]
async fn git_ssh_revoked_key_denied_on_next_session() {
    let f = git_ssh_fixture("sshrevoke").await;
    let work_dir = tempfile::tempdir().expect("work");
    let work = work_dir.path();
    let ok = git_ssh_cmd(
        &f.priv_key,
        f.sshd.port,
        &["clone", &f.clone_url, work.to_str().unwrap()],
    );
    assert!(ok.status.success());
    let revoke = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/ssh-keys/{}", f.key_id))
                .header(header::COOKIE, &f.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoke.status(), StatusCode::NO_CONTENT);
    let work2_dir = tempfile::tempdir().expect("work2");
    let work2 = work2_dir.path();
    let denied = git_ssh_cmd(
        &f.priv_key,
        f.sshd.port,
        &["clone", &f.clone_url, work2.to_str().unwrap()],
    );
    assert!(!denied.status.success());
}

#[tokio::test]
async fn git_ssh_disabled_principal_denied() {
    let f = git_ssh_fixture("sshdisabled").await;
    sqlx::query("UPDATE principals SET status = 'disabled' WHERE handle = 'admin'")
        .execute(&f.pool)
        .await
        .unwrap();
    let denied_dir = tempfile::tempdir().unwrap();
    let out = git_ssh_cmd(
        &f.priv_key,
        f.sshd.port,
        &["clone", &f.clone_url, denied_dir.path().to_str().unwrap()],
    );
    assert!(!out.status.success());
}

#[tokio::test]
async fn git_ssh_cross_repo_without_grant_denied() {
    let f = git_ssh_fixture("sshgrant").await;
    let other = format!("other-{}", Uuid::new_v4().simple());
    assert_eq!(
        f.app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos")
                    .header(header::COOKIE, &f.cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(json!({ "slug": other }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status(),
        StatusCode::CREATED
    );
    let outsider_id = Uuid::new_v4();
    let cred_id = Uuid::new_v4();
    let hash = musehub_app::hash_password_for_tests("Outsider-Strong-Pass-22!").unwrap();
    sqlx::query(
        "INSERT INTO principals (id, kind, handle, handle_normalized, display_name, status)
         VALUES ($1, 'human', 'outsider', 'outsider', 'Outsider', 'active')",
    )
    .bind(outsider_id)
    .execute(&f.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_credentials (id, principal_id, password_hash, password_changed_at)
         VALUES ($1, $2, $3, CURRENT_TIMESTAMP)",
    )
    .bind(cred_id)
    .bind(outsider_id)
    .bind(hash)
    .execute(&f.pool)
    .await
    .unwrap();
    let org_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM organizations WHERE slug_normalized = 'demo'")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO memberships (id, org_id, principal_id, role, status)
         VALUES ($1, $2, $3, 'member', 'active')",
    )
    .bind(Uuid::new_v4())
    .bind(org_id.0)
    .bind(outsider_id)
    .execute(&f.pool)
    .await
    .unwrap();
    let repo_id: (Uuid,) = sqlx::query_as("SELECT id FROM repositories WHERE slug_normalized = $1")
        .bind(&f.repo)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO repository_grants (id, org_id, repo_id, principal_id, role)
         VALUES ($1, $2, $3, $4, 'developer')",
    )
    .bind(Uuid::new_v4())
    .bind(org_id.0)
    .bind(repo_id.0)
    .bind(outsider_id)
    .execute(&f.pool)
    .await
    .unwrap();
    let (outsider_key, outsider_pub) = common::git_ssh_util::ed25519_keypair("outsider");
    let outsider_login = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "handle": "outsider",
                        "password": "Outsider-Strong-Pass-22!"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let outsider_cookie = outsider_login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(
        f.app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/ssh-keys")
                    .header(header::COOKIE, outsider_cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({ "name": "o", "public_key": outsider_pub }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status(),
        StatusCode::CREATED
    );
    let allowed_url = format!("ssh://git@127.0.0.1:{}/demo/{}.git", f.sshd.port, f.repo);
    let forbidden_url = format!("ssh://git@127.0.0.1:{}/demo/{}.git", f.sshd.port, other);
    let ok_dir = tempfile::tempdir().unwrap();
    let ok = git_ssh_cmd(
        &outsider_key,
        f.sshd.port,
        &["clone", &allowed_url, ok_dir.path().to_str().unwrap()],
    );
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    let bad_dir = tempfile::tempdir().unwrap();
    let bad = git_ssh_cmd(
        &outsider_key,
        f.sshd.port,
        &["clone", &forbidden_url, bad_dir.path().to_str().unwrap()],
    );
    assert!(!bad.status.success());
}

#[tokio::test]
async fn git_ssh_forbidden_shell_path_traversal_and_subshell() {
    let f = git_ssh_fixture("sshdeny").await;
    for cmd in [
        "sh",
        "/bin/sh -c id",
        "git-upload-pack ../secret",
        "git-receive-pack demo/../../etc",
    ] {
        let out = raw_ssh(&f.priv_key, f.sshd.port, cmd, false);
        assert!(!out.status.success(), "cmd {:?} should fail", cmd);
    }
    let pty = raw_ssh(&f.priv_key, f.sshd.port, "", true);
    assert!(!pty.status.success());
    let fwd_port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let fwd_only = Command::new("ssh")
        .args([
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-o",
            "ExitOnForwardFailure=yes",
            "-i",
            f.priv_key.to_str().unwrap(),
            "-p",
            &f.sshd.port.to_string(),
            "-N",
            "-L",
            &format!("{fwd_port}:127.0.0.1:22"),
            "git@127.0.0.1",
        ])
        .output()
        .unwrap();
    assert!(
        !fwd_only.status.success(),
        "tcp forwarding must fail: {}",
        String::from_utf8_lossy(&fwd_only.stderr)
    );
    assert!(
        std::net::TcpListener::bind(("127.0.0.1", fwd_port)).is_ok(),
        "local forward port must remain free"
    );
}

#[tokio::test]
async fn git_ssh_wrong_os_login_user_denied() {
    let f = git_ssh_fixture("sshuser").await;
    let out = raw_ssh_as("root", &f.priv_key, f.sshd.port, "", false);
    assert!(!out.status.success());
}

#[tokio::test]
async fn git_ssh_direct_tag_push_denied() {
    let f = git_ssh_fixture("sshtag").await;
    let work_dir = tempfile::tempdir().expect("work");
    let work = work_dir.path();
    git_ssh_cmd(
        &f.priv_key,
        f.sshd.port,
        &["clone", &f.clone_url, work.to_str().unwrap()],
    );
    Command::new("git")
        .current_dir(work)
        .args(["checkout", "-b", "feature/tag"])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(work)
        .args(["tag", "v1.0.0-test"])
        .status()
        .unwrap();
    let push_tag = git_ssh_cmd_in(
        &f.priv_key,
        f.sshd.port,
        Some(work),
        &["push", "origin", "refs/tags/v1.0.0-test"],
    );
    assert!(!push_tag.status.success());
}

#[tokio::test]
async fn git_ssh_concurrent_upload_pack_sessions() {
    let f = git_ssh_fixture("sshconc").await;
    let port = f.sshd.port;
    let url_a = f.clone_url.clone();
    let url_b = f.clone_url.clone();
    let key_a = f.priv_key.clone();
    let key_b = f.priv_key.clone();
    let dir_a = tempfile::tempdir().unwrap();
    let a = git_ssh_cmd(
        &key_a,
        port,
        &["clone", &url_a, dir_a.path().to_str().unwrap()],
    );
    let dir_b = tempfile::tempdir().unwrap();
    let b = git_ssh_cmd(
        &key_b,
        port,
        &["clone", &url_b, dir_b.path().to_str().unwrap()],
    );
    assert!(a.status.success() && b.status.success());
}
