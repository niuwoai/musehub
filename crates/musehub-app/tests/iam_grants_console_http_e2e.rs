//! Org default collaborator role + repository grant list/revoke (IAM console slice).

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

const MEMBER_PASS: &str = "Iam-Grants-Console-12!";

#[tokio::test]
async fn org_default_collaborator_role_and_grant_revoke_rest() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let boot = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &boot).await;
    let admin = common::login_cookie(&app, "admin", &boot).await;

    let repo = format!("iam-grants-{}", Uuid::new_v4().simple());
    let create_repo = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::COOKIE, &admin)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "slug": repo }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create_repo.status(), StatusCode::CREATED);

    let member = format!("iamg{}", &Uuid::new_v4().simple().to_string()[..8]);
    let invite = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/members")
                .header(header::COOKIE, &admin)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "handle": member,
                        "password": MEMBER_PASS,
                        "role": "member"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invite.status(), StatusCode::CREATED);

    let bad_prefs = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/orgs/demo/settings")
                .header(header::COOKIE, &admin)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "preferences": { "default_collaborator_role": "invalid" } })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad_prefs.status(), StatusCode::BAD_REQUEST);

    let patch_prefs = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/orgs/demo/settings")
                .header(header::COOKIE, &admin)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "preferences": { "default_collaborator_role": "developer" } })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(patch_prefs.status(), StatusCode::OK);
    let settings = body_json(patch_prefs).await;
    assert_eq!(
        settings["preferences"]["default_collaborator_role"],
        "developer"
    );

    let empty_handle = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/collaborators"))
                .header(header::COOKIE, &admin)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "handle": "  " }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(empty_handle.status(), StatusCode::BAD_REQUEST);

    let grant_default_role = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/collaborators"))
                .header(header::COOKIE, &admin)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "handle": member }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(grant_default_role.status(), StatusCode::NO_CONTENT);

    let list = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/grants"))
                .header(header::COOKIE, &admin)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let grants = body_json(list).await;
    let row = grants
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["handle"] == member)
        .expect("member grant listed");
    assert_eq!(row["role"], "developer");
    let principal_id = row["principal_id"].as_str().unwrap();

    let member_cookie = common::login_cookie(&app, &member, MEMBER_PASS).await;
    let member_issues = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/issues"))
                .header(header::COOKIE, &member_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(member_issues.status(), StatusCode::OK);

    let revoke = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/api/v1/orgs/demo/repos/{repo}/grants?principal_id={principal_id}"
                ))
                .header(header::COOKIE, &admin)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoke.status(), StatusCode::NO_CONTENT);

    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/orgs/demo/repos/{repo}/issues"))
                .header(header::COOKIE, &member_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
}
