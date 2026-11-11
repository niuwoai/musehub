//! Negative tests: SSH public key newline / multiline injection must not reach AKC output.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use serde_json::json;
use tower::ServiceExt;

use musehub_app::git_ssh::format_authorized_keys_line;
use musehub_app::git_ssh::SshKeyAuth;
use musehub_app::ssh_keys::parse_and_canonicalize_ssh_public_key;
use uuid::Uuid;

fn valid_ed25519_line() -> String {
    let dir = tempfile::tempdir().expect("keydir");
    let priv_path = dir.path().join("id_ed25519");
    std::process::Command::new("ssh-keygen")
        .args([
            "-t",
            "ed25519",
            "-f",
            priv_path.to_str().unwrap(),
            "-N",
            "",
            "-q",
        ])
        .status()
        .unwrap();
    std::fs::read_to_string(priv_path.with_extension("pub"))
        .unwrap()
        .trim()
        .to_string()
}

#[tokio::test]
async fn api_rejects_multiline_ssh_public_key() {
    let pool = common::migrated_pool().await;
    let boot = common::bootstrap_password();
    let app = common::product_app(pool, tempfile::tempdir().unwrap().path(), &boot).await;
    let cookie = common::login_cookie(&app, "admin", &boot).await;
    let good = valid_ed25519_line();
    let evil = format!(
        "{good}\nssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIUNTRVUSUNURURFQUtERVJMT0dJTgo= attacker@evil"
    );
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/ssh-keys")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "name": "evil", "public_key": evil }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn akc_line_never_contains_extra_authorized_keys_record() {
    let good = parse_and_canonicalize_ssh_public_key(&valid_ed25519_line()).unwrap();
    let auth = SshKeyAuth {
        key_id: Uuid::new_v4(),
        principal_id: Uuid::new_v4(),
        handle: "u".into(),
        public_key: good,
    };
    let line = format_authorized_keys_line(&auth, "/usr/local/bin/musehub-git-ssh").unwrap();
    assert!(!line.contains('\n'));
    assert!(!line.contains('\r'));
    assert_eq!(line.matches("ssh-ed25519 ").count(), 1);
    assert!(line.contains("environment=\"MUSEHUB_SSH_KEY_ID="));
    assert!(!line.contains("command=\""));
}

#[tokio::test]
async fn malformed_stored_key_fails_akc_format() {
    let auth = SshKeyAuth {
        key_id: Uuid::new_v4(),
        principal_id: Uuid::new_v4(),
        handle: "u".into(),
        public_key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI\nssh-ed25519 BOGUS".into(),
    };
    assert!(format_authorized_keys_line(&auth, "/bin/musehub-git-ssh").is_err());
}
