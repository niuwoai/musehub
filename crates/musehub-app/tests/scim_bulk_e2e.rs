//! SCIM Bulk bounded operation and partial-success acceptance.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

fn scim_request(token: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/api/v1/scim/v2/organizations/demo/Bulk")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/scim+json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn response_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

async fn create_token(app: &axum::Router, cookie: &str) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/orgs/demo/scim-tokens")
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
async fn scim_bulk_processes_bounded_ordered_operations_and_reports_partial_results() {
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let token = create_token(&app, &cookie).await;

    let created_user = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/scim/v2/organizations/demo/Users")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/scim+json")
                .body(Body::from(
                    json!({
                        "externalId": "bulk-existing-user",
                        "userName": "bulk-existing-user",
                        "displayName": "Before Bulk"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created_user.status(), StatusCode::CREATED);
    let user_id = response_json(created_user).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let bulk = app
        .clone()
        .oneshot(scim_request(
            &token,
            json!({
                "schemas": ["urn:ietf:params:scim:api:messages:2.0:BulkRequest"],
                "Operations": [
                    {
                        "method": "POST",
                        "bulkId": "create-user",
                        "path": "/Users",
                        "data": {
                            "externalId": "bulk-created-user",
                            "userName": "bulk-created-user",
                            "displayName": "Created in Bulk"
                        }
                    },
                    {
                        "method": "POST",
                        "bulkId": "create-group",
                        "path": "/Groups",
                        "data": {
                            "externalId": "bulk-group",
                            "displayName": "Bulk Group",
                            "members": [{"value": "bulkId:create-user", "type": "User"}]
                        }
                    },
                    {
                        "method": "PUT",
                        "path": format!("/Users/{user_id}"),
                        "data": {
                            "externalId": "bulk-existing-user",
                            "userName": "bulk-existing-user",
                            "displayName": "After Bulk",
                            "active": true
                        }
                    }
                ]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(bulk.status(), StatusCode::OK);
    assert_eq!(
        bulk.headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap(),
        "application/scim+json"
    );
    let bulk_json = response_json(bulk).await;
    assert_eq!(
        bulk_json["schemas"][0],
        "urn:ietf:params:scim:api:messages:2.0:BulkResponse"
    );
    assert_eq!(bulk_json["Operations"].as_array().unwrap().len(), 3);
    assert_eq!(bulk_json["Operations"][0]["status"], "201");
    assert_eq!(bulk_json["Operations"][0]["bulkId"], "create-user");
    assert_eq!(bulk_json["Operations"][1]["status"], "201");
    assert_eq!(bulk_json["Operations"][1]["bulkId"], "create-group");
    assert!(bulk_json["Operations"][1]["location"]
        .as_str()
        .unwrap()
        .contains("/Groups/"));
    assert_eq!(bulk_json["Operations"][2]["status"], "200");
    assert_eq!(
        bulk_json["Operations"][2]["response"]["displayName"],
        "After Bulk"
    );

    let config = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/v1/scim/v2/organizations/demo/ServiceProviderConfig")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(config.status(), StatusCode::OK);
    let config_json = response_json(config).await;
    assert_eq!(config_json["bulk"]["supported"], true);
    assert_eq!(config_json["bulk"]["maxOperations"], 100);
    assert_eq!(config_json["bulk"]["maxPayloadSize"], 1_048_576);

    let failed_then_stopped = app
        .clone()
        .oneshot(scim_request(
            &token,
            json!({
                "schemas": ["urn:ietf:params:scim:api:messages:2.0:BulkRequest"],
                "failOnErrors": 1,
                "Operations": [
                    {"method": "POST", "path": "/Users", "data": {"displayName": "Missing userName"}},
                    {"method": "POST", "path": "/Users", "data": {
                        "externalId": "must-not-run",
                        "userName": "must-not-run"
                    }}
                ]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(failed_then_stopped.status(), StatusCode::OK);
    let partial_json = response_json(failed_then_stopped).await;
    assert_eq!(partial_json["Operations"].as_array().unwrap().len(), 1);
    assert_eq!(partial_json["Operations"][0]["status"], "400");

    let too_many = app
        .clone()
        .oneshot(scim_request(
            &token,
            json!({
                "schemas": ["urn:ietf:params:scim:api:messages:2.0:BulkRequest"],
                "Operations": (0..=100).map(|_| json!({"method": "DELETE", "path": format!("/Users/{user_id}")})).collect::<Vec<_>>()
            }),
        ))
        .await
        .unwrap();
    assert_eq!(too_many.status(), StatusCode::BAD_REQUEST);
    assert_eq!(response_json(too_many).await["scimType"], "invalidValue");

    let audit_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM audit_events WHERE action IN ('scim.user.replace', 'scim.group.create')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit_count.0, 2);
}
