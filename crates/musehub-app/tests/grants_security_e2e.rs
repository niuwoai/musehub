//! Grant write auth, audit, self-grant deny, MCP selected PAT cross-repo.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use tower::ServiceExt;
use uuid::Uuid;

const MEMBER_PASS: &str = "Grants-Sec-Member-12!";

#[tokio::test]
async fn grant_self_and_member_write_denied_with_audit_on_success() {
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
                .body(Body::from(r#"{"slug":"g-sec-a","description":""}"#))
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
                        "handle": "g-sec-member",
                        "password": MEMBER_PASS,
                        "role": "member"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let member_cookie = common::login_cookie(&app, "g-sec-member", MEMBER_PASS).await;
    let deny = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/g-sec-a/collaborators")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &member_cookie)
                .body(Body::from(r#"{"handle":"g-sec-member","role":"reader"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deny.status(), StatusCode::FORBIDDEN);
    let self_grant = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/g-sec-a/collaborators")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"handle":"admin","role":"maintainer"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(self_grant.status(), StatusCode::BAD_REQUEST);
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/g-sec-a/collaborators")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"handle":"g-sec-member","role":"reader"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let org_id: Uuid =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM organizations WHERE slug = 'demo'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .0;
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM audit_events WHERE org_id = $1 AND action = 'repository.grant'",
    )
    .bind(org_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(count.0 >= 1);
}
