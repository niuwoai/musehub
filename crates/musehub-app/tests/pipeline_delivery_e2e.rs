//! Pipeline → release → compose-mock deployment → trace chain.

mod common;

use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request};
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

const CI_PASS: &str = "Ci-Delivery-Reporter-12!";
const REVIEWER_PASS: &str = "Delivery-Reviewer-12!";

#[tokio::test]
async fn merge_then_pipeline_release_deploy_trace() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    for (handle, pass, role) in [
        ("delivery-rev", REVIEWER_PASS, "member"),
        ("ci-delivery", CI_PASS, "admin"),
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
    let ci_cookie = common::login_cookie(&app, "ci-delivery", CI_PASS).await;

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"slug":"delivery","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/delivery/collaborators")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    r#"{"handle":"delivery-rev","role":"maintainer"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let issue: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/delivery/issues")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::from(r#"{"title":"d"}"#))
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
    let issue_uuid = Uuid::parse_str(issue["id"].as_str().unwrap()).unwrap();
    let n = issue["number"].as_i64().unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/demo/repos/delivery/issues/{n}/plans"))
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
                .uri(format!("/api/v1/orgs/demo/repos/delivery/issues/{n}/plans"))
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
                    .uri("/api/v1/orgs/demo/repos/delivery/agent-runs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::from(
                        serde_json::json!({
                            "issue_id": issue_uuid,
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
    }
    let agent_run_uuid = Uuid::parse_str(run_id).unwrap();
    let branch = format!("agent/{run_id}");

    let pr: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/delivery/pull-requests")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::from(
                        serde_json::json!({
                            "title": "delivery",
                            "source_branch": branch,
                            "issue_id": issue_uuid,
                            "agent_run_id": agent_run_uuid
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
    let pr_id = pr["id"].as_str().unwrap();
    let rev_cookie = common::login_cookie(&app, "delivery-rev", REVIEWER_PASS).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/delivery/pull-requests/{pr_number}/reviews"
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
                    "/api/v1/orgs/demo/repos/delivery/pull-requests/{pr_number}/checks/product-ci"
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
    let merged: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/v1/orgs/demo/repos/delivery/pull-requests/{pr_number}/merge"
                    ))
                    .header(header::COOKIE, &rev_cookie)
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
    let merge_commit = merged["head_oid"].as_str().unwrap();

    let pipe: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/delivery/pipeline-runs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::from(
                        serde_json::json!({
                            "source_oid": merge_commit,
                            "pr_id": pr_id,
                            "issue_id": issue_uuid,
                            "agent_run_id": run_id
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
    let pipeline_id = pipe["id"].as_str().unwrap();
    for _ in 0..120 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let st: serde_json::Value = serde_json::from_slice(
            &app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!(
                            "/api/v1/orgs/demo/repos/delivery/pipeline-runs/{pipeline_id}"
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
        if st["state"] == "succeeded" || st["state"] == "failed" {
            assert_eq!(
                st["state"], "succeeded",
                "pipeline error={:?}",
                st["error_code"]
            );
            break;
        }
    }

    let rel: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/delivery/releases")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::from(
                        serde_json::json!({
                            "version": "0.1.0-rc4.1",
                            "tag_name": "v0.1.0-rc4.1",
                            "commit_oid": merge_commit,
                            "pipeline_run_id": pipeline_id,
                            "pr_id": pr_id
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
    let release_id = rel["id"].as_str().unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/delivery/releases/{release_id}/publish"
                ))
                .header(header::COOKIE, &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let dep: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/delivery/deployments")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::from(
                        serde_json::json!({
                            "release_id": release_id,
                            "health_target": "mock://healthy"
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
    assert_eq!(dep["state"], "succeeded");
    let deployment_id = dep["id"].as_str().unwrap();

    let trace: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/api/v1/orgs/demo/repos/delivery/trace/deployments/{deployment_id}"
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
    assert_eq!(trace["pipeline_run_id"].as_str().unwrap(), pipeline_id);
    assert_eq!(trace["commit_oid"].as_str().unwrap(), merge_commit);
    assert!(trace["artifacts"].as_array().is_some_and(|a| !a.is_empty()));
    assert_eq!(trace["agent_run_id"].as_str().unwrap(), run_id);
    assert_eq!(trace["adapter_is_mock"].as_bool(), Some(true));
    assert_eq!(trace["pr_number"].as_i64(), Some(pr_number));
    assert_eq!(trace["merged_oid"].as_str(), Some(merge_commit));
    assert_eq!(trace["plan_accepted"].as_bool(), Some(true));
    let gaps = trace["gaps"].as_array().unwrap();
    let gap_text: Vec<&str> = gaps.iter().filter_map(|g| g.as_str()).collect();
    for expected in ["deploy_not_isolated_compose", "deploy_no_oci_registry"] {
        assert!(
            gap_text.contains(&expected),
            "missing trace gap {expected}: {gap_text:?}"
        );
    }
}
