//! PR merge gates: two principals, checks, head SHA, CAS merge.

mod common;

use std::process::Command;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use musehub_app::bare_repo_path;

const DEFAULT_REPO_CHECK: &str = "product-ci";
use tower::ServiceExt;

const REVIEWER_PASS: &str = "Reviewer-Strong-Pass-12!";
const CI_REPORTER_PASS: &str = "Ci-Reporter-Strong-12!";

#[tokio::test]
async fn pr_merge_gates_two_users_full_chain() {
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
                        "handle": "reviewer",
                        "password": REVIEWER_PASS,
                        "role": "member"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .expect("invite");
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/members")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
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
        .expect("invite ci");
    let ci_cookie = common::login_cookie(&app, "ci-reporter", CI_REPORTER_PASS).await;

    // repo + agent branch
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"slug":"pr-gates","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/pr-gates/collaborators")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"handle":"reviewer","role":"maintainer"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    let issue_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/pr-gates/issues")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"title":"t"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let issue: serde_json::Value =
        serde_json::from_slice(&issue_resp.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    let issue_number = issue["number"].as_i64().unwrap();
    for step in ["plan", "accept"] {
        if step == "plan" {
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!(
                            "/api/v1/orgs/demo/repos/pr-gates/issues/{issue_number}/plans"
                        ))
                        .header(header::CONTENT_TYPE, "application/json")
                        .header(header::COOKIE, &admin_cookie)
                        .body(Body::from(r#"{"body":"p"}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
        } else {
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("PATCH")
                        .uri(format!(
                            "/api/v1/orgs/demo/repos/pr-gates/issues/{issue_number}/plans"
                        ))
                        .header(header::COOKIE, &admin_cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
        }
    }
    let agent = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/pr-gates/agent-runs")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({"issue_id": issue["id"], "message": "work"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let agent_json: serde_json::Value =
        serde_json::from_slice(&agent.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let run_id = agent_json["id"].as_str().unwrap();
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
        if run["state"] == "failed" {
            panic!("agent failed: {run:?}");
        }
    }
    let source_branch = format!("agent/{run_id}");

    let pr_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/pr-gates/pull-requests")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "title": "Agent work",
                        "source_branch": source_branch
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .expect("create pr");
    assert_eq!(pr_resp.status(), StatusCode::CREATED);
    let pr_json: serde_json::Value =
        serde_json::from_slice(&pr_resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let pr_number = pr_json["number"].as_i64().unwrap();

    let self_review = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pr-gates/pull-requests/{pr_number}/reviews"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"decision":"approve"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(self_review.status(), StatusCode::BAD_REQUEST);

    let merge_early = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pr-gates/pull-requests/{pr_number}/merge"
                ))
                .header(header::COOKIE, &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(merge_early.status(), StatusCode::BAD_REQUEST);

    let reviewer_cookie = common::login_cookie(&app, "reviewer", REVIEWER_PASS).await;
    let approve = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pr-gates/pull-requests/{pr_number}/reviews"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &reviewer_cookie)
                .body(Body::from(r#"{"decision":"approve"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approve.status(), StatusCode::CREATED);

    let merge_no_check = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pr-gates/pull-requests/{pr_number}/merge"
                ))
                .header(header::COOKIE, &reviewer_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(merge_no_check.status(), StatusCode::BAD_REQUEST);

    let forged_check = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pr-gates/pull-requests/{pr_number}/checks/{DEFAULT_REPO_CHECK}"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &reviewer_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "state": "success",
                        "attempt_id": uuid::Uuid::new_v4()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(forged_check.status(), StatusCode::FORBIDDEN);

    let check_ok = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pr-gates/pull-requests/{pr_number}/checks/{DEFAULT_REPO_CHECK}"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &ci_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "state": "success",
                        "attempt_id": uuid::Uuid::new_v4()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(check_ok.status(), StatusCode::NO_CONTENT);

    let merge_ok = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pr-gates/pull-requests/{pr_number}/merge"
                ))
                .header(header::COOKIE, &reviewer_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let merge_status = merge_ok.status();
    if merge_status != StatusCode::OK {
        let body = merge_ok.into_body().collect().await.unwrap().to_bytes();
        panic!(
            "merge failed {merge_status}: {}",
            String::from_utf8_lossy(&body)
        );
    }

    let storage_key: uuid::Uuid = sqlx::query_as::<_, (uuid::Uuid,)>(
        "SELECT storage_key FROM repositories WHERE slug_normalized = 'pr-gates'",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .0;
    let bare = bare_repo_path(git_root.path(), &storage_key);
    assert!(common::branch_commit_count(&bare, "main") >= 2);
}

#[tokio::test]
async fn stale_approval_after_new_commit_requires_resync() {
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
                    serde_json::json!({"handle":"rev2","password":REVIEWER_PASS,"role":"member"})
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
                .body(Body::from(r#"{"slug":"stale","description":""}"#))
                .unwrap(),
        )
        .await
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
    let ci_cookie = common::login_cookie(&app, "ci-reporter", CI_REPORTER_PASS).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/stale/collaborators")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"handle":"rev2","role":"maintainer"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    let storage_key: uuid::Uuid = sqlx::query_as::<_, (uuid::Uuid,)>(
        "SELECT storage_key FROM repositories WHERE slug_normalized = 'stale'",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .0;
    let bare = bare_repo_path(git_root.path(), &storage_key);
    Command::new("git")
        .args([
            "clone",
            bare.to_str().unwrap(),
            git_root.path().join("work-stale").to_str().unwrap(),
        ])
        .status()
        .unwrap();
    let work = git_root.path().join("work-stale");
    Command::new("git")
        .current_dir(&work)
        .args(["checkout", "-B", "main", "origin/main"])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(&work)
        .args(["checkout", "-b", "feature/stale"])
        .status()
        .unwrap();
    std::fs::write(work.join("one.txt"), b"1").unwrap();
    Command::new("git")
        .current_dir(&work)
        .args(["add", "one.txt"])
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
            "one",
            "--no-gpg-sign",
        ])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(&work)
        .args(["push", "origin", "HEAD:refs/heads/feature/stale"])
        .status()
        .unwrap();

    let pr = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/stale/pull-requests")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "title": "stale",
                        "source_branch": "feature/stale"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let pr_json: serde_json::Value =
        serde_json::from_slice(&pr.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let pr_number = pr_json["number"].as_i64().unwrap();
    let rev_cookie = common::login_cookie(&app, "rev2", REVIEWER_PASS).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/stale/pull-requests/{pr_number}/reviews"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &rev_cookie)
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
                    "/api/v1/orgs/demo/repos/stale/pull-requests/{pr_number}/checks/{DEFAULT_REPO_CHECK}"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &ci_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "state": "success",
                        "attempt_id": uuid::Uuid::new_v4()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    std::fs::write(work.join("two.txt"), b"2").unwrap();
    Command::new("git")
        .current_dir(&work)
        .args(["add", "two.txt"])
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
            "two",
            "--no-gpg-sign",
        ])
        .status()
        .unwrap();
    Command::new("git")
        .current_dir(&work)
        .args(["push", "origin", "HEAD:refs/heads/feature/stale"])
        .status()
        .unwrap();

    let blocked = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/stale/pull-requests/{pr_number}/merge"
                ))
                .header(header::COOKIE, &rev_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::BAD_REQUEST);

    app.clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/stale/pull-requests/{pr_number}/sync"
                ))
                .header(header::COOKIE, &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/stale/pull-requests/{pr_number}/reviews"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &rev_cookie)
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
                    "/api/v1/orgs/demo/repos/stale/pull-requests/{pr_number}/checks/{DEFAULT_REPO_CHECK}"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &ci_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "state": "success",
                        "attempt_id": uuid::Uuid::new_v4()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let merged = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/stale/pull-requests/{pr_number}/merge"
                ))
                .header(header::COOKIE, &rev_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(merged.status(), StatusCode::OK);
    let _ = bare;
}
