//! PR security: hook never disabled, check auth, CAS, conflict, concurrent push.

mod common;

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use musehub_app::bare_repo_path;
use tower::ServiceExt;
use uuid::Uuid;

const REVIEWER_PASS: &str = "Reviewer-Sec-Strong-12!";
const CI_REPORTER_PASS: &str = "Ci-Reporter-Sec-12!";
const CHECK: &str = "product-ci";

async fn invite_ci_reporter(app: &axum::Router, admin_cookie: &str) -> String {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/members")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "handle": "ci-reporter",
                        "password": CI_REPORTER_PASS,
                        "role": "admin"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    common::login_cookie(app, "ci-reporter", CI_REPORTER_PASS).await
}

#[tokio::test]
async fn author_cannot_forged_required_check() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"slug":"forge","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let storage_key: Uuid = sqlx::query_as::<_, (Uuid,)>(
        "SELECT storage_key FROM repositories WHERE slug_normalized = 'forge'",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .0;
    let bare = bare_repo_path(git_root.path(), &storage_key);
    let work = git_root.path().join("w-forge");
    Command::new("git")
        .args(["clone", bare.to_str().unwrap(), work.to_str().unwrap()])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(&work)
        .args(["checkout", "-b", "feature/x"])
        .status()
        .unwrap();
    std::fs::write(work.join("a.txt"), b"a").unwrap();
    Command::new("git")
        .current_dir(&work)
        .args(["add", "a.txt"])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(&work)
        .args([
            "-c",
            "user.email=a@musehub.local",
            "-c",
            "user.name=a",
            "commit",
            "-m",
            "a",
            "--no-gpg-sign",
        ])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(&work)
        .args(["push", "origin", "HEAD:refs/heads/feature/x"])
        .status()
        .unwrap();

    let pr = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/forge/pull-requests")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "title": "forge",
                        "source_branch": "feature/x"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let pr_number = serde_json::from_slice::<serde_json::Value>(
        &pr.into_body().collect().await.unwrap().to_bytes(),
    )
    .unwrap()["number"]
        .as_i64()
        .unwrap();

    let forged = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/forge/pull-requests/{pr_number}/checks/{CHECK}"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "state": "success",
                        "attempt_id": Uuid::new_v4()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(forged.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn merge_target_cas_blocks_when_main_moves() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"slug":"conflict","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let storage_key: Uuid = sqlx::query_as::<_, (Uuid,)>(
        "SELECT storage_key FROM repositories WHERE slug_normalized = 'conflict'",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .0;
    let bare = bare_repo_path(git_root.path(), &storage_key);
    let work = git_root.path().join("w-conflict");
    Command::new("git")
        .args(["clone", bare.to_str().unwrap(), work.to_str().unwrap()])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(&work)
        .args(["checkout", "-B", "feature/a", "origin/main"])
        .status()
        .unwrap();
    std::fs::write(work.join("feature.txt"), b"a").unwrap();
    Command::new("git")
        .current_dir(&work)
        .args(["add", "feature.txt"])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(&work)
        .args([
            "-c",
            "user.email=a@musehub.local",
            "-c",
            "user.name=a",
            "commit",
            "-m",
            "feature",
            "--no-gpg-sign",
        ])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(&work)
        .args(["push", "origin", "HEAD:refs/heads/feature/a"])
        .status()
        .unwrap();
    let pr = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/conflict/pull-requests")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "title": "conflict",
                        "source_branch": "feature/a"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let pr_number = serde_json::from_slice::<serde_json::Value>(
        &pr.into_body().collect().await.unwrap().to_bytes(),
    )
    .unwrap()["number"]
        .as_i64()
        .unwrap();

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/members")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "handle": "confrev",
                        "password": REVIEWER_PASS,
                        "role": "member"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/conflict/collaborators")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"handle":"confrev","role":"maintainer"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let rev = common::login_cookie(&app, "confrev", REVIEWER_PASS).await;
    let ci_cookie = invite_ci_reporter(&app, &admin_cookie).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/conflict/pull-requests/{pr_number}/reviews"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &rev)
                .body(Body::from(r#"{"decision":"approve"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/conflict/pull-requests/{pr_number}/checks/{CHECK}"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &ci_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "state": "success",
                        "attempt_id": Uuid::new_v4()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    Command::new("git")
        .current_dir(&work)
        .args(["checkout", "-B", "main", "origin/main"])
        .status()
        .unwrap();
    std::fs::write(work.join("main-advance.txt"), b"moved").unwrap();
    Command::new("git")
        .current_dir(&work)
        .args(["add", "main-advance.txt"])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(&work)
        .args([
            "-c",
            "user.email=b@musehub.local",
            "-c",
            "user.name=b",
            "commit",
            "-m",
            "advance main",
            "--no-gpg-sign",
        ])
        .status()
        .unwrap();
    let main_tip = String::from_utf8_lossy(
        &Command::new("git")
            .current_dir(&work)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .trim()
    .to_string();
    Command::new("git")
        .args([
            "--git-dir",
            bare.to_str().unwrap(),
            "fetch",
            work.to_str().unwrap(),
            &format!("+{main_tip}:refs/heads/main"),
        ])
        .status()
        .unwrap();

    let blocked = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/conflict/pull-requests/{pr_number}/merge"
                ))
                .header(header::COOKIE, &rev)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::BAD_REQUEST);
    let body = blocked.into_body().collect().await.unwrap().to_bytes();
    let msg = String::from_utf8_lossy(&body);
    assert!(msg.contains("CAS") || msg.contains("target branch advanced"));
}

