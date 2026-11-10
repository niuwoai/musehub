//! Runner PAT: pr_validation pipeline → check report → merge (distinct from release_build).

mod common;

use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

const RUNNER_PASS: &str = "Runner-Pat-E2E-12!";
const REVIEWER_PASS: &str = "Runner-Rev-E2E-12!";

async fn wait_pipeline(
    app: &axum::Router,
    cookie: &str,
    slug: &str,
    id: &str,
) -> serde_json::Value {
    for _ in 0..120 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let st: serde_json::Value = serde_json::from_slice(
            &app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/v1/orgs/demo/repos/{slug}/pipeline-runs/{id}"))
                        .header(header::COOKIE, cookie)
                        .body(Body::empty())
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
        if st["state"] == "succeeded" || st["state"] == "failed" {
            return st;
        }
    }
    panic!("pipeline timeout");
}

#[tokio::test]
async fn runner_pat_validation_report_then_merge() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool, git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    for (handle, pass, role) in [
        ("runner-pat-rev", REVIEWER_PASS, "member"),
        ("runner-ci", RUNNER_PASS, "member"),
    ] {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/members")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::from(
                        serde_json::json!({
                            "handle": handle,
                            "password": pass,
                            "role": role
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
    }
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"slug":"runner-pat","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/runner-pat/collaborators")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    r#"{"handle":"runner-pat-rev","role":"maintainer"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let rev_cookie = common::login_cookie(&app, "runner-pat-rev", REVIEWER_PASS).await;

    let issue: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/runner-pat/issues")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::from(r#"{"title":"rp"}"#))
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
                .uri(format!(
                    "/api/v1/orgs/demo/repos/runner-pat/issues/{n}/plans"
                ))
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
                .uri(format!(
                    "/api/v1/orgs/demo/repos/runner-pat/issues/{n}/plans"
                ))
                .header(header::COOKIE, &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let agent: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/runner-pat/agent-runs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::from(
                        serde_json::json!({
                            "issue_id": issue["id"].as_str().unwrap(),
                            "message": "build"
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
    let run_id = agent["id"].as_str().unwrap();
    for _ in 0..120 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let st: serde_json::Value = serde_json::from_slice(
            &app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/v1/agent-runs/{run_id}"))
                        .header(header::COOKIE, &admin_cookie)
                        .body(Body::empty())
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
        if st["state"] == "succeeded" {
            break;
        }
        if st["state"] == "failed" {
            panic!("agent failed");
        }
    }
    let branch = format!("agent/{run_id}");
    let pr: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/runner-pat/pull-requests")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::from(
                        serde_json::json!({
                            "title": "runner",
                            "source_branch": branch
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
    let pr_number = pr["number"].as_i64().unwrap();

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/runner-pat/pull-requests/{pr_number}/reviews"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &rev_cookie)
                .body(Body::from(r#"{"decision":"approve"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    let validation: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/v1/orgs/demo/repos/runner-pat/pull-requests/{pr_number}/validation-runs"
                    ))
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::empty())
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
    let pipe_id = validation["id"].as_str().unwrap();
    let pipe = wait_pipeline(&app, &admin_cookie, "runner-pat", pipe_id).await;
    assert_eq!(pipe["state"], "succeeded");
    let attempt = pipe["run_attempt_id"].as_str().unwrap();
    let head_oid = validation["source_oid"].as_str().unwrap();

    let pat: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/tokens")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::from(
                        serde_json::json!({
                            "name": "runner",
                            "org": "demo",
                            "scopes": ["checks:write"],
                            "scope_mode": "all_accessible",
                            "runner_handle": "runner-ci"
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
    let token = pat["token"].as_str().unwrap();

    let report = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/runner-pat/runner/check-reports")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    serde_json::json!({
                        "pr_number": pr_number,
                        "check_name": "product-ci",
                        "head_oid": head_oid,
                        "state": "success",
                        "attempt_id": attempt,
                        "pipeline_run_id": pipe_id
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(report.status(), StatusCode::NO_CONTENT);

    let replay = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/runner-pat/runner/check-reports")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    serde_json::json!({
                        "pr_number": pr_number,
                        "check_name": "product-ci",
                        "head_oid": head_oid,
                        "state": "success",
                        "attempt_id": attempt,
                        "pipeline_run_id": pipe_id
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::BAD_REQUEST);

    let merged = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/runner-pat/pull-requests/{pr_number}/merge"
                ))
                .header(header::COOKIE, &rev_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(merged.status(), StatusCode::OK);
}
