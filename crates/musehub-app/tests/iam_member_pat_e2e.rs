//! Ordinary org members can list/mint/revoke their PATs without org-admin APIs.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

const MEMBER_PASS: &str = "Iam-Member-Pat-E2E-12!";

#[tokio::test]
async fn member_pat_list_create_revoke_without_admin_apis() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/members")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "handle": "iam-pat-member",
                        "password": MEMBER_PASS,
                        "role": "member"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let member_cookie = common::login_cookie(&app, "iam-pat-member", MEMBER_PASS).await;

    let members_denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/orgs/demo/members")
                .header(header::COOKIE, &member_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(members_denied.status(), StatusCode::FORBIDDEN);

    let runner_tokens_denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/orgs/demo/runner-tokens")
                .header(header::COOKIE, &member_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(runner_tokens_denied.status(), StatusCode::FORBIDDEN);

    let list = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/tokens")
                .header(header::COOKIE, &member_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);

    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &member_cookie)
                .body(Body::from(r#"{"name":"member-pat","org":"demo"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&create.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let token_id = body["id"].as_str().unwrap();

    let revoke = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/tokens/{token_id}"))
                .header(header::COOKIE, &member_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoke.status(), StatusCode::NO_CONTENT);
}
