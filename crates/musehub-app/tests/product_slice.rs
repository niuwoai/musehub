//! Vertical slice integration (requires `MUSEHUB_TEST_DATABASE_URL`).

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use std::time::Duration;

use musehub_app::agent_supervisor::new_supervisor;
use musehub_app::{boot_reconcile, lease, prepare, product_router, AppState};
use musehub_migrate::{load_migrations, migrate_up};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

mod common;

#[tokio::test]
async fn login_create_repo_issue_reference_agent() {
    let database_url =
        std::env::var("MUSEHUB_TEST_DATABASE_URL").expect("MUSEHUB_TEST_DATABASE_URL");
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .expect("connect");
    let migrations =
        load_migrations(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../migrations"))
            .expect("migrations");
    let _ = migrate_up(&pool, &migrations).await.expect("migrate");

    let git_root = tempfile::tempdir().expect("git root");
    let bootstrap = common::bootstrap_password();
    prepare(
        pool.clone(),
        git_root.path().to_path_buf(),
        Some(&bootstrap),
    )
    .await
    .expect("prepare");
    boot_reconcile(&pool).await.expect("reconcile");

    let adapter = std::env::var("CARGO_BIN_EXE_musehub_agent_reference")
        .ok()
        .or_else(|| std::env::var("CARGO_BIN_EXE_musehub-agent-reference").ok())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/debug/musehub-agent-reference")
        });
    assert!(
        adapter.is_file(),
        "missing {} (run: cargo build -p musehub-agent-adapter --bin musehub-agent-reference)",
        adapter.display()
    );
    let state = Arc::new(AppState {
        pool: pool.clone(),
        git_root: git_root.path().to_path_buf(),
        artifact_root: git_root.path().join("artifacts"),
        bind_display: "127.0.0.1:8080".to_string(),
        reference_adapter_bin: adapter.clone(),
        pipeline_adapter_bin: std::env::var("CARGO_BIN_EXE_musehub-pipeline-reference")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/debug/musehub-pipeline-reference")
            }),
        compose_adapter_bin: std::env::var("CARGO_BIN_EXE_musehub-compose-mock")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/debug/musehub-compose-mock")
            }),
        compose_runner_bin: std::env::var("CARGO_BIN_EXE_musehub-compose-runner")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/debug/musehub-compose-runner")
            }),
        instance_id: lease::parse_instance_id(),
        supervisor: new_supervisor(),
        pipeline_supervisor: musehub_app::pipeline_supervisor::PipelineSupervisor::new(),
        require_pipeline_oci: false,
        mcp_pat_slot: Arc::new(tokio::sync::RwLock::new(None)),
        mcp_session_pats: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
    });
    let app = product_router(state);

    let login_body = serde_json::json!({
        "handle": "admin",
        "password": bootstrap
    });
    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(login_body.to_string()))
                .unwrap(),
        )
        .await
        .expect("login");
    assert_eq!(login.status(), StatusCode::OK);
    let cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .expect("cookie")
        .to_str()
        .unwrap()
        .to_string();

    let create_repo = serde_json::json!({"slug": "hello", "description": "slice"});
    let repo_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie.clone())
                .body(Body::from(create_repo.to_string()))
                .unwrap(),
        )
        .await
        .expect("create repo");
    assert_eq!(repo_resp.status(), StatusCode::CREATED);

    let issue_body = serde_json::json!({"title": "First issue"});
    let issue_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/hello/issues")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie.clone())
                .body(Body::from(issue_body.to_string()))
                .unwrap(),
        )
        .await
        .expect("issue");
    assert_eq!(issue_resp.status(), StatusCode::CREATED);
    let issue_json: serde_json::Value =
        serde_json::from_slice(&issue_resp.into_body().collect().await.unwrap().to_bytes())
            .unwrap();

    let issue_number = issue_json["number"].as_i64().unwrap();
    let plan_body = serde_json::json!({"body": "slice plan v1"});
    let plan_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/hello/issues/{issue_number}/plans"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie.clone())
                .body(Body::from(plan_body.to_string()))
                .unwrap(),
        )
        .await
        .expect("plan");
    assert_eq!(plan_resp.status(), StatusCode::CREATED);
    let accept = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/hello/issues/{issue_number}/plans"
                ))
                .header(header::COOKIE, cookie.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("accept plan");
    assert_eq!(accept.status(), StatusCode::NO_CONTENT);

    let agent_body = serde_json::json!({
        "issue_id": issue_json["id"],
        "message": "implement slice"
    });
    let agent_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/hello/agent-runs")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie.clone())
                .body(Body::from(agent_body.to_string()))
                .unwrap(),
        )
        .await
        .expect("agent");
    assert_eq!(agent_resp.status(), StatusCode::OK);
    let agent_json: serde_json::Value =
        serde_json::from_slice(&agent_resp.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    let run_id = agent_json["id"].as_str().unwrap().to_string();
    assert_eq!(agent_json["adapter_kind"], "reference");

    let mut final_json = agent_json;
    for _ in 0..120 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let get = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v1/agent-runs/{run_id}"))
                    .header(header::COOKIE, cookie.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("get run");
        final_json =
            serde_json::from_slice(&get.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let state = final_json["state"].as_str().unwrap_or("");
        if matches!(state, "succeeded" | "failed" | "canceled") {
            break;
        }
    }
    assert_eq!(final_json["state"], "succeeded");
    assert!(final_json["result_commit_oid"]
        .as_str()
        .is_some_and(|c| c.len() == 40));
}
