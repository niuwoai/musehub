//! Production `/mcp` on the product API (PAT Bearer, real Postgres).

mod common;

use std::collections::HashMap;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request};
use http::HeaderValue;
use musehub_app::mcp::PRODUCTION_SCOPE;
use rmcp::{
    model::CallToolRequestParams,
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
    },
    ClientLifecycleMode, ClientServiceExt, ServiceExt,
};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as TowerServiceExt;

#[tokio::test]
async fn production_mcp_pat_repository_list() {
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
                .body(Body::from(r#"{"slug":"mcp-demo","description":""}"#))
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
                .body(Body::from(r#"{"name":"mcp-read","org":"demo"}"#))
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
    let token = pat_json["token"].as_str().unwrap().to_string();

    let org_id: (uuid::Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE slug = 'demo'")
        .fetch_one(&pool)
        .await
        .unwrap();

    let cancel = CancellationToken::new();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().unwrap();
    let router = app.clone();
    let serve_cancel = cancel.child_token();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                serve_cancel.cancelled().await;
            })
            .await;
    });

    let mut headers = HashMap::new();
    headers.insert(
        http::HeaderName::from_static("authorization"),
        HeaderValue::from_str(&format!("Bearer {token}")).expect("header"),
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
        .expect("mcp client");

    let caps = client
        .call_tool(CallToolRequestParams::new("musehub_capabilities_get"))
        .await
        .expect("capabilities");
    let structured = caps.structured_content.expect("structured");
    let data = structured.get("data").expect("data");
    assert_eq!(
        data.get("mcp_scope").and_then(Value::as_str),
        Some(PRODUCTION_SCOPE)
    );
    assert_eq!(
        data.get("maturity").and_then(Value::as_str),
        Some("partial_implementation")
    );
    assert!(data
        .get("implemented_tools")
        .and_then(Value::as_array)
        .is_some());

    let list = client
        .call_tool(
            CallToolRequestParams::new("musehub_repository_list").with_arguments({
                let mut map = serde_json::Map::new();
                map.insert("org_id".into(), json!(org_id.0.to_string()));
                map
            }),
        )
        .await
        .expect("list repos");
    let items = list
        .structured_content
        .as_ref()
        .and_then(|v| v.get("data"))
        .and_then(|d| d.get("items"))
        .and_then(Value::as_array)
        .expect("items");
    assert!(items
        .iter()
        .any(|r| r.get("slug").and_then(Value::as_str) == Some("mcp-demo")));

    client.cancel().await.expect("cancel");
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}
