//! Vertical slice integration (requires `MUSEHUB_TEST_DATABASE_URL`).

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use musehub_app::{prepare, product_router, AppState};
use musehub_migrate::{load_migrations, migrate_up};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

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
    prepare(
        pool.clone(),
        git_root.path().to_path_buf(),
        Some("Slice-Test-Bootstrap-Strong!1"),
    )
    .await
    .expect("prepare");

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
        bind_display: "127.0.0.1:8080".to_string(),
        reference_adapter_bin: adapter,
    });
    let app = product_router(state);

    let login_body = serde_json::json!({
        "handle": "admin",
        "password": "Slice-Test-Bootstrap-Strong!1"
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

    let agent_body = serde_json::json!({
        "issue_id": issue_json["id"],
        "message": "implement slice"
    });
    let agent_resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/hello/agent-runs")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie)
                .body(Body::from(agent_body.to_string()))
                .unwrap(),
        )
        .await
        .expect("agent");
    assert_eq!(agent_resp.status(), StatusCode::OK);
    let agent_json: serde_json::Value =
        serde_json::from_slice(&agent_resp.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    assert_eq!(agent_json["adapter_kind"], "reference");
    assert_eq!(agent_json["state"], "succeeded");
    assert!(agent_json["result_commit_oid"]
        .as_str()
        .is_some_and(|c| c.len() == 40));
}
