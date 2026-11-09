//! Codex HTTP adapter: sim default, live gated, timeout/budget rejection.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use serde_json::json;
use tower::ServiceExt as TowerServiceExt;

#[tokio::test]
async fn codex_status_sim_default_no_online_verified() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let app = common::product_app(pool, git_root.path(), &common::bootstrap_password()).await;
    let cookie = common::login_cookie(&app, "admin", &common::bootstrap_password()).await;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/adapters/codex/status")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(v["online_verified"], false);
    assert!(v["available_modes"]
        .as_array()
        .map(|m| m.iter().any(|x| x == "simulated_protocol"))
        .unwrap_or(false));
}

#[tokio::test]
async fn codex_task_sim_succeeds_without_live_flag() {
    std::env::remove_var("MUSEHUB_CODEX_PROBE_ALLOW_LIVE");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let app = common::product_app(pool, git_root.path(), &common::bootstrap_password()).await;
    let cookie = common::login_cookie(&app, "admin", &common::bootstrap_password()).await;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/adapters/codex/task")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(
                    json!({"message":"hello sim","timeout_seconds":10}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(v["mode"], "simulated_headless");
    assert_eq!(v["online_verified"], false);
}

#[tokio::test]
async fn codex_task_rejects_empty_message() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let app = common::product_app(pool, git_root.path(), &common::bootstrap_password()).await;
    let cookie = common::login_cookie(&app, "admin", &common::bootstrap_password()).await;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/adapters/codex/task")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"message":"   "}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
