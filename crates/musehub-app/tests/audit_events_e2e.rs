//! Audit list API after login.

mod common;

use axum::body::Body;
use axum::http::{header, Request};
use http_body_util::BodyExt;
use tower::ServiceExt;

#[tokio::test]
async fn login_emits_audit_event_listable() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool, git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let list = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/orgs/demo/audit/events?limit=10")
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
        .to_bytes();
    let events: Vec<serde_json::Value> = serde_json::from_slice(&list).unwrap();
    assert!(
        events
            .iter()
            .any(|e| e["action"] == "auth.login" && e["decision"] == "allow"),
        "{events:?}"
    );
}
