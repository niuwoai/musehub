//! Protected default branch: direct push refused; agent branch allowed.

mod common;

use std::process::Command;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use musehub_app::bare_repo_path;
use tower::ServiceExt;
use uuid::Uuid;

#[test]
fn mirror_import_can_replace_seed_once_and_internal_refs_stay_hidden() {
    let target = std::env::current_dir()
        .unwrap()
        .join("../../target")
        .canonicalize()
        .expect("workspace target directory");
    let git_root = tempfile::Builder::new()
        .prefix("musehub-git-policy-")
        .tempdir_in(target)
        .expect("git root");
    let storage_key = Uuid::new_v4();
    musehub_app::init_bare_repository(git_root.path(), &storage_key).expect("initialize bare repo");
    let bare = bare_repo_path(git_root.path(), &storage_key);
    let source = tempfile::tempdir().expect("source");
    assert!(Command::new("git")
        .args(["init", "-b", "main", source.path().to_str().unwrap()])
        .status()
        .unwrap()
        .success());
    std::fs::write(source.path().join("imported.txt"), b"imported history").unwrap();
    assert!(Command::new("git")
        .current_dir(source.path())
        .args(["add", "imported.txt"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .current_dir(source.path())
        .args([
            "-c",
            "user.email=t@musehub.local",
            "-c",
            "user.name=t",
            "commit",
            "-m",
            "import",
            "--no-gpg-sign",
        ])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .current_dir(source.path())
        .args(["remote", "add", "musehub", bare.to_str().unwrap()])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .current_dir(source.path())
        .args(["push", "--mirror", "musehub"])
        .status()
        .unwrap()
        .success());

    let imported_oid = Command::new("git")
        .current_dir(source.path())
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let imported_oid = String::from_utf8_lossy(&imported_oid.stdout)
        .trim()
        .to_string();
    assert!(Command::new("git")
        .arg("--git-dir")
        .arg(&bare)
        .args([
            "update-ref",
            "refs/musehub/merge-staging/probe",
            &imported_oid,
        ])
        .status()
        .unwrap()
        .success());
    let advertised = Command::new("git")
        .args(["ls-remote", bare.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(advertised.status.success());
    assert!(!String::from_utf8_lossy(&advertised.stdout).contains("refs/musehub/"));

    assert!(Command::new("git")
        .current_dir(source.path())
        .args([
            "update-ref",
            "refs/musehub/merge-staging/attacker",
            &imported_oid,
        ])
        .status()
        .unwrap()
        .success());
    let internal_push = Command::new("git")
        .current_dir(source.path())
        .args(["push", "--mirror", "musehub"])
        .status()
        .unwrap();
    assert!(!internal_push.success());
    std::fs::write(source.path().join("later.txt"), b"later update").unwrap();
    assert!(Command::new("git")
        .current_dir(source.path())
        .args(["add", "later.txt"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .current_dir(source.path())
        .args([
            "-c",
            "user.email=t@musehub.local",
            "-c",
            "user.name=t",
            "commit",
            "-m",
            "later",
            "--no-gpg-sign",
        ])
        .status()
        .unwrap()
        .success());
    assert!(!Command::new("git")
        .current_dir(source.path())
        .args(["push", "musehub", "HEAD:main"])
        .status()
        .unwrap()
        .success());
}

#[tokio::test]
async fn direct_main_push_denied_agent_branch_allowed() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;

    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"handle":"admin","password":bootstrap}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .expect("login");
    let cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let create_repo = serde_json::json!({"slug": "prot", "description": "prot"});
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(create_repo.to_string()))
                .unwrap(),
        )
        .await
        .expect("repo");

    let storage_key: Uuid = sqlx::query_as::<_, (Uuid,)>(
        "SELECT storage_key FROM repositories WHERE slug_normalized = 'prot'",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .0;
    let bare = bare_repo_path(git_root.path(), &storage_key);

    let work = tempfile::tempdir().expect("work");
    assert!(Command::new("git")
        .args([
            "clone",
            bare.to_str().unwrap(),
            work.path().to_str().unwrap()
        ])
        .status()
        .unwrap()
        .success());
    std::fs::write(work.path().join("direct.txt"), b"blocked").unwrap();
    Command::new("git")
        .current_dir(work.path())
        .args(["add", "direct.txt"])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(work.path())
        .args([
            "-c",
            "user.email=t@musehub.local",
            "-c",
            "user.name=t",
            "commit",
            "-m",
            "direct",
            "--no-gpg-sign",
        ])
        .status()
        .unwrap();
    let push_main = Command::new("git")
        .current_dir(work.path())
        .args(["push", "origin", "HEAD:main"])
        .status()
        .unwrap();
    assert!(!push_main.success());

    let agent_work = tempfile::tempdir().expect("agent work");
    assert!(Command::new("git")
        .args([
            "clone",
            bare.to_str().unwrap(),
            agent_work.path().to_str().unwrap(),
        ])
        .status()
        .unwrap()
        .success());
    std::fs::write(agent_work.path().join("agent.txt"), b"ok").unwrap();
    Command::new("git")
        .current_dir(agent_work.path())
        .args(["checkout", "-b", "agent/test-branch"])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(agent_work.path())
        .args(["add", "agent.txt"])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(agent_work.path())
        .args([
            "-c",
            "user.email=a@musehub.local",
            "-c",
            "user.name=a",
            "commit",
            "-m",
            "agent",
            "--no-gpg-sign",
        ])
        .status()
        .unwrap();
    let push_agent = Command::new("git")
        .current_dir(agent_work.path())
        .args(["push", "origin", "HEAD:refs/heads/agent/test-branch"])
        .status()
        .unwrap();
    assert!(push_agent.success());
    let _ = StatusCode::OK;
}
