//! Spot-check MCP writes that must persist rows (§5–9 implemented tools).

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
async fn mcp_discussion_comment_persists_row() {
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
                .body(Body::from(r#"{"name":"side-fx","org":"demo"}"#))
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
                .body(Body::from(r#"{"slug":"side-fx","description":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let repo_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM repositories WHERE slug_normalized = 'side-fx'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let client = mcp_client(&app, token).await;
    let issue = client
        .call_tool(CallToolRequestParams::new("musehub_issue_create").with_arguments(
            serde_json::Map::from_iter([
                ("org_id".into(), json!(org_id.0.to_string())),
                ("repository_id".into(), json!(repo_id.0.to_string())),
                ("idempotency_key".into(), json!("iss-side-fx")),
                ("title".into(), json!("Discuss")),
                ("body".into(), json!("")),
            ]),
        ))
        .await
        .expect("issue");
    let issue_id = tool_data(&issue)["id"].as_str().unwrap().to_string();
    let comment = client
        .call_tool(
            CallToolRequestParams::new("musehub_discussion_comment").with_arguments(
                serde_json::Map::from_iter([
                    ("org_id".into(), json!(org_id.0.to_string())),
                    ("repository_id".into(), json!(repo_id.0.to_string())),
                    ("idempotency_key".into(), json!("disc-side-fx-v1")),
                    (
                        "target".into(),
                        json!({"kind": "issue", "id": issue_id}),
                    ),
                    ("body".into(), json!("visible")),
                ]),
            ),
        )
        .await
        .expect("comment");
    assert!(!comment.is_error.unwrap_or(true));
    let comment_id = tool_data(&comment)["id"].as_str().unwrap().to_string();
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM discussion_comments WHERE id = $1")
        .bind(Uuid::parse_str(&comment_id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 1);
    client.cancel().await.ok();
}
