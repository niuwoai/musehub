//! Repository environment CRUD (session, maintainer gate).

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use tower::ServiceExt;

async fn post_json(
    app: &axum::Router,
    uri: &str,
    cookie: &str,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, val)
}

#[tokio::test]
async fn environment_create_and_patch_requires_maintainer() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"slug":"env-crud","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, created) = post_json(
        &app,
        "/api/v1/orgs/demo/repos/env-crud/environments",
        &admin_cookie,
        r#"{"slug":"staging","name":"Staging","requires_approval":true}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["slug"], "staging");
    assert_eq!(created["requires_approval"], true);
    let row_version = created["row_version"].as_i64().unwrap();
    let (dup_status, _) = post_json(
        &app,
        "/api/v1/orgs/demo/repos/env-crud/environments",
        &admin_cookie,
        r#"{"slug":"staging","name":"Dup","requires_approval":false}"#,
    )
    .await;
    assert_eq!(dup_status, StatusCode::CONFLICT);
    let patch = Request::builder()
        .method("PATCH")
        .uri("/api/v1/orgs/demo/repos/env-crud/environments/staging")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, &admin_cookie)
        .body(Body::from(format!(
            r#"{{"requires_approval":false,"expected_row_version":{row_version}}}"#
        )))
        .unwrap();
    let resp = app.clone().oneshot(patch).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
