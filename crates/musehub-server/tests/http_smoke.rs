use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use musehub_core::VERSION_HEADER;
use musehub_server::{app, AppState, GIT_NOT_CONFIGURED, PROCESS_OK};
use serde_json::Value;
use tower::ServiceExt;

fn empty_state() -> Arc<AppState> {
    Arc::new(AppState {
        database: None,
        database_configured: false,
        git_root: None,
        product: None,
    })
}

async fn call(path: &str) -> (StatusCode, Value) {
    let response = app(empty_state())
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, body)
}

#[tokio::test]
async fn healthz_ok() {
    let (status, body) = call("/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], PROCESS_OK);
}

#[tokio::test]
async fn readyz_without_deps() {
    let (status, body) = call("/readyz").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["checks"]["git_storage"], GIT_NOT_CONFIGURED);
}

#[test]
fn version_header_constant() {
    assert_eq!(VERSION_HEADER, "X-App-Version");
}
