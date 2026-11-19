//! Tenant-scoped webhook subscription API and audit-trigger queueing.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

async fn request_json(
    app: &axum::Router,
    method: &str,
    uri: &str,
    cookie: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, cookie);
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    let response = app
        .clone()
        .oneshot(
            builder
                .body(Body::from(
                    body.map_or_else(String::new, |value| value.to_string()),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn webhook_subscriptions_are_admin_scoped_and_audit_events_enqueue_deliveries() {
    let encryption_key = STANDARD.encode([7_u8; 32]);
    std::env::set_var("MUSEHUB_WEBHOOK_ENCRYPTION_KEY", encryption_key);

    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;

    let (status, created) = request_json(
        &app,
        "POST",
        "/api/v1/orgs/demo/webhooks",
        &admin_cookie,
        Some(json!({
            "endpoint_url": "https://1.1.1.1/musehub-events",
            "event_types": ["issue.created"],
            "repo_id": null
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let subscription_id = created["subscription"]["id"].as_str().unwrap();
    assert_eq!(created["secret"].as_str().unwrap().len(), 43);

    let private_endpoint = request_json(
        &app,
        "POST",
        "/api/v1/orgs/demo/webhooks",
        &admin_cookie,
        Some(json!({
            "endpoint_url": "https://127.0.0.1/musehub-events",
            "event_types": ["issue.created"],
            "repo_id": null
        })),
    )
    .await;
    assert_eq!(private_endpoint.0, StatusCode::BAD_REQUEST);

    let org_id: Uuid = sqlx::query_scalar("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let admin_id: Uuid = sqlx::query_scalar("SELECT id FROM principals WHERE handle = 'admin'")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO audit_events
         (id, org_id, actor_id, action, resource_type, resource_id, decision, metadata)
         VALUES ($1, $2, $3, 'issue.created', 'issue', $4, 'allow', '{}'::jsonb)",
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(admin_id)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();

    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM webhook_deliveries WHERE subscription_id = $1 AND status = 'pending'",
    )
    .bind(Uuid::parse_str(subscription_id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        queued, 1,
        "audit insert should enqueue one durable delivery"
    );

    let (status, deliveries) = request_json(
        &app,
        "GET",
        &format!("/api/v1/orgs/demo/webhooks/{subscription_id}/deliveries"),
        &admin_cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deliveries.as_array().unwrap().len(), 1);
    assert_eq!(deliveries[0]["status"], "pending");

    let member_password = "Webhook-Member-Secret-2026!";
    let member_handle = format!("webhookmember{}", &Uuid::new_v4().simple().to_string()[..8]);
    let (status, _) = request_json(
        &app,
        "POST",
        "/api/v1/orgs/demo/members",
        &admin_cookie,
        Some(json!({
            "handle": &member_handle,
            "password": member_password,
            "role": "member"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let member_cookie = common::login_cookie(&app, &member_handle, member_password).await;
    let (status, _) = request_json(
        &app,
        "GET",
        "/api/v1/orgs/demo/webhooks",
        &member_cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = request_json(
        &app,
        "DELETE",
        &format!("/api/v1/orgs/demo/webhooks/{subscription_id}"),
        &admin_cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}
