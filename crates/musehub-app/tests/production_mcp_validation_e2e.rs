//! MCP input validation: schema, pagination cursor, write idempotency shape.

mod common;

use http::HeaderValue;
use rmcp::model::CallToolRequestParams;
use rmcp::{
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
    },
    ClientLifecycleMode, ClientServiceExt,
};
use serde_json::json;
use std::collections::HashMap;
use tower::ServiceExt as TowerServiceExt;

async fn client(
    app: &axum::Router,
    token: &str,
) -> rmcp::service::RunningService<rmcp::RoleClient, rmcp::model::ClientConfig> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
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
async fn repository_list_rejects_bad_cursor_and_org_id() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().unwrap();
    let app =
        common::product_app(pool.clone(), git_root.path(), &common::bootstrap_password()).await;
    let bootstrap = common::bootstrap_password();
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;
    let pat_resp = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/v1/tokens")
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .header(axum::http::header::COOKIE, &cookie)
                .body(axum::body::Body::from(r#"{"name":"val","org":"demo"}"#))
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
    let org_id: (uuid::Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();

    let mcp = client(&app, token).await;
    let bad_cursor = mcp
        .call_tool(
            CallToolRequestParams::new("musehub_repository_list").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("org_id".into(), json!(org_id.0.to_string()));
                m.insert("cursor".into(), json!("not-a-musehub-cursor"));
                m
            }),
        )
        .await;
    assert!(bad_cursor.is_err(), "invalid cursor must not succeed");
    let bad_org = mcp
        .call_tool(
            CallToolRequestParams::new("musehub_repository_list").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("org_id".into(), json!("not-a-uuid"));
                m
            }),
        )
        .await;
    assert!(bad_org.is_err() || bad_org.as_ref().unwrap().is_error.unwrap_or(false));

    let missing_key = mcp
        .call_tool(
            CallToolRequestParams::new("musehub_issue_create").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("org_id".into(), json!(org_id.0.to_string()));
                m.insert(
                    "repository_id".into(),
                    json!(uuid::Uuid::new_v4().to_string()),
                );
                m.insert("title".into(), json!("x"));
                m.insert("body".into(), json!(""));
                m
            }),
        )
        .await;
    assert!(
        missing_key.is_err()
            || missing_key
                .as_ref()
                .ok()
                .and_then(|r| r.is_error)
                .unwrap_or(false)
    );
    mcp.cancel().await.ok();
}
