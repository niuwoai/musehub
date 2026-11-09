//! Product `/mcp` interoperability: `rust-rmcp` Streamable HTTP against real Postgres (primary matrix).
//!
//! Official Python `mcp` SDK HTTP against this server is exercised in CI via
//! `scripts/verify-production-mcp-python.sh` (stdio PAT proxy); see docs/PROGRESS.md.

mod common;

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{header, Request};
use http::HeaderValue;
use musehub_app::mcp::{IMPLEMENTED_TOOLS, PRODUCTION_SCOPE};
use rmcp::{
    model::CallToolRequestParams,
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
    },
    ClientLifecycleMode, ClientServiceExt,
};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tower::ServiceExt as TowerServiceExt;

#[tokio::test]
async fn product_mcp_rust_streamable_http_matrix() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
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
                .body(Body::from(r#"{"slug":"mcp-matrix","description":""}"#))
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
                .body(Body::from(r#"{"name":"mcp-matrix","org":"demo"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let pat_json: Value = serde_json::from_slice(
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

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    let mut headers = HashMap::new();
    headers.insert(
        http::HeaderName::from_static("authorization"),
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp"))
            .custom_headers(headers),
    );
    let client = rmcp::model::ClientConfig::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
                legacy_version: Some(rmcp::model::ProtocolVersion::V_2025_11_25),
            },
        )
        .await
        .expect("rust client");
    let tools = client.list_all_tools().await.expect("list");
    let names: std::collections::HashSet<_> = tools.iter().map(|t| t.name.as_ref()).collect();
    for expected in IMPLEMENTED_TOOLS {
        assert!(names.contains(expected), "missing tool {expected}");
    }
    let caps = client
        .call_tool(CallToolRequestParams::new("musehub_capabilities_get"))
        .await
        .expect("caps");
    let data = caps
        .structured_content
        .as_ref()
        .and_then(|v| v.get("data"))
        .expect("data");
    assert_eq!(
        data.get("mcp_scope").and_then(Value::as_str),
        Some(PRODUCTION_SCOPE)
    );
    assert!(data.get("production_claim").is_none());
    let list = client
        .call_tool(
            CallToolRequestParams::new("musehub_repository_list").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("org_id".into(), json!(org_id.0.to_string()));
                m
            }),
        )
        .await
        .expect("list");
    assert!(list
        .structured_content
        .as_ref()
        .and_then(|v| v.get("data"))
        .is_some());
    client.cancel().await.ok();
}
