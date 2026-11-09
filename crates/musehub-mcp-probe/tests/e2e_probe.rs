//! Probe-level end-to-end mock tests (real subprocess / HTTP listener).
//!
//! These exercise the **M0 MCP probe** only — not MuseHub account UI, database, or production `/mcp`.

use std::sync::Arc;
use std::time::Duration;

use http::HeaderValue;
use musehub_mcp_probe::{echo_arguments, serve_http_probe, EXPECTED_PROBE_TOOLS, PROBE_SCOPE};
use rmcp::{
    model::CallToolRequestParams,
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
        TokioChildProcess,
    },
    ClientLifecycleMode, ClientServiceExt, ServiceExt,
};
use serde_json::Value;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

/// Stdio: spawn probe binary → initialize → list tools → echo → cancel → child exits.
#[tokio::test]
async fn e2e_stdio_subprocess_client_lifecycle() {
    let transport =
        TokioChildProcess::new(Command::new(env!("CARGO_BIN_EXE_musehub-mcp-probe-stdio")))
            .expect("spawn probe stdio");

    let client = rmcp::model::ClientConfig::default()
        .serve(transport)
        .await
        .expect("initialize");

    let tools = client.list_all_tools().await.expect("list");
    let names: Vec<_> = tools.iter().map(|t| t.name.as_ref()).collect();
    for expected in EXPECTED_PROBE_TOOLS {
        assert!(names.iter().any(|n| n == expected));
    }

    let echo = client
        .call_tool(
            CallToolRequestParams::new("musehub_probe_echo")
                .with_arguments(echo_arguments("e2e-stdio")),
        )
        .await
        .expect("echo");
    let structured = echo.structured_content.expect("structured");
    let data = structured
        .get("data")
        .and_then(Value::as_object)
        .expect("data");
    assert_eq!(
        data.get("probe_scope").and_then(Value::as_str),
        Some(PROBE_SCOPE)
    );

    client.cancel().await.expect("cancel");
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// HTTP: ephemeral port → authorized MCP round-trip → cancel listener.
#[tokio::test]
async fn e2e_http_ephemeral_listener_round_trip() {
    let cancel = CancellationToken::new();
    let bearer: Arc<str> = Arc::from("e2e-http-bearer");
    let address = serve_http_probe(bearer.clone(), cancel.child_token())
        .await
        .expect("bind");

    let mut headers = std::collections::HashMap::new();
    headers.insert(
        http::HeaderName::from_static("authorization"),
        HeaderValue::from_str(&format!("Bearer {bearer}")).expect("header"),
    );
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{address}/mcp"))
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
        .expect("http client");

    assert!(client
        .call_tool(
            CallToolRequestParams::new("musehub_probe_echo")
                .with_arguments(echo_arguments("e2e-http"))
        )
        .await
        .is_ok());
    client.cancel().await.expect("cancel");
    cancel.cancel();
}

/// Negative: empty echo must fail at the protocol/tool boundary.
#[tokio::test]
async fn e2e_stdio_rejects_whitespace_only_echo() {
    let transport =
        TokioChildProcess::new(Command::new(env!("CARGO_BIN_EXE_musehub-mcp-probe-stdio")))
            .expect("spawn");
    let client = rmcp::model::ClientConfig::default()
        .serve(transport)
        .await
        .expect("client");
    assert!(client
        .call_tool(
            CallToolRequestParams::new("musehub_probe_echo")
                .with_arguments(echo_arguments("  \t  "))
        )
        .await
        .is_err());
    client.cancel().await.expect("cancel");
}
