//! AgentRun mainline with `musehub-agent-codex-fixture` (not online Codex).

mod common;

use std::path::PathBuf;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request};
use http_body_util::BodyExt;
use musehub_agent_adapter::CODEX_ADAPTER;
use musehub_app::bare_repo_path;
use tower::ServiceExt;

fn codex_fixture_bin() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_musehub-agent-codex-fixture")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/debug/musehub-agent-codex-fixture")
        })
}

async fn start_agent_run(
    app: &axum::Router,
    cookie: &str,
    slug: &str,
    message: &str,
    budget_tokens: Option<u64>,
) -> String {
    let issue: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/orgs/demo/repos/{slug}/issues"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, cookie)
                    .body(Body::from(r#"{"title":"fx"}"#))
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
                .uri(format!("/api/v1/orgs/demo/repos/{slug}/issues/{n}/plans"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie)
                .body(Body::from(r#"{"body":"p"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/orgs/demo/repos/{slug}/issues/{n}/plans"))
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut body = serde_json::json!({
        "issue_id": issue["id"],
        "message": message,
    });
    if let Some(b) = budget_tokens {
        body["budget_tokens"] = serde_json::json!(b);
    }
    let agent: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/orgs/demo/repos/{slug}/agent-runs"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, cookie)
                    .body(Body::from(body.to_string()))
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
    agent["id"].as_str().unwrap().to_string()
}

async fn wait_run_state(app: &axum::Router, cookie: &str, run_id: &str) -> serde_json::Value {
    for _ in 0..80 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let st: serde_json::Value = serde_json::from_slice(
            &app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/v1/agent-runs/{run_id}"))
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
        let state = st["state"].as_str().unwrap_or("");
        if matches!(state, "succeeded" | "failed" | "canceled") {
            return st;
        }
    }
    panic!("agent run {run_id} did not settle");
}

#[tokio::test]
async fn agent_run_codex_fixture_ok_on_mainline() {
    let fixture = codex_fixture_bin();
    if !fixture.is_file() {
        eprintln!("skip: musehub-agent-codex-fixture missing");
        return;
    }
    std::env::set_var("MUSEHUB_AGENT_CODEX_FIXTURE_BIN", &fixture);
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let app =
        common::product_app(pool.clone(), git_root.path(), &common::bootstrap_password()).await;
    let cookie = common::login_cookie(&app, "admin", &common::bootstrap_password()).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"slug":"codex-fx","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let run_id = start_agent_run(&app, &cookie, "codex-fx", "fixture:ok", None).await;
    let st = wait_run_state(&app, &cookie, &run_id).await;
    assert_eq!(st["state"], "succeeded");
    let kind: (String,) = sqlx::query_as("SELECT adapter_kind FROM agent_runs WHERE id = $1")
        .bind(uuid::Uuid::parse_str(&run_id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(kind.0, CODEX_ADAPTER);
}

#[tokio::test]
async fn agent_run_codex_fixture_budget_exceeded() {
    let fixture = codex_fixture_bin();
    if !fixture.is_file() {
        eprintln!("skip");
        return;
    }
    std::env::set_var("MUSEHUB_AGENT_CODEX_FIXTURE_BIN", &fixture);
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let app = common::product_app(pool, git_root.path(), &common::bootstrap_password()).await;
    let cookie = common::login_cookie(&app, "admin", &common::bootstrap_password()).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"slug":"codex-budget","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let run_id = start_agent_run(&app, &cookie, "codex-budget", "fixture:budget", Some(100)).await;
    let st = wait_run_state(&app, &cookie, &run_id).await;
    assert_eq!(st["state"], "failed");
}

#[tokio::test]
async fn agent_run_codex_fixture_timeout() {
    let fixture = codex_fixture_bin();
    if !fixture.is_file() {
        eprintln!("skip");
        return;
    }
    std::env::set_var("MUSEHUB_AGENT_CODEX_FIXTURE_BIN", &fixture);
    std::env::set_var("MUSEHUB_AGENT_RUN_DEADLINE_SECS", "2");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let app = common::product_app(pool, git_root.path(), &common::bootstrap_password()).await;
    let cookie = common::login_cookie(&app, "admin", &common::bootstrap_password()).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"slug":"codex-timeout","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let run_id = start_agent_run(&app, &cookie, "codex-timeout", "fixture:timeout", None).await;
    let st = wait_run_state(&app, &cookie, &run_id).await;
    assert_eq!(st["state"], "failed");
    assert_eq!(st["error_code"].as_str(), Some("agent_timeout"));
    std::env::remove_var("MUSEHUB_AGENT_RUN_DEADLINE_SECS");
}

#[tokio::test]
async fn agent_run_codex_fixture_cancel() {
    let fixture = codex_fixture_bin();
    if !fixture.is_file() {
        eprintln!("skip");
        return;
    }
    std::env::set_var("MUSEHUB_AGENT_CODEX_FIXTURE_BIN", &fixture);
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let app =
        common::product_app(pool.clone(), git_root.path(), &common::bootstrap_password()).await;
    let cookie = common::login_cookie(&app, "admin", &common::bootstrap_password()).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"slug":"codex-cancel","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let run_id = start_agent_run(&app, &cookie, "codex-cancel", "fixture:slow", None).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/agent-runs/{run_id}/cancel"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let st = wait_run_state(&app, &cookie, &run_id).await;
    assert_eq!(st["state"], "canceled");
    let storage_key: uuid::Uuid = sqlx::query_as::<_, (uuid::Uuid,)>(
        "SELECT storage_key FROM repositories WHERE slug_normalized = 'codex-cancel'",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .0;
    let bare = bare_repo_path(git_root.path(), &storage_key);
    assert_eq!(
        common::branch_commit_count(&bare, &format!("agent/{run_id}")),
        0
    );
}
