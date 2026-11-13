//! Repository create emits audit events filterable as `resource_type=repository`.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn repository_create_writes_repository_audit_event() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let boot = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &boot).await;
    let admin = common::login_cookie(&app, "admin", &boot).await;

    let slug = format!("audit-repo-{}", &Uuid::new_v4().simple().to_string()[..10]);
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::COOKIE, &admin)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "slug": slug, "description": "audit" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created_body = serde_json::from_slice::<serde_json::Value>(
        &created.into_body().collect().await.unwrap().to_bytes(),
    )
    .unwrap();
    let repo_id = created_body["id"].as_str().expect("repo id");
    assert_eq!(created_body["slug"].as_str(), Some(slug.as_str()));

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/orgs/demo/audit/events?resource_type=repository&limit=40")
                .header(header::COOKIE, &admin)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let body = serde_json::from_slice::<serde_json::Value>(
        &listed.into_body().collect().await.unwrap().to_bytes(),
    )
    .unwrap();
    let events = body.as_array().expect("audit array");
    let matched = events
        .iter()
        .find(|e| {
            e["action"] == "repository.create"
                && e["resource_id"] == repo_id
                && e["metadata"]["slug"] == slug
        })
        .expect("repository.create audit for this repo");
    assert_eq!(matched["resource_type"], "repository");
    assert_eq!(matched["decision"], "allow");
}
