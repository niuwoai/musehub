//! Org role privilege: admins cannot manage owners; invite cannot demote via conflict.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

const ADMIN_PASS: &str = "Iam-Priv-Operator-12!";
const OWNER_HANDLE: &str = "admin";

#[tokio::test]
async fn admin_cannot_manage_owner_or_demote_via_reinvite() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let owner_cookie = common::login_cookie(&app, OWNER_HANDLE, &bootstrap).await;

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/members")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &owner_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "handle": "iam-priv-admin",
                        "password": ADMIN_PASS,
                        "role": "admin"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let admin_cookie = common::login_cookie(&app, "iam-priv-admin", ADMIN_PASS).await;

    let demote = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/orgs/demo/members/{OWNER_HANDLE}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"role":"member"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(demote.status(), StatusCode::FORBIDDEN);

    let disable = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/demo/members/{OWNER_HANDLE}/disable"))
                .header(header::COOKIE, &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(disable.status(), StatusCode::FORBIDDEN);

    let reinvite = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/members")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "handle": OWNER_HANDLE,
                        "password": "Unused-New-Pass-12!",
                        "role": "member"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reinvite.status(), StatusCode::CONFLICT);

    let list = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/orgs/demo/members")
                .header(header::COOKIE, &owner_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&list.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let owner_row = body
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["handle"].as_str() == Some(OWNER_HANDLE))
        .expect("owner in member list");
    assert_eq!(owner_row["org_role"].as_str(), Some("owner"));
}