#[tokio::test]
async fn merge_conflict_returns_validation() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"slug":"unrel","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let storage_key: Uuid = sqlx::query_as::<_, (Uuid,)>(
        "SELECT storage_key FROM repositories WHERE slug_normalized = 'unrel'",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .0;
    let bare = bare_repo_path(git_root.path(), &storage_key);
    let orphan = git_root.path().join("orphan");
    Command::new("git")
        .args(["init", orphan.to_str().unwrap()])
        .status()
        .unwrap();
    std::fs::write(orphan.join("only.txt"), b"orphan").unwrap();
    Command::new("git")
        .current_dir(&orphan)
        .args(["add", "only.txt"])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(&orphan)
        .args([
            "-c",
            "user.email=o@musehub.local",
            "-c",
            "user.name=o",
            "commit",
            "-m",
            "orphan",
            "--no-gpg-sign",
        ])
        .status()
        .unwrap();
    Command::new("git")
        .args([
            "--git-dir",
            bare.to_str().unwrap(),
            "remote",
            "add",
            "orphan",
            orphan.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    Command::new("git")
        .args([
            "--git-dir",
            bare.to_str().unwrap(),
            "fetch",
            "orphan",
            "+HEAD:refs/heads/feature/unrelated",
        ])
        .status()
        .unwrap();
    let pr = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/unrel/pull-requests")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "title": "unrel",
                        "source_branch": "feature/unrelated"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pr.status(), StatusCode::CREATED);
    let pr_number = serde_json::from_slice::<serde_json::Value>(
        &pr.into_body().collect().await.unwrap().to_bytes(),
    )
    .unwrap()["number"]
        .as_i64()
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/members")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "handle": "urev",
                        "password": REVIEWER_PASS,
                        "role": "member"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/unrel/collaborators")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"handle":"urev","role":"maintainer"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let rev = common::login_cookie(&app, "urev", REVIEWER_PASS).await;
    let ci_cookie = invite_ci_reporter(&app, &admin_cookie).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/unrel/pull-requests/{pr_number}/reviews"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &rev)
                .body(Body::from(r#"{"decision":"approve"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/unrel/pull-requests/{pr_number}/checks/{CHECK}"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &ci_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "state": "success",
                        "attempt_id": Uuid::new_v4()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let blocked = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/unrel/pull-requests/{pr_number}/merge"
                ))
                .header(header::COOKIE, &rev)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn outsider_cannot_merge_pull_request() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/members")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "handle": "outsider",
                        "password": "Outsider-Strong-Pass-9!",
                        "role": "member"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"slug":"outs","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let storage_key: Uuid = sqlx::query_as::<_, (Uuid,)>(
        "SELECT storage_key FROM repositories WHERE slug_normalized = 'outs'",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .0;
    let bare = bare_repo_path(git_root.path(), &storage_key);
    let work = git_root.path().join("w-out");
    Command::new("git")
        .args(["clone", bare.to_str().unwrap(), work.to_str().unwrap()])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(&work)
        .args(["checkout", "-b", "feature/o"])
        .status()
        .unwrap();
    std::fs::write(work.join("o.txt"), b"o").unwrap();
    Command::new("git")
        .current_dir(&work)
        .args(["add", "o.txt"])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(&work)
        .args([
            "-c",
            "user.email=o@musehub.local",
            "-c",
            "user.name=o",
            "commit",
            "-m",
            "o",
            "--no-gpg-sign",
        ])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(&work)
        .args(["push", "origin", "HEAD:refs/heads/feature/o"])
        .status()
        .unwrap();
    let pr = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/outs/pull-requests")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "title": "outs",
                        "source_branch": "feature/o"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let pr_number = serde_json::from_slice::<serde_json::Value>(
        &pr.into_body().collect().await.unwrap().to_bytes(),
    )
    .unwrap()["number"]
        .as_i64()
        .unwrap();
    let outsider = common::login_cookie(&app, "outsider", "Outsider-Strong-Pass-9!").await;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/outs/pull-requests/{pr_number}/merge"
                ))
                .header(header::COOKIE, &outsider)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn concurrent_direct_push_blocked_while_merge_runs() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/members")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "handle": "mergerev",
                        "password": REVIEWER_PASS,
                        "role": "member"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"slug":"conc","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/conc/collaborators")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"handle":"mergerev","role":"maintainer"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let issue: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/conc/issues")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::from(r#"{"title":"i"}"#))
                    .unwrap(),
            )
            .await
            .unwrap()
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap();
    let n = issue["number"].as_i64().unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/demo/repos/conc/issues/{n}/plans"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"body":"p"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/orgs/demo/repos/conc/issues/{n}/plans"))
                .header(header::COOKIE, &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let agent = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/conc/agent-runs")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({"issue_id": issue["id"], "message": "work"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let run_id = serde_json::from_slice::<serde_json::Value>(
        &agent.into_body().collect().await.unwrap().to_bytes(),
    )
    .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    for _ in 0..120 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let st = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v1/agent-runs/{run_id}"))
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let run: serde_json::Value =
            serde_json::from_slice(&st.into_body().collect().await.unwrap().to_bytes()).unwrap();
        if run["state"] == "succeeded" {
            break;
        }
    }
    let source = format!("agent/{run_id}");
    let pr = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/conc/pull-requests")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "title": "c",
                        "source_branch": source
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let pr_number = serde_json::from_slice::<serde_json::Value>(
        &pr.into_body().collect().await.unwrap().to_bytes(),
    )
    .unwrap()["number"]
        .as_i64()
        .unwrap();
    let rev = common::login_cookie(&app, "mergerev", REVIEWER_PASS).await;
    let ci_cookie = invite_ci_reporter(&app, &admin_cookie).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/conc/pull-requests/{pr_number}/reviews"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &rev)
                .body(Body::from(r#"{"decision":"approve"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/conc/pull-requests/{pr_number}/checks/{CHECK}"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &ci_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "state": "success",
                        "attempt_id": Uuid::new_v4()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let storage_key: Uuid = sqlx::query_as::<_, (Uuid,)>(
        "SELECT storage_key FROM repositories WHERE slug_normalized = 'conc'",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .0;
    let bare = bare_repo_path(git_root.path(), &storage_key);
    let hook_path = bare.join("hooks/pre-receive");
    assert!(hook_path.is_file());

    let pusher_work = git_root.path().join("push-main");
    Command::new("git")
        .args([
            "clone",
            bare.to_str().unwrap(),
            pusher_work.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(&pusher_work)
        .args(["checkout", "main"])
        .status()
        .unwrap();
    std::fs::write(pusher_work.join("rogue.txt"), b"rogue").unwrap();
    Command::new("git")
        .current_dir(&pusher_work)
        .args(["add", "rogue.txt"])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(&pusher_work)
        .args([
            "-c",
            "user.email=r@musehub.local",
            "-c",
            "user.name=r",
            "commit",
            "-m",
            "rogue",
            "--no-gpg-sign",
        ])
        .status()
        .unwrap();

    let push_failed = Arc::new(AtomicBool::new(false));
    let push_failed_cl = push_failed.clone();
    let pusher = pusher_work.clone();
    let bare_s = bare.to_str().unwrap().to_string();
    let app_merge = app.clone();
    let rev_merge = rev.clone();
    let merge_handle = tokio::spawn(async move {
        for _ in 0..40 {
            let push_out = Command::new("git")
                .current_dir(&pusher)
                .args(["push", &bare_s, "HEAD:refs/heads/main"])
                .output();
            if let Ok(out) = push_out {
                if !out.status.success() {
                    push_failed_cl.store(true, Ordering::SeqCst);
                }
            }
            let resp = app_merge
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!(
                            "/api/v1/orgs/demo/repos/conc/pull-requests/{pr_number}/merge"
                        ))
                        .header(header::COOKIE, &rev_merge)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            if resp.status() == StatusCode::OK {
                return Some(StatusCode::OK);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        None
    });
    let merge_result = merge_handle.await.unwrap();
    assert!(hook_path.is_file());
    assert!(
        !bare.join("hooks/pre-receive.musehub-merge-bak").exists(),
        "pre-receive hook must not be temporarily moved aside"
    );
    assert!(
        push_failed.load(Ordering::SeqCst),
        "direct push to main must fail"
    );
    assert_eq!(merge_result, Some(StatusCode::OK));
}
