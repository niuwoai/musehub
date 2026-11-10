//! HTTP Bearer gate tests for the probe router (no MCP session required).

use std::sync::Arc;

use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use musehub_mcp_probe::http_probe_router;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

#[tokio::test]
async fn healthz_does_not_require_bearer() {
    let router = http_probe_router(Arc::from("probe-secret"), CancellationToken::new());
    let response = router
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .expect("healthz");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"ok");
}

#[tokio::test]
async fn mcp_post_without_bearer_is_unauthorized() {
    let router = http_probe_router(Arc::from("probe-secret"), CancellationToken::new());
    let response = router
        .oneshot(
            Request::post("/mcp")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .expect("mcp");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mcp_post_with_wrong_bearer_is_unauthorized() {
    let router = http_probe_router(Arc::from("probe-secret"), CancellationToken::new());
    let response = router
        .oneshot(
            Request::post("/mcp")
                .header("content-type", "application/json")
                .header("authorization", "Bearer wrong")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .expect("mcp");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
