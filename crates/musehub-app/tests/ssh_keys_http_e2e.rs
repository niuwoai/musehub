//! SSH key IAM-02: create/revoke + audit trail.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

const TEST_PUB: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIM1RneJ7XbTm4iJThlaXp0q55nOJkRyE5PCLUeCKKWpk e2e@test";

#[tokio::test]
async fn ssh_key_create_revoke_writes_audit() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let boot = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &boot).await;
    let admin = common::login_cookie(&app, "admin", &boot).await;

    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/ssh-keys")
                .header(header::COOKIE, &admin)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "name": "e2e-laptop", "public_key": TEST_PUB }).to_string(),
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
    let key_id = created_body["id"].as_str().expect("key id");

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/orgs/demo/audit/events?resource_type=ssh_key&limit=20")
                .header(header::COOKIE, &admin)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let events = serde_json::from_slice::<serde_json::Value>(
        &listed.into_body().collect().await.unwrap().to_bytes(),
    )
    .unwrap();
    let items = events.as_array().expect("audit array");
    assert!(
        items.iter().any(|e| {
            e["action"] == "ssh_key.create"
                && e["resource_id"] == key_id
                && e["resource_type"] == "ssh_key"
        }),
        "missing ssh_key.create audit"
    );

    let key_uuid = Uuid::parse_str(key_id).unwrap();
    let revoked = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/ssh-keys/{key_uuid}"))
                .header(header::COOKIE, &admin)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::NO_CONTENT);

    let listed2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/orgs/demo/audit/events?resource_type=ssh_key&limit=20")
                .header(header::COOKIE, &admin)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let events2 = serde_json::from_slice::<serde_json::Value>(
        &listed2.into_body().collect().await.unwrap().to_bytes(),
    )
    .unwrap();
    assert!(
        events2
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["action"] == "ssh_key.revoke" && e["resource_id"] == key_id),
        "missing ssh_key.revoke audit"
    );
}
