//! Remote agent executor: API enqueue-only; worker claims and runs reference adapter.

mod common;

use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use musehub_app::agent_supervisor::new_supervisor;
use musehub_app::execute_agent_job;
use tower::ServiceExt;
use uuid::Uuid;

const RUNNER_PASS: &str = "Remote-Agent-Member-12!";

#[tokio::test]
async fn remote_worker_claims_and_completes_agent_run() {
    std::env::set_var("MUSEHUB_AGENT_EXECUTOR", "remote");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
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
                        "handle": "remote-agent",
                        "password": RUNNER_PASS,
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
                .body(Body::from(r#"{"slug":"remote-agent","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let repo_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM repositories WHERE slug = 'remote-agent'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let storage_key: (Uuid,) =
        sqlx::query_as("SELECT storage_key FROM repositories WHERE slug = 'remote-agent'")
            .fetch_one(&pool)
            .await
            .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/remote-agent/collaborators")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "handle": "remote-agent",
                        "role": "developer"
                    })
                    .to_string(),
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
                .uri("/api/v1/orgs/demo/repos/remote-agent/issues")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"title":"agent remote"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let issue: serde_json::Value =
        serde_json::from_slice(&issue_resp.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    let issue_id = issue["id"].as_str().unwrap();
    let issue_number = issue["number"].as_i64().unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/remote-agent/issues/{issue_number}/plans"
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
                    "/api/v1/orgs/demo/repos/remote-agent/issues/{issue_number}/plans"
                ))
                .header(header::COOKIE, &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let pat_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "name": "agent-runner",
                        "org": "demo",
                        "scopes": ["agent:execute", "repo:read"],
                        "scope_mode": "selected",
                        "repo_ids": [repo_id.0],
                        "runner_handle": "remote-agent"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let pat_body: serde_json::Value =
        serde_json::from_slice(&pat_resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let token = pat_body["token"].as_str().unwrap();
    let agent_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/remote-agent/agent-runs")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "issue_id": issue_id,
                        "message": "remote agent task"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(agent_resp.status(), StatusCode::OK);
    let agent_json: serde_json::Value =
        serde_json::from_slice(&agent_resp.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    assert_eq!(agent_json["state"], "queued");
    let run_id = Uuid::parse_str(agent_json["id"].as_str().unwrap()).unwrap();
    let runner_id = Uuid::new_v4();
    let claim = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/runner/agent-runs/claim")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("x-musehub-runner-id", runner_id.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(claim.status(), StatusCode::OK);
    let claimed: serde_json::Value =
        serde_json::from_slice(&claim.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(claimed["run_id"], run_id.to_string());
    let branch = claimed["work_branch"].as_str().unwrap().to_string();
    let message = claimed["task_message"].as_str().unwrap().to_string();
    let supervisor = new_supervisor();
    let reference_bin = common::reference_adapter_bin();
    execute_agent_job(
        pool.clone(),
        supervisor,
        runner_id,
        run_id,
        repo_id.0,
        storage_key.0,
        git_root.path().to_path_buf(),
        reference_bin,
        branch,
        message,
        Duration::from_secs(120),
        None,
        None,
    )
    .await;
    let state: (String,) = sqlx::query_as("SELECT state FROM agent_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state.0, "succeeded");
    std::env::remove_var("MUSEHUB_AGENT_EXECUTOR");
}
