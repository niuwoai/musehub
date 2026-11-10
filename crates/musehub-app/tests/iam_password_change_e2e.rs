//! IAM-01: signed-in user can change password; other sessions revoked; concurrency-safe.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

const MEMBER: &str = "iam-pwd-change-member";
const INITIAL: &str = "Iam-Pwd-Member-12!";
const ROTATED_A: &str = "Rotated-Member-Secret-A9!";
const ROTATED_B: &str = "Rotated-Member-Secret-B9!";

async fn invite_member(app: &axum::Router, admin_cookie: &str) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/members")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "handle": MEMBER,
                        "password": INITIAL,
                        "role": "member",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status() == StatusCode::CREATED || resp.status() == StatusCode::NO_CONTENT);
}

async fn login_status(app: &axum::Router, handle: &str, password: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({ "handle": handle, "password": password }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

async fn change_password_request(
    app: &axum::Router,
    cookie: &str,
    current: &str,
    new_pass: &str,
) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/change-password")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie)
                .body(Body::from(
                    serde_json::json!({
                        "current_password": current,
                        "new_password": new_pass,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn password_change_revokes_other_sessions_and_requires_new_password() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    invite_member(&app, &admin_cookie).await;

    let primary_cookie = common::login_cookie(&app, MEMBER, INITIAL).await;
    let stale_cookie = common::login_cookie(&app, MEMBER, INITIAL).await;

    assert_eq!(
        change_password_request(&app, &primary_cookie, INITIAL, ROTATED_A).await,
        StatusCode::NO_CONTENT,
    );

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

    assert_eq!(
        change_password_request(&app, &primary_cookie, INITIAL, ROTATED_A).await,
        StatusCode::UNAUTHORIZED,
    );
    let _ = common::login_cookie(&app, MEMBER, ROTATED_A).await;

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
    assert!(!body.contains(INITIAL));
    assert!(!body.contains(ROTATED_A));
}

#[tokio::test]
async fn wrong_current_password_leaves_credential_unchanged() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    invite_member(&app, &admin_cookie).await;

    let cookie = common::login_cookie(&app, MEMBER, INITIAL).await;
    assert_eq!(
        change_password_request(&app, &cookie, "wrong-current-password!", ROTATED_A).await,
        StatusCode::UNAUTHORIZED,
    );
    let _ = common::login_cookie(&app, MEMBER, INITIAL).await;
}

#[tokio::test]
async fn concurrent_password_changes_allow_only_one_success() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    invite_member(&app, &admin_cookie).await;

    let cookie_a = common::login_cookie(&app, MEMBER, INITIAL).await;
    let cookie_b = common::login_cookie(&app, MEMBER, INITIAL).await;
    let app_a = app.clone();
    let app_b = app.clone();
    let (status_a, status_b) = tokio::join!(
        change_password_request(&app_a, &cookie_a, INITIAL, ROTATED_A),
        change_password_request(&app_b, &cookie_b, INITIAL, ROTATED_B),
    );
    let outcomes = [status_a, status_b];
    assert_eq!(
        outcomes
            .iter()
            .filter(|s| **s == StatusCode::NO_CONTENT)
            .count(),
        1,
    );
    assert!(
        outcomes
            .iter()
            .filter(|s| { **s == StatusCode::UNAUTHORIZED || **s == StatusCode::CONFLICT })
            .count()
            == 1,
    );

    let login_initial = login_status(&app, MEMBER, INITIAL).await;
    let login_a = login_status(&app, MEMBER, ROTATED_A).await;
    let login_b = login_status(&app, MEMBER, ROTATED_B).await;
    assert_eq!(login_initial, StatusCode::UNAUTHORIZED);
    assert_ne!(login_a, login_b);
    assert!(login_a == StatusCode::OK || login_b == StatusCode::OK);
}
