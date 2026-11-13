//! Repository environment CRUD (session, maintainer gate, CAS).

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

async fn post_json(
    app: &axum::Router,
    uri: &str,
    cookie: &str,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, val)
}

async fn patch_json(app: &axum::Router, uri: &str, cookie: &str, body: &str) -> StatusCode {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    resp.status()
}

const MEMBER_PASS: &str = "Env-Crud-Member-12!";

#[tokio::test]
async fn environment_create_and_patch_requires_maintainer() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let admin_cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let repo = format!("env-crud-{}", Uuid::new_v4().simple());
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &admin_cookie)
                .body(Body::from(json!({ "slug": repo }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    let reader = format!("envread{}", &Uuid::new_v4().simple().to_string()[..6]);
    let developer = format!("envdev{}", &Uuid::new_v4().simple().to_string()[..6]);
    let maint = format!("envmnt{}", &Uuid::new_v4().simple().to_string()[..6]);
    for (handle, pass) in [
        (reader.as_str(), MEMBER_PASS),
        (developer.as_str(), MEMBER_PASS),
        (maint.as_str(), MEMBER_PASS),
    ] {
        let invite = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/demo/members")
                    .header(header::COOKIE, &admin_cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({ "handle": handle, "password": pass, "role": "member" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invite.status(), StatusCode::CREATED);
    }
    for (handle, role) in [
        (reader.as_str(), "reader"),
        (developer.as_str(), "developer"),
        (maint.as_str(), "maintainer"),
    ] {
        let grant = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/orgs/demo/repos/{repo}/collaborators"))
                    .header(header::COOKIE, &admin_cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({ "handle": handle, "role": role }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            grant.status() == StatusCode::CREATED || grant.status() == StatusCode::NO_CONTENT,
            "grant status {}",
            grant.status()
        );
    }

    let reader_cookie = common::login_cookie(&app, &reader, MEMBER_PASS).await;
    let developer_cookie = common::login_cookie(&app, &developer, MEMBER_PASS).await;
    let maint_cookie = common::login_cookie(&app, &maint, MEMBER_PASS).await;

    let env_uri = format!("/api/v1/orgs/demo/repos/{repo}/environments");
    let (reader_status, _) = post_json(
        &app,
        &env_uri,
        &reader_cookie,
        r#"{"slug":"staging","name":"Staging","requires_approval":true}"#,
    )
    .await;
    assert_eq!(reader_status, StatusCode::FORBIDDEN);
    let (dev_status, _) = post_json(
        &app,
        &env_uri,
        &developer_cookie,
        r#"{"slug":"staging","name":"Staging","requires_approval":true}"#,
    )
    .await;
    assert_eq!(dev_status, StatusCode::FORBIDDEN);

    let (status, created) = post_json(
        &app,
        &env_uri,
        &maint_cookie,
        r#"{"slug":"staging","name":"Staging","requires_approval":true}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["slug"], "staging");
    assert_eq!(created["requires_approval"], true);
    assert!(created["row_version"].as_i64().unwrap() >= 1);

    let (dup_status, _) = post_json(
        &app,
        &env_uri,
        &maint_cookie,
        r#"{"slug":"staging","name":"Dup","requires_approval":false}"#,
    )
    .await;
    assert_eq!(dup_status, StatusCode::CONFLICT);

    let row_version = created["row_version"].as_i64().unwrap();
    let patch_uri = format!("/api/v1/orgs/demo/repos/{repo}/environments/staging");
    let ok = patch_json(
        &app,
        &patch_uri,
        &maint_cookie,
        &format!(r#"{{"requires_approval":false,"expected_row_version":{row_version}}}"#),
    )
    .await;
    assert_eq!(ok, StatusCode::OK);

    let stale = patch_json(
        &app,
        &patch_uri,
        &maint_cookie,
        &format!(r#"{{"requires_approval":true,"expected_row_version":{row_version}}}"#),
    )
    .await;
    assert_eq!(stale, StatusCode::CONFLICT);

    let long_name = "n".repeat(129);
    let bad_name = patch_json(
        &app,
        &patch_uri,
        &admin_cookie,
        &format!(
            r#"{{"name":{},"expected_row_version":{}}}"#,
            serde_json::to_string(&long_name).unwrap(),
            row_version + 1
        ),
    )
    .await;
    assert_eq!(bad_name, StatusCode::BAD_REQUEST);
}
