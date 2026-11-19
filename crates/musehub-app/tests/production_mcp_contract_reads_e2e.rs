//! Product MCP contract-read coverage: capabilities catalog lists full §5–9 tool set.

mod common;

use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request};
use http::HeaderValue;
use rmcp::{
    model::CallToolRequestParams,
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
    },
    ClientLifecycleMode, ClientServiceExt,
};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as TowerServiceExt;

fn documented_tools() -> Vec<&'static str> {
    include_str!("../../../docs/04-mcp-api.md")
        .lines()
        .filter_map(|line| {
            let first_cell = line.trim().strip_prefix('|')?.split('|').next()?.trim();
            first_cell.starts_with("musehub_").then_some(first_cell)
        })
        .collect()
}

fn documented_resource_templates() -> Vec<&'static str> {
    include_str!("../../../docs/04-mcp-api.md")
        .lines()
        .find(|line| line.starts_with("Resource URI："))
        .into_iter()
        .flat_map(|line| line.split('`'))
        .filter(|item| item.starts_with("musehub://"))
        .collect()
}

#[test]
fn documented_mcp_tool_catalog_matches_capabilities() {
    assert_eq!(
        documented_tools(),
        musehub_app::mcp::IMPLEMENTED_TOOLS,
        "MCP contract tool table must match the advertised product catalog"
    );
}

#[test]
fn documented_mcp_resource_catalog_matches_capabilities() {
    let documented = documented_resource_templates();
    assert_eq!(
        documented,
        musehub_app::mcp::V1_RESOURCE_URI_TEMPLATES,
        "MCP resource templates must match docs/04-mcp-api.md §13"
    );

    let mut documented_reads = documented;
    documented_reads.sort_unstable();
    let mut implemented_reads =
        musehub_app::mcp::capabilities::V1_RESOURCE_READS_IMPLEMENTED.to_vec();
    implemented_reads.sort_unstable();
    assert_eq!(documented_reads, implemented_reads);
}

#[tokio::test]
async fn production_mcp_capabilities_full_contract_tool_count() {
    std::env::set_var("MUSEHUB_MCP_PRODUCT", "1");
    let pool = common::migrated_pool().await;
    let git_root = tempfile::tempdir().expect("git");
    let bootstrap = common::bootstrap_password();
    let app = common::product_app(pool, git_root.path(), &bootstrap).await;
    let cookie = common::login_cookie(&app, "admin", &bootstrap).await;

    let pat_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"name":"mcp-contract-read","org":"demo"}"#))
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

    let mut headers = std::collections::HashMap::new();
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
        .expect("capabilities_get");
    let structured = caps.structured_content.expect("structured");
    let data = structured.get("data").expect("data");
    let tools = data
        .get("implemented_tools")
        .and_then(Value::as_array)
        .expect("implemented_tools");
    let expected_tools = documented_tools();
    assert!(
        !expected_tools.is_empty(),
        "MCP contract tool table is empty"
    );
    let actual_tools: Vec<&str> = tools
        .iter()
        .map(|tool| tool.as_str().expect("tool name"))
        .collect();
    assert_eq!(actual_tools, expected_tools);

    let discovered = client.list_all_tools().await.expect("tools/list");
    let mut discovered_names: Vec<&str> =
        discovered.iter().map(|tool| tool.name.as_ref()).collect();
    discovered_names.sort_unstable();
    let mut expected_names = expected_tools.to_vec();
    expected_names.sort_unstable();
    assert_eq!(discovered_names, expected_names);

    let expected_resources = documented_resource_templates();
    let advertised_resources = data
        .get("resource_uri_templates_v1")
        .and_then(Value::as_array)
        .expect("resource_uri_templates_v1");
    let advertised_resources: Vec<&str> = advertised_resources
        .iter()
        .map(|resource| resource.as_str().expect("resource URI"))
        .collect();
    assert_eq!(advertised_resources, expected_resources);

    let listed_resources = client.list_resources(None).await.expect("resources/list");
    let listed_uris: Vec<&str> = listed_resources
        .resources
        .iter()
        .map(|resource| resource.uri.as_str())
        .collect();
    assert_eq!(listed_uris, expected_resources);

    assert_eq!(
        data.get("maturity").and_then(Value::as_str),
        Some("v1_implemented")
    );
    assert_eq!(
        data.get("limits")
            .and_then(|l| l.get("full_contract_tools"))
            .and_then(Value::as_bool),
        Some(true)
    );

    client.cancel().await.expect("cancel");
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}
