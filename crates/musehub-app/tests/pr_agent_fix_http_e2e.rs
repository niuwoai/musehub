//! PR CI failure → bounded agent fix (`docs/01-product-requirements.md` §2 S3).

mod common;

use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt;

const CI_PASS: &str = "Ci-Reporter-Fix-E2e-12!";

async fn bootstrap_ci_fix_user(app: &axum::Router, admin_cookie: &str) -> (String, String) {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/members")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                      "handle": "ci-fix",
                      "password": CI_PASS,
                      "role": "admin"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let ci_cookie = common::login_cookie(app, "ci-fix", CI_PASS).await;
    (ci_cookie, CI_PASS.to_string())
}

async fn seed_repo_issue_plan_run(
    app: &axum::Router,
    admin_cookie: &str,
    repo_slug: &str,
) -> (String, String) {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, admin_cookie)
                .body(Body::from(
                    serde_json::json!({ "slug": repo_slug, "description": "" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let issue_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/demo/repos/{repo_slug}/issues"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, admin_cookie)
                .body(Body::from(r#"{"title":"fix slice"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(issue_resp.status(), StatusCode::CREATED);
    let issue_json: serde_json::Value =
        serde_json::from_slice(&issue_resp.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    let issue_id = issue_json["id"].as_str().unwrap().to_string();
    let issue_number = issue_json["number"].as_i64().unwrap();

    for (method, accept) in [("POST", None), ("PATCH", Some(r#"{"accept":true}"#))] {
        app.clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!(
                        "/api/v1/orgs/demo/repos/{repo_slug}/issues/{issue_number}/plans"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, admin_cookie)
                    .body(Body::from(accept.unwrap_or(r#"{"body":"plan"}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
    }

    let run_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/demo/repos/{repo_slug}/agent-runs"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, admin_cookie)
                .body(Body::from(
                    serde_json::json!({ "issue_id": issue_id, "message": "implement" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(run_resp.status(), StatusCode::OK);
    let run_json: serde_json::Value =
        serde_json::from_slice(&run_resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let run_id = run_json["id"].as_str().unwrap().to_string();

    for _ in 0..120 {
        let st = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v1/agent-runs/{run_id}"))
                    .header(header::COOKIE, admin_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let st_json: serde_json::Value =
            serde_json::from_slice(&st.into_body().collect().await.unwrap().to_bytes()).unwrap();
        if st_json["state"].as_str() == Some("succeeded") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    (issue_id, run_id)
}

async fn storage_key_for_repo(pool: &PgPool, slug: &str) -> uuid::Uuid {
    sqlx::query_as::<_, (uuid::Uuid,)>("SELECT storage_key FROM repositories WHERE slug = $1")
        .bind(slug)
        .fetch_one(pool)
        .await
        .expect("repo")
        .0
}

fn bare_repo_update_ref(
    git_root: &std::path::Path,
    storage_key: uuid::Uuid,
    branch: &str,
    oid: &str,
) {
    let bare = git_root.join(format!("{storage_key}.git"));
    let status = std::process::Command::new("git")
        .current_dir(&bare)
        .args(["update-ref", &format!("refs/heads/{branch}"), oid])
        .status()
        .expect("git update-ref");
    assert!(status.success());
}

async fn record_ci_failure(app: &axum::Router, ci_cookie: &str, repo_slug: &str, pr_number: i64) {
    app.clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo_slug}/pull-requests/{pr_number}/checks/product-ci"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, ci_cookie)
                .body(Body::from(
                    serde_json::json!({
                      "state": "failure",
                      "attempt_id": uuid::Uuid::new_v4().to_string()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn pr_agent_fix_requires_ci_failure_and_respects_max_rounds() {
    std::env::set_var("MUSEHUB_AGENT_FIX_MAX_ROUNDS", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let (ci_cookie, _) = bootstrap_ci_fix_user(&app, &admin_cookie).await;

    let (_issue_id, run_id) = seed_repo_issue_plan_run(&app, &admin_cookie, "pr-agent-fix").await;

    let pr_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/pr-agent-fix/pull-requests")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                      "title": "agent pr",
                      "source_branch": format!("agent/{run_id}"),
                      "issue_id": _issue_id
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pr_resp.status(), StatusCode::CREATED);
    let pr_json: serde_json::Value =
        serde_json::from_slice(&pr_resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let pr_number = pr_json["number"].as_i64().unwrap();
    let head = pr_json["head_oid"].as_str().unwrap();

    let deny = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pr-agent-fix/pull-requests/{pr_number}/ci-failure"
                ))
                .header(header::COOKIE, &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deny.status(), StatusCode::OK);
    let deny_json: serde_json::Value =
        serde_json::from_slice(&deny.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(deny_json["agent_fix_allowed"].as_bool(), Some(false));

    record_ci_failure(&app, &ci_cookie, "pr-agent-fix", pr_number).await;

    let ctx = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pr-agent-fix/pull-requests/{pr_number}/ci-failure"
                ))
                .header(header::COOKIE, &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let ctx_json: serde_json::Value =
        serde_json::from_slice(&ctx.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(ctx_json["agent_fix_allowed"].as_bool(), Some(true));
    assert_eq!(ctx_json["head_oid"].as_str(), Some(head));
    assert_eq!(ctx_json["fix_rounds_max"].as_i64(), Some(1));

    let fix = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pr-agent-fix/pull-requests/{pr_number}/agent-fix"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fix.status(), StatusCode::CREATED);
    let fix_json: serde_json::Value =
        serde_json::from_slice(&fix.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(fix_json["fix_round"].as_i64(), Some(1));
    assert_eq!(fix_json["head_oid"].as_str(), Some(head));
    let fix_run = fix_json["id"].as_str().unwrap();

    for _ in 0..120 {
        let st = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v1/agent-runs/{fix_run}"))
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let st_json: serde_json::Value =
            serde_json::from_slice(&st.into_body().collect().await.unwrap().to_bytes()).unwrap();
        if ["succeeded", "failed", "canceled"].contains(&st_json["state"].as_str().unwrap_or("")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    let ctx2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pr-agent-fix/pull-requests/{pr_number}/ci-failure"
                ))
                .header(header::COOKIE, &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let ctx2_json: serde_json::Value =
        serde_json::from_slice(&ctx2.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(ctx2_json["fix_rounds_used"].as_i64(), Some(1));
    assert_eq!(ctx2_json["agent_fix_allowed"].as_bool(), Some(false));
    let denial = ctx2_json["agent_fix_denial_reason"].as_str().unwrap();
    assert!(
        denial.contains("maximum agent fix rounds") || denial.contains("stale"),
        "unexpected denial: {denial}"
    );

    let fix2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pr-agent-fix/pull-requests/{pr_number}/agent-fix"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fix2.status(), StatusCode::BAD_REQUEST);

    std::env::remove_var("MUSEHUB_AGENT_FIX_MAX_ROUNDS");
}

#[tokio::test]
async fn pr_agent_fix_allows_non_agent_source_branch() {
    std::env::set_var("MUSEHUB_AGENT_FIX_MAX_ROUNDS", "2");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let (ci_cookie, _) = bootstrap_ci_fix_user(&app, &admin_cookie).await;

    let repo_slug = "pr-agent-fix-feature-branch";
    let (issue_id, run_id) = seed_repo_issue_plan_run(&app, &admin_cookie, repo_slug).await;

    let head_row: (String,) =
        sqlx::query_as("SELECT result_commit_oid FROM agent_runs WHERE id = $1::uuid")
            .bind(&run_id)
            .fetch_one(&pool)
            .await
            .expect("run commit");
    let head = head_row.0;
    let storage_key = storage_key_for_repo(&pool, repo_slug).await;
    let feature_branch = "feature/ci-fix-e2e";
    bare_repo_update_ref(git_root.path(), storage_key, feature_branch, &head);

    let pr_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/demo/repos/{repo_slug}/pull-requests"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                      "title": "feature branch pr",
                      "source_branch": feature_branch,
                      "issue_id": issue_id
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pr_resp.status(), StatusCode::CREATED);
    let pr_json: serde_json::Value =
        serde_json::from_slice(&pr_resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let pr_number = pr_json["number"].as_i64().unwrap();

    record_ci_failure(&app, &ci_cookie, repo_slug, pr_number).await;

    let fix = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo_slug}/pull-requests/{pr_number}/agent-fix"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fix.status(), StatusCode::CREATED);

    std::env::remove_var("MUSEHUB_AGENT_FIX_MAX_ROUNDS");
}

#[tokio::test]
async fn pr_agent_fix_concurrent_post_cannot_double_insert() {
    std::env::set_var("MUSEHUB_AGENT_FIX_MAX_ROUNDS", "3");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let (ci_cookie, _) = bootstrap_ci_fix_user(&app, &admin_cookie).await;

    let repo_slug = "pr-agent-fix-race";
    let (issue_id, run_id) = seed_repo_issue_plan_run(&app, &admin_cookie, repo_slug).await;

    let pr_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/demo/repos/{repo_slug}/pull-requests"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                      "title": "race pr",
                      "source_branch": format!("agent/{run_id}"),
                      "issue_id": issue_id
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pr_resp.status(), StatusCode::CREATED);
    let pr_json: serde_json::Value =
        serde_json::from_slice(&pr_resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let pr_number = pr_json["number"].as_i64().unwrap();

    record_ci_failure(&app, &ci_cookie, repo_slug, pr_number).await;

    let uri = format!("/api/v1/orgs/demo/repos/{repo_slug}/pull-requests/{pr_number}/agent-fix");
    let app_a = app.clone();
    let app_b = app.clone();
    let cookie_a = admin_cookie.clone();
    let cookie_b = admin_cookie.clone();
    let uri_b = uri.clone();
    let (ra, rb) = tokio::join!(
        async move {
            app_a
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(&uri)
                        .header(header::CONTENT_TYPE, "application/json")
                        .header(header::COOKIE, &cookie_a)
                        .body(Body::from("{}"))
                        .unwrap(),
                )
                .await
                .unwrap()
        },
        async move {
            app_b
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(uri_b)
                        .header(header::CONTENT_TYPE, "application/json")
                        .header(header::COOKIE, &cookie_b)
                        .body(Body::from("{}"))
                        .unwrap(),
                )
                .await
                .unwrap()
        }
    );

    let statuses = [ra.status().as_u16(), rb.status().as_u16()];
    assert!(statuses.contains(&201));
    assert!(statuses.contains(&400));

    let in_flight: (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::bigint FROM agent_runs WHERE pr_id = (
            SELECT id FROM pull_requests WHERE repo_id = (
                SELECT id FROM repositories WHERE slug = $1
            ) AND number = $2
        ) AND state IN ('queued','running','cancel_requested','waiting_input')",
    )
    .bind(repo_slug)
    .bind(pr_number)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(in_flight.0 <= 1);

    std::env::remove_var("MUSEHUB_AGENT_FIX_MAX_ROUNDS");
}
