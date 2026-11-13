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

#[tokio::test]
async fn git_browse_large_text_blob_truncated_in_api() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git root").path().to_path_buf();
    let boot = common::bootstrap_password();
    let app = common::product_app(pool.clone(), &git_root, &boot).await;
    let cookie = common::login_cookie(&app, "admin", &boot).await;
    let repo = format!("large-{}", Uuid::new_v4().simple());
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
    let payload = vec![b'x'; 300 * 1024];
    push_branch(
        &git_root,
        storage_key.0,
        "feature/large",
        "big.txt",
        &payload,
    );
    let head = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/git/commits?ref=feature%2Flarge&limit=1"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let head_json: serde_json::Value =
        serde_json::from_slice(&head.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let commit = head_json["commit_oid"].as_str().unwrap();
    let blob = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/git/blob?commit={commit}&path=big.txt"
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
    assert_eq!(blob_json["binary"], false);
    assert_eq!(blob_json["truncated"], true);
    assert!(blob_json["text"].as_str().unwrap().len() < payload.len());
}

#[tokio::test]
async fn git_browse_diff_and_tree_limits() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git root").path().to_path_buf();
    let boot = common::bootstrap_password();
    let app = common::product_app(pool.clone(), &git_root, &boot).await;
    let cookie = common::login_cookie(&app, "admin", &boot).await;
    let repo = format!("limits2-{}", Uuid::new_v4().simple());
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
    let base_payload = vec![b'a'; 150 * 1024];
    push_branch(
        &git_root,
        storage_key.0,
        "feature/base",
        "huge.txt",
        &base_payload,
    );
    let head_payload = vec![b'b'; 150 * 1024];
    push_branch(
        &git_root,
        storage_key.0,
        "feature/head",
        "huge.txt",
        &head_payload,
    );
    let base_commit = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/git/commits?ref=feature%2Fbase&limit=1"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let base_json: serde_json::Value =
        serde_json::from_slice(&base_commit.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    let base_oid = base_json["commit_oid"].as_str().unwrap();
    let head_commit = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/git/commits?ref=feature%2Fhead&limit=1"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let head_json: serde_json::Value =
        serde_json::from_slice(&head_commit.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    let head_oid = head_json["commit_oid"].as_str().unwrap();
    let diff = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/git/diff?base={base_oid}&head={head_oid}&path=huge.txt"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(diff.status(), StatusCode::OK);
    let diff_json: serde_json::Value =
        serde_json::from_slice(&diff.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(diff_json["truncated"], true);
    let tree = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/git/tree?commit={head_oid}&path=&limit=9999"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(tree.status(), StatusCode::OK);
    let tree_json: serde_json::Value =
        serde_json::from_slice(&tree.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let items = tree_json["items"].as_array().unwrap();
    assert!(items.len() <= 500);
}
