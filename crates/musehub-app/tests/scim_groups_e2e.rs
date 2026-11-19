//! SCIM Group provisioning, tenant isolation, and CRUD acceptance.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::response::Response;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

fn scim_request(method: Method, uri: String, token: &str, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/scim+json");
    }
    builder
        .body(Body::from(
            body.map_or_else(String::new, |value| value.to_string()),
        ))
        .unwrap()
}

async fn response_json(response: Response) -> Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

async fn create_token(app: &axum::Router, cookie: &str, org: &str) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/orgs/{org}/scim-tokens"))
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"expires_in_days":1}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    response_json(response).await["token"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn groups_support_crud_membership_and_organization_isolation() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;

    let demo_token = create_token(&app, &cookie, "demo").await;
    let user = app
        .clone()
        .oneshot(scim_request(
            Method::POST,
            "/api/v1/scim/v2/organizations/demo/Users".into(),
            &demo_token,
            Some(json!({
                "externalId": "directory-user-1",
                "userName": "scim-member-1",
                "displayName": "SCIM Member"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(user.status(), StatusCode::CREATED);
    let user_id = response_json(user).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let group = app
        .clone()
        .oneshot(scim_request(
            Method::POST,
            "/api/v1/scim/v2/organizations/demo/Groups".into(),
            &demo_token,
            Some(json!({
                "externalId": "engineering-1",
                "displayName": "Engineering",
                "members": [{"value": user_id, "type": "User"}]
            })),
        ))
        .await
        .unwrap();
    assert_eq!(group.status(), StatusCode::CREATED);
    let group_response = response_json(group).await;
    let group_id = group_response["id"].as_str().unwrap().to_string();
    assert_eq!(group_response["members"][0]["display"], "SCIM Member");
    assert!(group_response["members"][0]["$ref"]
        .as_str()
        .unwrap()
        .ends_with(&user_id));

    let duplicate_external_id = app
        .clone()
        .oneshot(scim_request(
            Method::POST,
            "/api/v1/scim/v2/organizations/demo/Groups".into(),
            &demo_token,
            Some(json!({
                "externalId": "engineering-1",
                "displayName": "Duplicate"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(duplicate_external_id.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(duplicate_external_id).await["scimType"],
        "uniqueness"
    );

    let filtered = app
        .clone()
        .oneshot(scim_request(
            Method::GET,
            "/api/v1/scim/v2/organizations/demo/Groups?filter=displayName%20eq%20%22Engineering%22"
                .into(),
            &demo_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(filtered.status(), StatusCode::OK);
    let filtered_json = response_json(filtered).await;
    assert_eq!(filtered_json["totalResults"], 1);
    assert_eq!(filtered_json["Resources"][0]["id"], group_id);

    let other_org = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/orgs")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"slug":"other","display_name":"Other Organization"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(other_org.status(), StatusCode::CREATED);
    let other_token = create_token(&app, &cookie, "other").await;

    let foreign_group = app
        .clone()
        .oneshot(scim_request(
            Method::GET,
            format!("/api/v1/scim/v2/organizations/other/Groups/{group_id}"),
            &other_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(foreign_group.status(), StatusCode::NOT_FOUND);

    let foreign_member = app
        .clone()
        .oneshot(scim_request(
            Method::POST,
            "/api/v1/scim/v2/organizations/other/Groups".into(),
            &other_token,
            Some(json!({
                "externalId": "foreign-member-test",
                "displayName": "Must Reject Foreign User",
                "members": [{"value": user_id, "type": "User"}]
            })),
        ))
        .await
        .unwrap();
    assert_eq!(foreign_member.status(), StatusCode::BAD_REQUEST);

    let patched = app
        .clone()
        .oneshot(scim_request(
            Method::PATCH,
            format!("/api/v1/scim/v2/organizations/demo/Groups/{group_id}"),
            &demo_token,
            Some(json!({
                "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                "operations": [
                    {"op": "replace", "path": "displayName", "value": "Platform Engineering"},
                    {"op": "remove", "path": format!("members[value eq \"{user_id}\"]")}
                ]
            })),
        ))
        .await
        .unwrap();
    assert_eq!(patched.status(), StatusCode::OK);
    let patched_json = response_json(patched).await;
    assert_eq!(patched_json["displayName"], "Platform Engineering");
    assert!(patched_json["members"].as_array().unwrap().is_empty());

    let replaced = app
        .clone()
        .oneshot(scim_request(
            Method::PUT,
            format!("/api/v1/scim/v2/organizations/demo/Groups/{group_id}"),
            &demo_token,
            Some(json!({
                "externalId": "engineering-1",
                "displayName": "Platform",
                "members": [{"value": user_id, "type": "User"}]
            })),
        ))
        .await
        .unwrap();
    assert_eq!(replaced.status(), StatusCode::OK);
    assert_eq!(response_json(replaced).await["displayName"], "Platform");

    let deleted = app
        .clone()
        .oneshot(scim_request(
            Method::DELETE,
            format!("/api/v1/scim/v2/organizations/demo/Groups/{group_id}"),
            &demo_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

    let audit_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM audit_events WHERE action LIKE 'scim.group.%'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(audit_count.0 >= 4, "expected SCIM Group audit events");
}
