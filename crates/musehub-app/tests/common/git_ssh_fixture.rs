//! Shared Git-over-SSH e2e fixture (repo, key, loopback sshd).

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use std::path::PathBuf;
use tower::ServiceExt;
use uuid::Uuid;

use super::git_ssh_sshd::TestGitSshd;
use super::git_ssh_util::{ed25519_keypair, git_ssh_binary};

pub struct GitSshFixture {
    pub pool: sqlx::PgPool,
    pub app: axum::Router,
    pub cookie: String,
    pub git_root: PathBuf,
    pub repo: String,
    pub priv_key: PathBuf,
    pub key_id: Uuid,
    pub sshd: TestGitSshd,
    pub clone_url: String,
}

pub async fn git_ssh_fixture(repo_prefix: &str) -> GitSshFixture {
    super::git_ssh_util::require_git_ssh_transport_prereqs();
    super::git_ssh_sshd::ensure_git_storage_group();
    let bin = git_ssh_binary();
    let pool = super::migrated_pool().await;
    let database_url = std::env::var("MUSEHUB_TEST_DATABASE_URL").expect("db url");
    let git_root = tempfile::tempdir().expect("git root").path().to_path_buf();
    let boot = super::bootstrap_password();
    let app = super::product_app(pool.clone(), &git_root, &boot).await;
    let cookie = super::login_cookie(&app, "admin", &boot).await;
    let repo = format!("{repo_prefix}-{}", Uuid::new_v4().simple());
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
    let (priv_key, pub_key) = ed25519_keypair("gitssh");
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
    let body = key_create.into_body().collect().await.unwrap().to_bytes();
    let key_id = serde_json::from_slice::<serde_json::Value>(&body)
        .expect("key json")
        .get("id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .expect("key id");
    let sshd = TestGitSshd::start(&git_root, &database_url, &bin);
    let fp_row: (String,) = sqlx::query_as("SELECT fingerprint FROM ssh_keys WHERE id = $1")
        .bind(key_id)
        .fetch_one(&pool)
        .await
        .expect("key fp");
    let _ = super::git_ssh_sshd::install_root_owned_git_ssh_bin(&bin);
    let pk_parts: Vec<&str> = pub_key.split_whitespace().collect();
    let akc_wrap = sshd.run_dir.join("akc-wrap.sh");
    let akc_ok = std::process::Command::new("sudo")
        .args([
            akc_wrap.to_str().expect("akc wrap"),
            "git",
            pk_parts[1],
            pk_parts[0],
            &fp_row.0,
        ])
        .output()
        .expect("akc preflight");
    assert!(
        akc_ok.status.success(),
        "akc preflight failed: {}",
        String::from_utf8_lossy(&akc_ok.stderr)
    );
    let akc_line = String::from_utf8_lossy(&akc_ok.stdout).trim().to_string();
    assert!(
        akc_line.contains("environment=\"MUSEHUB_SSH_KEY_ID=") && akc_line.contains("ssh-ed25519 "),
        "AKC line missing key-bound gateway env: {akc_line}"
    );
    let clone_url = format!("ssh://git@127.0.0.1:{}/demo/{}.git", sshd.port, repo);
    GitSshFixture {
        pool,
        app,
        cookie,
        git_root,
        repo,
        priv_key,
        key_id,
        sshd,
        clone_url,
    }
}
