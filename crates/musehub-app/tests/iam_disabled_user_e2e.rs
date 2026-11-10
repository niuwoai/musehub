//! IAM-01: disabled principals lose API sessions and PAT/Git HTTP access.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use base64::Engine;
use http_body_util::BodyExt;
use tower::ServiceExt;

const MEMBER_PASS: &str = "Iam-Disabled-Member-12!";

#[tokio::test]
async fn disabled_user_rejected_on_api_pat_and_git() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"slug":"iam-dis","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/members")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "handle": "iam-dis-member",
                        "password": MEMBER_PASS,
                        "role": "member"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/iam-dis/collaborators")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"handle":"iam-dis-member","role":"reader"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let member_cookie = common::login_cookie(&app, "iam-dis-member", MEMBER_PASS).await;
    let pat_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &member_cookie)
                .body(Body::from(r#"{"name":"pre-disable","org":"demo"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let pat_json: serde_json::Value =
        serde_json::from_slice(&pat_resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let token = pat_json["token"].as_str().unwrap().to_string();
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("iam-dis-member:{token}"));
    let git_ok = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/git/demo/iam-dis.git/info/refs?service=git-upload-pack")
                .header(header::AUTHORIZATION, format!("Basic {auth}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(git_ok.status(), StatusCode::OK);

    let disable = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/members/iam-dis-member/disable")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(disable.status(), StatusCode::NO_CONTENT);

    let api_denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/me")
                .header(header::COOKIE, &member_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(api_denied.status(), StatusCode::UNAUTHORIZED);

    let git_denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/git/demo/iam-dis.git/info/refs?service=git-upload-pack")
                .header(header::AUTHORIZATION, format!("Basic {auth}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(git_denied.status(), StatusCode::UNAUTHORIZED);

    let login_denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "handle": "iam-dis-member",
                        "password": MEMBER_PASS,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        login_denied.status() == StatusCode::UNAUTHORIZED
            || login_denied.status() == StatusCode::FORBIDDEN
    );
}
