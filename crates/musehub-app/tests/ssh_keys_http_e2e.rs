//! SSH key IAM-02: create/revoke + principal-scoped audit (org_id NULL).

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
async fn ssh_key_create_revoke_writes_principal_audit_not_org_audit() {
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
    let fingerprint = created_body["fingerprint"].as_str().expect("fingerprint");

    let self_audit = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/me/audit/events?resource_type=ssh_key&limit=20")
                .header(header::COOKIE, &admin)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(self_audit.status(), StatusCode::OK);
    let self_bytes = self_audit.into_body().collect().await.unwrap().to_bytes();
    let self_json = String::from_utf8(self_bytes.to_vec()).unwrap();
    assert!(
        !self_json.contains("AAAAC3NzaC1lZDI1NTE5"),
        "audit must not contain public key material"
    );
    let items: Vec<serde_json::Value> = serde_json::from_slice(self_bytes.as_ref()).unwrap();
    let create_ev = items
        .iter()
        .find(|e| e["action"] == "ssh_key.create" && e["resource_id"] == key_id)
        .expect("missing ssh_key.create on self audit");
    assert!(create_ev["org_id"].is_null());
    assert_eq!(create_ev["metadata"]["fingerprint"], fingerprint);

    let org_audit = app
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
    let org_items: Vec<serde_json::Value> =
        serde_json::from_slice(&org_audit.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(
        !org_items
            .iter()
            .any(|e| e["resource_type"] == "ssh_key" && e["resource_id"] == key_id),
        "personal ssh_key must not appear in org audit"
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

    let self_audit2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/me/audit/events?resource_type=ssh_key&limit=20")
                .header(header::COOKIE, &admin)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let items2: Vec<serde_json::Value> =
        serde_json::from_slice(&self_audit2.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    assert!(
        items2
            .iter()
            .any(|e| e["action"] == "ssh_key.revoke" && e["resource_id"] == key_id),
        "missing ssh_key.revoke on self audit"
    );
}
