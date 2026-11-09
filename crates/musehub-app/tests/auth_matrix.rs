//! Authorization matrix: non-member forbidden; PAT cannot access foreign org repo.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

#[tokio::test]
async fn outsider_cannot_list_repos_and_pat_scoped() {
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
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"slug":"private","description":""}"#))
                .unwrap(),
        )
        .await
        .expect("repo");

    let outsider_id = uuid::Uuid::new_v4();
    let cred_id = uuid::Uuid::new_v4();
    let hash = musehub_app::hash_password_for_tests("Outsider-Strong-Pass-22!").unwrap();
    sqlx::query(
        "INSERT INTO principals (id, kind, handle, handle_normalized, display_name, status)
         VALUES ($1, 'human', 'outsider', 'outsider', 'Outsider', 'active')",
    )
    .bind(outsider_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_credentials (id, principal_id, password_hash, password_changed_at)
         VALUES ($1, $2, $3, CURRENT_TIMESTAMP)",
    )
    .bind(cred_id)
    .bind(outsider_id)
    .bind(hash)
    .execute(&pool)
    .await
    .unwrap();

    let outsider_login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "handle": "outsider",
                        "password": "Outsider-Strong-Pass-22!"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .expect("outsider login");
    assert_eq!(outsider_login.status(), StatusCode::OK);
    let outsider_cookie = outsider_login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let forbidden = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::COOKIE, &outsider_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("list repos");
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let pat_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &outsider_cookie)
                .body(Body::from(r#"{"name":"outsider-pat"}"#))
                .unwrap(),
        )
        .await
        .expect("pat");
    let pat_json: serde_json::Value =
        serde_json::from_slice(&pat_resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let token = pat_json["token"].as_str().unwrap();
    let auth = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        format!("outsider:{token}"),
    );
    let git_forbidden = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/git/demo/private.git/info/refs?service=git-upload-pack")
                .header(header::AUTHORIZATION, format!("Basic {auth}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("git");
    assert_eq!(git_forbidden.status(), StatusCode::FORBIDDEN);
}
