//! GIT-03: binary blobs, path traversal, and bounded git read output.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

use common::s1_git_push::push_branch;

#[tokio::test]
async fn git_browse_binary_and_path_rejection() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git root").path().to_path_buf();
    let boot = common::bootstrap_password();
    let app = common::product_app(pool.clone(), &git_root, &boot).await;
    let cookie = common::login_cookie(&app, "admin", &boot).await;
    let repo = format!("browse-{}", Uuid::new_v4().simple());
    assert_eq!(
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos")
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(json!({ "slug": repo }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status(),
        StatusCode::CREATED
    );
    let storage_key: (Uuid,) =
        sqlx::query_as("SELECT storage_key FROM repositories WHERE slug_normalized = $1")
            .bind(&repo)
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut payload = vec![0u8; 1024];
    payload[0] = b'\0';
    push_branch(
        &git_root,
        storage_key.0,
        "feature/bin",
        "bin/blob.bin",
        &payload,
    );
    let head = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/git/commits?ref=feature%2Fbin&limit=1"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(head.status(), StatusCode::OK);
    let head_json: serde_json::Value =
        serde_json::from_slice(&head.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let commit = head_json["commit_oid"].as_str().unwrap();
    let blob = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/git/blob?commit={commit}&path=bin/blob.bin"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(blob.status(), StatusCode::OK);
    let blob_json: serde_json::Value =
        serde_json::from_slice(&blob.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(blob_json["binary"], true);
    assert!(blob_json["text"].is_null());
    let bad = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/git/blob?commit={commit}&path=..%2Fetc%2Fpasswd"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
}
