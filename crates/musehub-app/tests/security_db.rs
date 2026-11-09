//! Database-backed authorization checks (requires Postgres).

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn session_revoked_after_logout() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;

    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"handle":"admin","password":bootstrap}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .expect("login");
    let cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .expect("cookie")
        .to_str()
        .unwrap()
        .to_string();

    let logout = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/logout")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("logout");
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);

    let me = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/me")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("me");
    assert_eq!(me.status(), StatusCode::UNAUTHORIZED);
}
