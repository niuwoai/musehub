//! Pipeline / PAT runner / release negative security cases.

mod common;

use std::process::Command;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use musehub_app::bare_repo_path;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const MEMBER_PASS: &str = "Pipe-Sec-Member-12!";

async fn seed_repo_with_open_pr(
    app: &axum::Router,
    pool: &PgPool,
    git_root: &std::path::Path,
    admin_cookie: &str,
) -> (i64, String) {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, admin_cookie)
                .body(Body::from(r#"{"slug":"pipe-sec","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let storage_key: Uuid = sqlx::query_as::<_, (Uuid,)>(
        "SELECT storage_key FROM repositories WHERE slug_normalized = 'pipe-sec'",
    )
    .fetch_one(pool)
    .await
    .unwrap()
    .0;
    let bare = bare_repo_path(git_root, &storage_key);
    let work = git_root.join("w-pipe-sec");
    let _ = std::fs::remove_dir_all(&work);
    Command::new("git")
        .args(["clone", bare.to_str().unwrap(), work.to_str().unwrap()])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(&work)
        .args(["checkout", "-b", "feature/sec"])
        .status()
        .unwrap();
    std::fs::write(work.join("sec.txt"), b"x").unwrap();
    Command::new("git")
        .current_dir(&work)
        .args(["add", "sec.txt"])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(&work)
        .args([
            "-c",
            "user.email=s@musehub.local",
            "-c",
            "user.name=s",
            "commit",
            "-m",
            "sec",
            "--no-gpg-sign",
        ])
        .status()
        .unwrap();
    let head = String::from_utf8_lossy(
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
        .current_dir(&work)
        .args(["push", "origin", "HEAD:refs/heads/feature/sec"])
        .status()
        .unwrap();
    let pr: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/pipe-sec/pull-requests")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, admin_cookie)
                    .body(Body::from(
                        serde_json::json!({
                            "title": "sec",
                            "source_branch": "feature/sec"
                        })
                        .to_string(),
                    ))
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
    (pr["number"].as_i64().unwrap(), head)
}

#[tokio::test]
async fn member_cannot_issue_checks_write_pat() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool, git_root.path(), &bootstrap).await;
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
                        "handle": "pipe-member",
                        "password": MEMBER_PASS,
                        "role": "member"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let member_cookie = common::login_cookie(&app, "pipe-member", MEMBER_PASS).await;
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &member_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "name": "bad",
                        "org": "demo",
                        "scopes": ["checks:write"],
                        "runner_handle": "pipe-member"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_cannot_delegate_runner_token_to_self() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool, git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "name": "self-runner",
                        "org": "demo",
                        "scopes": ["checks:write"],
                        "runner_handle": "admin"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn pipeline_rejects_unmerged_source_oid() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let (_pr_number, head) =
        seed_repo_with_open_pr(&app, &pool, git_root.path(), &admin_cookie).await;
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/pipe-sec/pipeline-runs")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({ "source_oid": head }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn release_blocked_when_pipeline_failed() {
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
                .body(Body::from(r#"{"slug":"rel-fail","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let repo_id: Uuid = sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM repositories WHERE slug_normalized = 'rel-fail'",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .0;
    let org_id: Uuid = sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM organizations WHERE slug_normalized = 'demo'",
    )
        .fetch_one(&pool)
        .await
        .unwrap()
        .0;
    let admin_id: Uuid = sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM principals WHERE handle_normalized = 'admin'",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .0;
    let run_id = Uuid::new_v4();
    let attempt = Uuid::new_v4();
    let oid = "a".repeat(40);
    sqlx::query(
        "INSERT INTO pipeline_runs (id, org_id, repo_id, source_oid, workflow_name, state,
         requested_by, run_attempt_id, cancel_requested)
         VALUES ($1,$2,$3,$4,'product-ci','failed',$5,$6,FALSE)",
    )
    .bind(run_id)
    .bind(org_id)
    .bind(repo_id)
    .bind(&oid)
    .bind(admin_id)
    .bind(attempt)
    .execute(&pool)
    .await
    .unwrap();
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/rel-fail/releases")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "version": "0.0.1",
                        "tag_name": "v0.0.1",
                        "commit_oid": oid,
                        "pipeline_run_id": run_id
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}
