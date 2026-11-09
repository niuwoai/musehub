//! Cancel agent run: process group kill, no commit after cancel.

mod common;

use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;


#[tokio::test]
async fn slow_agent_cancel_leaves_no_commit() {
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

    let create_repo = serde_json::json!({"slug": "cancel", "description": "e2e"});
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

    let issue_body = serde_json::json!({"title": "cancel issue"});
    let issue_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/cancel/issues")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(issue_body.to_string()))
                .unwrap(),
        )
        .await
        .expect("issue");
    let issue_json: serde_json::Value =
        serde_json::from_slice(&issue_resp.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    let issue_number = issue_json["number"].as_i64().unwrap();

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/cancel/issues/{issue_number}/plans"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"body":"plan"}"#))
                .unwrap(),
        )
        .await
        .expect("plan");
    app.clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/cancel/issues/{issue_number}/plans"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("accept");

    let agent_body = serde_json::json!({
        "issue_id": issue_json["id"],
        "message": "__slow__"
    });
    let agent_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/cancel/agent-runs")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(agent_body.to_string()))
                .unwrap(),
        )
        .await
        .expect("agent");
    assert_eq!(agent_resp.status(), StatusCode::OK);
    let agent_json: serde_json::Value =
        serde_json::from_slice(&agent_resp.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    let run_id = agent_json["id"].as_str().unwrap();

    tokio::time::sleep(Duration::from_millis(400)).await;
    let cancel = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/agent-runs/{run_id}/cancel"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("cancel");
    assert_eq!(cancel.status(), StatusCode::ACCEPTED);

    let mut final_state = String::new();
    for _ in 0..80 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let get = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v1/agent-runs/{run_id}"))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("get run");
        let run: serde_json::Value =
            serde_json::from_slice(&get.into_body().collect().await.unwrap().to_bytes()).unwrap();
        final_state = run["state"].as_str().unwrap_or("").to_string();
        if final_state == "canceled" || final_state == "failed" {
            break;
        }
    }
    assert_eq!(final_state, "canceled");
    let oid: Option<String> = sqlx::query_as::<_, (Option<String>,)>(
        "SELECT result_commit_oid FROM agent_runs WHERE id = $1",
    )
    .bind(uuid::Uuid::parse_str(run_id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap()
    .0;
    assert!(oid.is_none());
}
