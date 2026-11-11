//! PR CI failure → bounded agent fix (`docs/01-product-requirements.md` §2 S3).

mod common;

use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

const CI_PASS: &str = "Ci-Reporter-Fix-E2e-12!";

#[tokio::test]
async fn pr_agent_fix_requires_ci_failure_and_respects_max_rounds() {
    std::env::set_var("MUSEHUB_AGENT_FIX_MAX_ROUNDS", "1");
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
    let ci_cookie = common::login_cookie(&app, "ci-fix", CI_PASS).await;

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"slug":"pr-agent-fix","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    let issue_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/pr-agent-fix/issues")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"title":"fix slice"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(issue_resp.status(), StatusCode::CREATED);
    let issue_json: serde_json::Value =
        serde_json::from_slice(&issue_resp.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    let issue_id = issue_json["id"].as_str().unwrap();

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pr-agent-fix/issues/{}/plans",
                    issue_json["number"]
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"body":"plan"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/pr-agent-fix/issues/{}/plans",
                    issue_json["number"]
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"accept":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    let run_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/pr-agent-fix/agent-runs")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
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
    let run_id = run_json["id"].as_str().unwrap();

    for _ in 0..120 {
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
        let st_json: serde_json::Value =
            serde_json::from_slice(&st.into_body().collect().await.unwrap().to_bytes()).unwrap();
        if st_json["state"].as_str() == Some("succeeded") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

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

    app.clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
          "/api/v1/orgs/demo/repos/pr-agent-fix/pull-requests/{pr_number}/checks/product-ci"
        ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &ci_cookie)
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
    assert!(ctx2_json["agent_fix_denial_reason"]
        .as_str()
        .unwrap()
        .contains("maximum agent fix rounds"));

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
