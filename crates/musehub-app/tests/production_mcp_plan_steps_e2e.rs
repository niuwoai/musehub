//! plan_create steps[] persistence per 04-mcp-api PlanStep schema.

mod common;

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{header, Request};
use http::HeaderValue;
use rmcp::model::CallToolRequestParams;
use rmcp::{
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
    },
    ClientLifecycleMode, ClientServiceExt,
};
use serde_json::json;
use tokio::net::TcpListener;
use tower::ServiceExt as TowerServiceExt;
use uuid::Uuid;

fn tool_data(result: &rmcp::model::CallToolResult) -> serde_json::Value {
    let v = result.structured_content.as_ref().expect("structured").clone();
    v.get("data").cloned().unwrap_or(v)
}

async fn mcp_client(
    app: &axum::Router,
    token: &str,
) -> rmcp::service::RunningService<rmcp::RoleClient, rmcp::model::ClientConfig> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let mut headers = HashMap::new();
    headers.insert(
        http::HeaderName::from_static("authorization"),
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp"))
            .custom_headers(headers),
    );
    rmcp::model::ClientConfig::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
                legacy_version: Some(rmcp::model::ProtocolVersion::V_2025_11_25),
            },
        )
        .await
        .expect("client")
}

#[tokio::test]
async fn mcp_plan_create_persists_steps_and_plan_get_roundtrip() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool.clone(), git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let pat_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"name":"plan-steps","org":"demo"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let pat_json: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(pat_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let token = pat_json["token"].as_str().unwrap();
    let org_id: (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/demo/repos")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"slug":"plan-steps","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let repo_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM repositories WHERE slug_normalized = 'plan-steps'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let client = mcp_client(&app, token).await;
    let issue = client
        .call_tool(CallToolRequestParams::new("musehub_issue_create").with_arguments(
            serde_json::Map::from_iter([
                ("org_id".into(), json!(org_id.0.to_string())),
                ("repository_id".into(), json!(repo_id.0.to_string())),
                ("idempotency_key".into(), json!("issue-plan-steps")),
                ("title".into(), json!("Plan steps")),
                ("body".into(), json!("body")),
                (
                    "criteria".into(),
                    json!([{
                        "description": "criterion one",
                        "verification_method": "manual",
                        "required": true
                    }]),
                ),
            ]),
        ))
        .await
        .expect("issue");
    assert!(!issue.is_error.unwrap_or(true));
    let issue_data = tool_data(&issue);
    let revision_id = issue_data["current_revision_id"].as_str().unwrap();
    let plan = client
        .call_tool(CallToolRequestParams::new("musehub_plan_create").with_arguments(
            serde_json::Map::from_iter([
                ("org_id".into(), json!(org_id.0.to_string())),
                ("repository_id".into(), json!(repo_id.0.to_string())),
                ("issue_revision_id".into(), json!(revision_id)),
                ("body".into(), json!("plan body")),
                ("idempotency_key".into(), json!("plan-steps-v1")),
                (
                    "steps".into(),
                    json!([{
                        "key": "step-1",
                        "title": "Implement",
                        "description": "Do the work",
                        "criterion_ordinals": [1]
                    }]),
                ),
            ]),
        ))
        .await
        .expect("plan_create");
    assert!(!plan.is_error.unwrap_or(true));
    let plan_data = tool_data(&plan);
    let plan_id = plan_data["id"].as_str().unwrap();
    let steps = plan_data["steps"].as_array().expect("steps in response");
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0]["key"], "step-1");
    let row: (serde_json::Value,) =
        sqlx::query_as("SELECT steps FROM implementation_plans WHERE id = $1")
            .bind(Uuid::parse_str(plan_id).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.0, plan_data["steps"]);
    let get = client
        .call_tool(CallToolRequestParams::new("musehub_plan_get").with_arguments(
            serde_json::Map::from_iter([
                ("org_id".into(), json!(org_id.0.to_string())),
                ("repository_id".into(), json!(repo_id.0.to_string())),
                ("plan_id".into(), json!(plan_id)),
            ]),
        ))
        .await
        .expect("plan_get");
    assert!(!get.is_error.unwrap_or(true));
    let get_data = tool_data(&get);
    assert_eq!(get_data["steps"], plan_data["steps"]);
    let bad = client
        .call_tool(CallToolRequestParams::new("musehub_plan_create").with_arguments(
            serde_json::Map::from_iter([
                ("org_id".into(), json!(org_id.0.to_string())),
                ("repository_id".into(), json!(repo_id.0.to_string())),
                ("issue_revision_id".into(), json!(revision_id)),
                ("body".into(), json!("plan2")),
                ("idempotency_key".into(), json!("plan-bad-ord")),
                (
                    "steps".into(),
                    json!([{
                        "key": "bad",
                        "title": "x",
                        "description": "y",
                        "criterion_ordinals": [99]
                    }]),
                ),
            ]),
        ))
        .await
        .expect("bad plan");
    assert!(bad.is_error.unwrap_or(true));
    client.cancel().await.ok();
}
