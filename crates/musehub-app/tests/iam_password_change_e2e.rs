//! IAM-01: signed-in user can change password; other sessions revoked.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

const NEW_PASS: &str = "Rotated-Account-Secret-99!";

#[tokio::test]
async fn password_change_revokes_other_sessions_and_requires_new_password() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let primary_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let stale_cookie = common::login_cookie(&app, "admin", &bootstrap).await;

    let change = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/change-password")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &primary_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "current_password": bootstrap,
                        "new_password": NEW_PASS,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(change.status(), StatusCode::NO_CONTENT);

    let stale_me = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/me")
                .header(header::COOKIE, &stale_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale_me.status(), StatusCode::UNAUTHORIZED);

    let primary_me = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/me")
                .header(header::COOKIE, &primary_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(primary_me.status(), StatusCode::OK);

    let old_login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "handle": "admin",
                        "password": bootstrap,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(old_login.status(), StatusCode::UNAUTHORIZED);

    let audit = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/orgs/demo/audit/events?limit=20")
                .header(header::COOKIE, &primary_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let audit_bytes = audit.into_body().collect().await.unwrap().to_bytes();
    let events: Vec<serde_json::Value> = serde_json::from_slice(&audit_bytes).unwrap();
    let body = serde_json::to_string(&events).unwrap();
    assert!(events.iter().any(|e| e["action"] == "auth.password_change"));
    assert!(!body.contains(&bootstrap));
    assert!(!body.contains(NEW_PASS));
}
