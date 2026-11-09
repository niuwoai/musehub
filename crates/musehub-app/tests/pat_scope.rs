mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use base64::Engine;
use http_body_util::BodyExt;
use tower::ServiceExt;

#[tokio::test]
async fn revoked_and_read_only_pat_git_matrix() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"slug":"pat","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let pat_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"name":"git-pat","org":"demo"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let pat_json: serde_json::Value =
        serde_json::from_slice(&pat_resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let token = pat_json["token"].as_str().unwrap();
    let token_id = pat_json["id"].as_str().unwrap();
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("admin:{token}"));
    let read_ok = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/git/demo/pat.git/info/refs?service=git-upload-pack")
                .header(header::AUTHORIZATION, format!("Basic {auth}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(read_ok.status(), StatusCode::OK);

    app.clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/tokens/{token_id}"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let revoked = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/git/demo/pat.git/info/refs?service=git-upload-pack")
                .header(header::AUTHORIZATION, format!("Basic {auth}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);

    let read_only_id = uuid::Uuid::new_v4();
    let read_only_token = musehub_app::pat::generate_pat();
    sqlx::query(
        "INSERT INTO access_tokens (id, principal_id, name, token_prefix, token_hash, scopes, expires_at, org_id)
         SELECT $1, p.id, 'read', substring($2 from 1 for 12), $3, ARRAY['repo:read']::text[], CURRENT_TIMESTAMP + interval '1 day', o.id
         FROM principals p, organizations o
         WHERE p.handle_normalized = 'admin' AND o.slug_normalized = 'demo'",
    )
    .bind(read_only_id)
    .bind(&read_only_token)
    .bind(musehub_app::pat::hash_pat(&read_only_token))
    .execute(&pool)
    .await
    .unwrap();
    let read_auth =
        base64::engine::general_purpose::STANDARD.encode(format!("admin:{read_only_token}"));
    let write_denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/git/demo/pat.git/git-receive-pack")
                .header(header::AUTHORIZATION, format!("Basic {read_auth}"))
                .header(
                    header::CONTENT_TYPE,
                    "application/x-git-receive-pack-request",
                )
                .body(Body::from("0000"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(write_denied.status(), StatusCode::FORBIDDEN);
    let _ = read_only_id;
}
