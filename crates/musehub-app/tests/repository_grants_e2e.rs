//! Fine-grained repository grants across REST list + MCP PAT repo scope.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

const ALICE_PASS: &str = "Grants-Alice-Member-12!";

#[tokio::test]
async fn member_sees_only_granted_repos_and_selected_pat_denies_peer_repo() {
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
                        "handle": "grant-alice",
                        "password": ALICE_PASS,
                        "role": "member"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    for slug in ["grant-a", "grant-b"] {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/repos")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &admin_cookie)
                    .body(Body::from(format!(
                        r#"{{"slug":"{slug}","description":""}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
    }
    let repo_a: (Uuid,) = sqlx::query_as("SELECT id FROM repositories WHERE slug = 'grant-a'")
        .fetch_one(&pool)
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos/grant-a/collaborators")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(r#"{"handle":"grant-alice","role":"reader"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let alice_cookie = common::login_cookie(&app, "grant-alice", ALICE_PASS).await;
    let repos: Vec<serde_json::Value> = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/orgs/demo/repos")
                    .header(header::COOKIE, &alice_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap();
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0]["slug"], "grant-a");
    let deny = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/orgs/demo/repos/grant-b/issues")
                .header(header::COOKIE, &alice_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deny.status(), StatusCode::FORBIDDEN);
    let pat = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(
                    serde_json::json!({
                        "name": "grant-scoped",
                        "org": "demo",
                        "scopes": ["repo:read"],
                        "scope_mode": "selected",
                        "repo_ids": [repo_a.0]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let token: serde_json::Value =
        serde_json::from_slice(&pat.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let pat_str = token["token"].as_str().unwrap();
    let basic = |token: &str| {
        format!(
            "Basic {}",
            base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                format!("x:{token}")
            )
        )
    };
    let allowed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/git/demo/grant-a.git/info/refs?service=git-upload-pack")
                .header(header::AUTHORIZATION, basic(pat_str))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
    let foreign = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/git/demo/grant-b.git/info/refs?service=git-upload-pack")
                .header(header::AUTHORIZATION, basic(pat_str))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
}
