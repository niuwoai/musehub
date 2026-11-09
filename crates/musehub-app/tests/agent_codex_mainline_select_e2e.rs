//! AgentRun mainline selects codex adapter when requested (sim when live binary absent).

mod common;

use std::path::PathBuf;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request};
use http_body_util::BodyExt;
use musehub_agent_adapter::CODEX_ADAPTER;
use tower::ServiceExt;

fn codex_sim_bin() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_musehub-agent-codex-sim")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/debug/musehub-agent-codex-sim")
        })
}

#[tokio::test]
async fn agent_run_codex_adapter_uses_sim_on_mainline() {
    let sim = codex_sim_bin();
    if !sim.is_file() {
        eprintln!("skip: build musehub-agent-codex-sim");
        return;
    }
    std::env::remove_var("MUSEHUB_AGENT_CODEX_BIN");
    std::env::remove_var("MUSEHUB_AGENT_CODEX_FIXTURE_BIN");
    std::env::set_var("MUSEHUB_AGENT_CODEX_SIM_BIN", sim);
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
                .body(Body::from(r#"{"slug":"codex-select","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let issue: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/codex-select/issues")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(r#"{"title":"c"}"#))
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
                    "/api/v1/orgs/demo/repos/codex-select/issues/{n}/plans"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
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
                    "/api/v1/orgs/demo/repos/codex-select/issues/{n}/plans"
                ))
                .header(header::COOKIE, &cookie)
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
                    .uri("/api/v1/orgs/demo/repos/codex-select/agent-runs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(
                        serde_json::json!({
                            "issue_id": issue["id"],
                            "message": "sim-mainline",
                            "adapter": "codex"
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
    assert_eq!(agent["adapter_kind"].as_str(), Some(CODEX_ADAPTER));
    let run_id = agent["id"].as_str().unwrap();
    for _ in 0..80 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let st: serde_json::Value = serde_json::from_slice(
            &app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/v1/agent-runs/{run_id}"))
                        .header(header::COOKIE, &cookie)
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
            assert_ne!(st["state"], "failed", "{:?}", st["error_code"]);
            break;
        }
    }
}

#[tokio::test]
async fn agent_run_codex_without_binary_fails_fast() {
    std::env::remove_var("MUSEHUB_AGENT_CODEX_BIN");
    std::env::remove_var("MUSEHUB_AGENT_CODEX_SIM_BIN");
    std::env::remove_var("MUSEHUB_AGENT_CODEX_FIXTURE_BIN");
    std::env::set_var("MUSEHUB_AGENT_CODEX_SIM_BIN", "/nonexistent/codex-sim");
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
                .body(Body::from(r#"{"slug":"codex-miss","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let issue: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos/codex-miss/issues")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(r#"{"title":"m"}"#))
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
                    "/api/v1/orgs/demo/repos/codex-miss/issues/{n}/plans"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
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
                    "/api/v1/orgs/demo/repos/codex-miss/issues/{n}/plans"
                ))
                .header(header::COOKIE, &cookie)
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
                    .uri("/api/v1/orgs/demo/repos/codex-miss/agent-runs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(
                        serde_json::json!({
                            "issue_id": issue["id"],
                            "message": "x",
                            "adapter": "codex"
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
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(150)).await;
        let st: serde_json::Value = serde_json::from_slice(
            &app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/v1/agent-runs/{run_id}"))
                        .header(header::COOKIE, &cookie)
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
        if st["state"] == "failed" {
            assert_eq!(st["error_code"].as_str(), Some("codex_not_configured"));
            return;
        }
    }
    panic!("expected codex_not_configured failure");
}
